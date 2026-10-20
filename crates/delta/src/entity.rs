use std::sync::atomic::{AtomicI64, Ordering};

use crate::archetype::ArchetypeId;

/// Low 32 bits: slot index. High 32 bits: generation.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Entity(u64);

impl Entity {
    pub const fn from_parts(index: u32, generation: u32) -> Self {
        Self((generation as u64) << 32 | index as u64)
    }

    pub const fn index(self) -> u32 {
        self.0 as u32
    }

    pub const fn generation(self) -> u32 {
        (self.0 >> 32) as u32
    }

    pub const fn to_bits(self) -> u64 {
        self.0
    }

    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntityLocation {
    pub archetype: ArchetypeId,
    pub row: usize,
}

struct Meta {
    generation: u32,
    /// `None` while free, or reserved and not yet flushed.
    location: Option<EntityLocation>,
}

/// Entity allocator: a free list of slots plus a generation per slot.
///
/// [`reserve`](Self::reserve) works through `&self` and may be called from
/// several threads; reserved entities become alive at [`flush`](Self::flush).
pub struct Entities {
    meta: Vec<Meta>,
    free: Vec<u32>,
    /// Signed cursor over `free`. Equal to `free.len()` when nothing is
    /// pending. Reservations decrement it; once it goes negative, the
    /// reserved entities are brand-new slots past the end of `meta`.
    cursor: AtomicI64,
}

impl Default for Entities {
    fn default() -> Self {
        Self::new()
    }
}

impl Entities {
    pub fn new() -> Self {
        Self {
            meta: Vec::new(),
            free: Vec::new(),
            cursor: AtomicI64::new(0),
        }
    }

    /// Hands out an entity id without needing `&mut`. It is not alive until
    /// the next [`flush`](Self::flush).
    pub fn reserve(&self) -> Entity {
        let n = self.cursor.fetch_sub(1, Ordering::Relaxed);
        if n > 0 {
            let index = self.free[(n - 1) as usize];
            Entity::from_parts(index, self.meta[index as usize].generation)
        } else {
            // n is 0 or negative: a new slot beyond the current table.
            let index = self.meta.len() as i64 - n;
            Entity::from_parts(
                u32::try_from(index).expect("too many entities"),
                0,
            )
        }
    }

    pub fn has_pending(&self) -> bool {
        self.cursor.load(Ordering::Relaxed) != self.free.len() as i64
    }

    /// Makes every reserved entity alive, placing it at `init(entity)`.
    pub fn flush(&mut self, mut init: impl FnMut(Entity) -> EntityLocation) {
        let n = *self.cursor.get_mut();
        let free_len = self.free.len() as i64;
        if n == free_len {
            return;
        }
        // Reserved from the free list: its tail from max(n, 0).
        let keep = n.max(0) as usize;
        for index in self.free.drain(keep..) {
            let meta = &mut self.meta[index as usize];
            let e = Entity::from_parts(index, meta.generation);
            meta.location = Some(init(e));
        }
        // Reserved beyond the table.
        if n < 0 {
            for _ in 0..(-n) {
                let index =
                    u32::try_from(self.meta.len()).expect("too many entities");
                let e = Entity::from_parts(index, 0);
                self.meta.push(Meta {
                    generation: 0,
                    location: Some(init(e)),
                });
            }
        }
        *self.cursor.get_mut() = self.free.len() as i64;
    }

    pub fn is_alive(&self, e: Entity) -> bool {
        self.location(e).is_some()
    }

    pub fn location(&self, e: Entity) -> Option<EntityLocation> {
        let meta = self.meta.get(e.index() as usize)?;
        if meta.generation == e.generation() {
            meta.location
        } else {
            None
        }
    }

    /// Updates the location of a live entity.
    ///
    /// # Panics
    /// If the entity is not alive.
    pub fn set_location(&mut self, e: Entity, location: EntityLocation) {
        let meta = &mut self.meta[e.index() as usize];
        assert!(
            meta.generation == e.generation() && meta.location.is_some(),
            "set_location on dead entity {e:?}"
        );
        meta.location = Some(location);
    }

    /// Allocates an entity immediately and gives it `location`.
    ///
    /// # Panics
    /// If there are unflushed reservations.
    pub fn alloc(&mut self, location: EntityLocation) -> Entity {
        assert!(!self.has_pending(), "flush reserved entities first");
        let e = if let Some(index) = self.free.pop() {
            Entity::from_parts(index, self.meta[index as usize].generation)
        } else {
            let index =
                u32::try_from(self.meta.len()).expect("too many entities");
            self.meta.push(Meta {
                generation: 0,
                location: None,
            });
            Entity::from_parts(index, 0)
        };
        self.meta[e.index() as usize].location = Some(location);
        *self.cursor.get_mut() = self.free.len() as i64;
        e
    }

    /// Frees a live entity, bumping its slot's generation. Returns its last
    /// location, or `None` if it was already dead.
    ///
    /// # Panics
    /// If there are unflushed reservations.
    pub fn free(&mut self, e: Entity) -> Option<EntityLocation> {
        assert!(!self.has_pending(), "flush reserved entities first");
        let location = self.location(e)?;
        let meta = &mut self.meta[e.index() as usize];
        meta.location = None;
        meta.generation = meta.generation.wrapping_add(1);
        self.free.push(e.index());
        *self.cursor.get_mut() = self.free.len() as i64;
        Some(location)
    }

    /// Number of live entities.
    pub fn len(&self) -> usize {
        self.meta.len() - self.free.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOC: EntityLocation = EntityLocation {
        archetype: ArchetypeId::EMPTY,
        row: 0,
    };

    #[test]
    fn bits_roundtrip() {
        let e = Entity::from_parts(7, 9);
        assert_eq!((e.index(), e.generation()), (7, 9));
        assert_eq!(Entity::from_bits(e.to_bits()), e);
    }

    #[test]
    fn stale_entity_is_dead_after_reuse() {
        let mut es = Entities::new();
        let a = es.alloc(LOC);
        assert!(es.is_alive(a));
        assert_eq!(es.free(a), Some(LOC));
        assert!(!es.is_alive(a));
        assert_eq!(es.free(a), None);
        let b = es.alloc(LOC);
        assert_eq!(b.index(), a.index());
        assert_ne!(b.generation(), a.generation());
        assert!(es.is_alive(b));
        assert!(!es.is_alive(a));
    }

    #[test]
    fn reserve_then_flush() {
        let mut es = Entities::new();
        let old = es.alloc(LOC);
        es.free(old);
        // One from the free list, two brand new.
        let r: Vec<_> = (0..3).map(|_| es.reserve()).collect();
        assert!(es.has_pending());
        assert!(r.iter().all(|&e| !es.is_alive(e)));
        let mut seen = Vec::new();
        es.flush(|e| {
            seen.push(e);
            LOC
        });
        seen.sort();
        let mut want = r.clone();
        want.sort();
        assert_eq!(seen, want);
        assert!(r.iter().all(|&e| es.is_alive(e)));
        assert!(!es.has_pending());
        assert_eq!(es.len(), 3);
        // Distinct ids.
        let mut idx: Vec<_> = r.iter().map(|e| e.index()).collect();
        idx.dedup();
        assert_eq!(idx.len(), 3);
    }

    #[test]
    fn partial_free_list_reservation() {
        let mut es = Entities::new();
        let a: Vec<_> = (0..4).map(|_| es.alloc(LOC)).collect();
        for &e in &a {
            es.free(e);
        }
        let r = es.reserve(); // takes 1 of the 4 free slots
        es.flush(|_| LOC);
        assert!(es.is_alive(r));
        assert_eq!(es.len(), 1);
        // The remaining 3 slots are still reusable.
        let more: Vec<_> = (0..3).map(|_| es.alloc(LOC)).collect();
        assert!(more.iter().all(|&e| e.index() < 4));
    }
}
