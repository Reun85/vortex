use std::{
    any::{Any, TypeId},
    collections::HashMap,
    fmt,
    marker::PhantomData,
    mem::ManuallyDrop,
    ptr,
};

use crate::{
    archetype::{ArchetypeId, Archetypes},
    change::Mut,
    component::{ComponentId, Components, Storage},
    entity::{Entities, Entity, EntityLocation},
    tick::{CHECK_TICK_THRESHOLD, Tick},
};

/// The operation targeted an entity that is not alive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeadEntity(pub Entity);

impl fmt::Display for DeadEntity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "entity {:?} is not alive", self.0)
    }
}

impl std::error::Error for DeadEntity {}

/// Owns the registry, entity allocator and archetype storage.
pub struct World {
    components: Components,
    entities: Entities,
    archetypes: Archetypes,
    change_tick: Tick,
    resources: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    last_check_tick: Tick,
    archetype_moves: u64,
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl World {
    pub fn new() -> Self {
        let components = Components::new();
        let archetypes = Archetypes::new(&components);
        Self {
            components,
            entities: Entities::new(),
            archetypes,
            change_tick: Tick::new(1),
            resources: HashMap::new(),
            last_check_tick: Tick::new(1),
            archetype_moves: 0,
        }
    }

    pub fn insert_resource<T: Send + Sync + 'static>(&mut self, value: T) {
        self.resources.insert(TypeId::of::<T>(), Box::new(value));
    }

    pub fn remove_resource<T: Send + Sync + 'static>(&mut self) -> Option<T> {
        let boxed = self.resources.remove(&TypeId::of::<T>())?;
        boxed.downcast().ok().map(|b| *b)
    }

    pub fn resource<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.resources.get(&TypeId::of::<T>())?.downcast_ref()
    }

    pub fn resource_mut<T: Send + Sync + 'static>(&mut self) -> Option<&mut T> {
        self.resources.get_mut(&TypeId::of::<T>())?.downcast_mut()
    }

    pub fn components(&self) -> &Components {
        &self.components
    }

    pub fn components_mut(&mut self) -> &mut Components {
        &mut self.components
    }

    pub fn entities(&self) -> &Entities {
        &self.entities
    }

    pub fn archetypes(&self) -> &Archetypes {
        &self.archetypes
    }

    /// How many times an entity has changed archetype, the expensive
    /// structural operation that command batching exists to minimize.
    pub fn archetype_moves(&self) -> u64 {
        self.archetype_moves
    }

    pub fn change_tick(&self) -> Tick {
        self.change_tick
    }

    #[cfg(test)]
    pub(crate) fn set_change_tick(&mut self, tick: Tick) {
        self.change_tick = tick;
    }

    /// Advances the global change tick (once per system run) and returns it.
    pub fn increment_change_tick(&mut self) -> Tick {
        self.change_tick = Tick::new(self.change_tick.get().wrapping_add(1));
        self.change_tick
    }

    pub fn component_id<T: Send + Sync + 'static>(&mut self) -> ComponentId {
        self.components.register::<T>()
    }

    pub fn is_alive(&self, e: Entity) -> bool {
        self.entities.is_alive(e)
    }

    pub fn location(&self, e: Entity) -> Option<EntityLocation> {
        self.entities.location(e)
    }

    /// Makes entities handed out by [`Entities::reserve`] alive, in the empty
    /// archetype.
    pub fn flush_entities(&mut self) {
        let empty = self.archetypes.get_mut(ArchetypeId::EMPTY);
        self.entities.flush(|e| EntityLocation {
            archetype: ArchetypeId::EMPTY,
            row: empty.push_entity(e),
        });
    }

    pub fn spawn(&mut self) -> Entity {
        self.flush_entities();
        let empty = self.archetypes.get_mut(ArchetypeId::EMPTY);
        let row = empty.len();
        let e = self.entities.alloc(EntityLocation {
            archetype: ArchetypeId::EMPTY,
            row,
        });
        empty.push_entity(e);
        e
    }

    /// Despawns `e`, dropping all of its components.
    pub fn despawn(&mut self, e: Entity) -> Result<(), DeadEntity> {
        self.flush_entities();
        let loc = self.entities.free(e).ok_or(DeadEntity(e))?;
        let arch = self.archetypes.get_mut(loc.archetype);
        for col in arch.columns_mut() {
            col.swap_remove_and_drop(loc.row);
        }
        if let Some(moved) = arch.swap_remove_entity(loc.row) {
            self.entities.set_location(moved, loc);
        }
        Ok(())
    }

    /// Adds component `c` to `e` (or overwrites it), moving the bytes at
    /// `src` into the world.
    ///
    /// # Safety
    /// `src` must point to an initialized value of `c`'s layout and type. The
    /// world takes ownership of it only if this returns `Ok`.
    ///
    /// # Panics
    /// If `c` uses sparse-set storage, which is not implemented yet.
    pub unsafe fn insert_by_id(
        &mut self,
        e: Entity,
        c: ComponentId,
        src: *const u8,
    ) -> Result<(), DeadEntity> {
        assert!(
            self.components.info(c).storage == Storage::Table,
            "sparse-set storage is not implemented yet"
        );
        self.flush_entities();
        let loc = self.entities.location(e).ok_or(DeadEntity(e))?;
        let tick = self.change_tick;
        let arch = self.archetypes.get_mut(loc.archetype);
        if let Some(col) = arch.column_mut(c) {
            // SAFETY: src is valid per the contract.
            unsafe { col.replace(loc.row, src, tick) };
            return Ok(());
        }
        let to = self
            .archetypes
            .with_added(loc.archetype, c, &self.components);
        // SAFETY: src is valid per the contract.
        unsafe { self.move_entity(e, loc, to, &[(c, src)]) };
        Ok(())
    }

    /// Adds several components to `e` at once, overwriting any it already
    /// has. The entity changes archetype at most once, straight to the final
    /// one, without passing through intermediate archetypes.
    ///
    /// # Safety
    /// Every `src` must satisfy [`insert_by_id`](Self::insert_by_id)'s
    /// contract, and the component ids must be distinct. The world takes
    /// ownership of all values only if this returns `Ok`.
    ///
    /// # Panics
    /// If any component uses sparse-set storage.
    pub unsafe fn insert_many_by_id(
        &mut self,
        e: Entity,
        items: &[(ComponentId, *const u8)],
    ) -> Result<(), DeadEntity> {
        for &(c, _) in items {
            assert!(
                self.components.info(c).storage == Storage::Table,
                "sparse-set storage is not implemented yet"
            );
        }
        self.flush_entities();
        let loc = self.entities.location(e).ok_or(DeadEntity(e))?;
        let tick = self.change_tick;
        let arch = self.archetypes.get_mut(loc.archetype);
        let mut added = Vec::new();
        for &(c, src) in items {
            match arch.column_mut(c) {
                // SAFETY: src is valid per the contract.
                Some(col) => unsafe { col.replace(loc.row, src, tick) },
                None => added.push((c, src)),
            }
        }
        let to = match added[..] {
            [] => return Ok(()),
            [(c, _)] => {
                self.archetypes
                    .with_added(loc.archetype, c, &self.components)
            }
            _ => {
                let mut comps =
                    self.archetypes.get(loc.archetype).components().to_vec();
                comps.extend(added.iter().map(|&(c, _)| c));
                comps.sort_unstable();
                self.archetypes.get_or_create(&comps, &self.components)
            }
        };
        // SAFETY: `added` holds exactly the destination's new components.
        unsafe { self.move_entity(e, loc, to, &added) };
        Ok(())
    }

    /// Removes component `c` from `e`, dropping it. Returns whether the
    /// entity had it.
    pub fn remove_by_id(
        &mut self,
        e: Entity,
        c: ComponentId,
    ) -> Result<bool, DeadEntity> {
        self.flush_entities();
        let loc = self.entities.location(e).ok_or(DeadEntity(e))?;
        if !self.archetypes.get(loc.archetype).contains(c) {
            return Ok(false);
        }
        let to =
            self.archetypes
                .with_removed(loc.archetype, c, &self.components);
        // SAFETY: no extra component is added.
        unsafe { self.move_entity(e, loc, to, &[]) };
        Ok(true)
    }

    pub fn has_by_id(&self, e: Entity, c: ComponentId) -> bool {
        self.entities
            .location(e)
            .is_some_and(|l| self.archetypes.get(l.archetype).contains(c))
    }

    pub fn get_by_id(&self, e: Entity, c: ComponentId) -> Option<*const u8> {
        let loc = self.entities.location(e)?;
        let col = self.archetypes.get(loc.archetype).column(c)?;
        Some(col.get_ptr(loc.row))
    }

    pub fn insert<T: Send + Sync + 'static>(
        &mut self,
        e: Entity,
        value: T,
    ) -> Result<(), DeadEntity> {
        let c = self.components.register::<T>();
        let value = ManuallyDrop::new(value);
        // SAFETY: `value` is a valid T matching `c`; on error we drop it.
        let result =
            unsafe { self.insert_by_id(e, c, (&raw const *value).cast()) };
        if result.is_err() {
            drop(ManuallyDrop::into_inner(value));
        }
        result
    }

    pub fn remove<T: Send + Sync + 'static>(
        &mut self,
        e: Entity,
    ) -> Result<bool, DeadEntity> {
        match self.components.id_of::<T>() {
            Some(c) => self.remove_by_id(e, c),
            None if self.is_alive(e) => Ok(false),
            None => Err(DeadEntity(e)),
        }
    }

    pub fn has<T: 'static>(&self, e: Entity) -> bool {
        self.components
            .id_of::<T>()
            .is_some_and(|c| self.has_by_id(e, c))
    }

    pub fn get<T: 'static>(&self, e: Entity) -> Option<&T> {
        let c = self.components.id_of::<T>()?;
        // SAFETY: the column for `c` stores initialized `T`s.
        self.get_by_id(e, c).map(|p| unsafe { &*p.cast::<T>() })
    }

    /// Mutable access. The component is marked changed when written through
    /// the returned [`Mut`], not when merely fetched.
    pub fn get_mut<T: 'static>(&mut self, e: Entity) -> Option<Mut<'_, T>> {
        let c = self.components.id_of::<T>()?;
        let loc = self.entities.location(e)?;
        let this_run = self.change_tick;
        let col = self.archetypes.get_mut(loc.archetype).column(c)?;
        // SAFETY: the column for `c` stores initialized `T`s, and `&mut self`
        // guarantees exclusive access to the row and its tick.
        let value = unsafe { &mut *col.get_ptr(loc.row).cast::<T>() };
        Some(Mut::new(value, col.changed_cell(loc.row), this_run))
    }

    /// Clamps every stored tick that has grown too old, once per
    /// [`CHECK_TICK_THRESHOLD`] ticks. Returns the tick it clamped against
    /// when it ran, so the caller can clamp the systems' `last_run` too.
    ///
    /// Must be called regularly (the schedule does so once per frame): a
    /// tick left unchecked for about half the `u32` range would otherwise
    /// start comparing as newer than it is.
    pub fn check_change_ticks(&mut self) -> Option<Tick> {
        let now = self.change_tick;
        if now.relative_to(self.last_check_tick).get() < CHECK_TICK_THRESHOLD {
            return None;
        }
        for arch in self.archetypes.iter_mut() {
            for col in arch.columns_mut() {
                col.check_ticks(now);
            }
        }
        self.last_check_tick = now;
        Some(now)
    }

    /// Moves `e` from `loc`'s archetype to `to`. Columns shared by both are
    /// moved bytewise (ticks kept), columns only in the source are dropped,
    /// and `extra` supplies every column only in the destination.
    ///
    /// # Safety
    /// `extra` must hold exactly the destination's additional components,
    /// each satisfying [`insert_by_id`](Self::insert_by_id)'s contract.
    unsafe fn move_entity(
        &mut self,
        e: Entity,
        loc: EntityLocation,
        to: ArchetypeId,
        extra: &[(ComponentId, *const u8)],
    ) {
        self.archetype_moves += 1;
        let tick = self.change_tick;
        let (src, dst) = self.archetypes.get_two_mut(loc.archetype, to);
        let new_row = dst.push_entity(e);
        let (dst_comps, dst_cols) = dst.components_and_columns_mut();
        for (&c, dcol) in dst_comps.iter().zip(dst_cols.iter_mut()) {
            match src.column_index(c) {
                // SAFETY: same component type; the source row is forgotten
                // below without being dropped.
                Some(si) => unsafe {
                    dcol.push_moved_from(&src.columns()[si], loc.row);
                },
                None => {
                    let &(_, ptr) = extra
                        .iter()
                        .find(|(id, _)| *id == c)
                        .expect("destination has an unexpected column");
                    // SAFETY: forwarded from this fn's contract.
                    unsafe { dcol.push(ptr, tick) };
                }
            }
        }
        let (src_comps, src_cols) = src.components_and_columns_mut();
        for (&c, scol) in src_comps.iter().zip(src_cols.iter_mut()) {
            if dst.contains(c) {
                // SAFETY: the value was moved into `dst` above.
                unsafe { scol.swap_remove_and_forget(loc.row) };
            } else {
                scol.swap_remove_and_drop(loc.row);
            }
        }
        let moved = src.swap_remove_entity(loc.row);
        if let Some(moved) = moved {
            self.entities.set_location(moved, loc);
        }
        self.entities.set_location(
            e,
            EntityLocation {
                archetype: to,
                row: new_row,
            },
        );
    }

    /// Asserts every storage invariant; used by tests.
    #[cfg(test)]
    pub(crate) fn validate(&self) {
        let mut total = 0;
        for arch in self.archetypes.iter() {
            arch.validate();
            total += arch.len();
            for (row, &e) in arch.entities().iter().enumerate() {
                assert_eq!(
                    self.entities.location(e),
                    Some(EntityLocation {
                        archetype: arch.id(),
                        row
                    })
                );
            }
        }
        assert_eq!(total, self.entities.len());
    }
}

/// Shared handle to a world for system parameters.
///
/// Several parameters of one system hold a `WorldCell` at the same time, each
/// touching disjoint parts (different component columns, different resources,
/// the entity allocator). Declared access sets are what make that sound, so
/// every accessor is `unsafe`: the caller promises not to alias.
#[derive(Clone, Copy)]
pub struct WorldCell<'w> {
    world: *mut World,
    _marker: PhantomData<&'w World>,
}

impl<'w> WorldCell<'w> {
    pub fn new(world: &'w mut World) -> Self {
        Self {
            world,
            _marker: PhantomData,
        }
    }

    /// A cell that must only be used for reads.
    pub fn from_ref(world: &'w World) -> Self {
        Self {
            world: ptr::from_ref(world).cast_mut(),
            _marker: PhantomData,
        }
    }

    /// # Safety
    /// No live `&mut` to anything inside the world.
    pub unsafe fn world(self) -> &'w World {
        // SAFETY: per contract.
        unsafe { &*self.world }
    }

    /// # Safety
    /// Exclusive access to the whole world, and the cell was built from
    /// `&mut World`.
    pub unsafe fn world_mut(self) -> &'w mut World {
        // SAFETY: per contract.
        unsafe { &mut *self.world }
    }

    /// # Safety
    /// Nobody holds `&mut` to the entity allocator.
    pub unsafe fn entities(self) -> &'w Entities {
        // SAFETY: field projection without creating `&World`.
        unsafe { &*ptr::addr_of!((*self.world).entities) }
    }

    /// # Safety
    /// Nobody holds `&mut` to the archetypes.
    pub unsafe fn archetypes(self) -> &'w Archetypes {
        // SAFETY: field projection without creating `&World`.
        unsafe { &*ptr::addr_of!((*self.world).archetypes) }
    }

    /// # Safety
    /// Exclusive access to the archetype metadata being modified (in
    /// practice: change-tick stamping of columns this caller may write), and
    /// the cell was built from `&mut World`.
    pub(crate) unsafe fn archetypes_mut(self) -> &'w mut Archetypes {
        // SAFETY: field projection without creating `&mut World`.
        unsafe { &mut *ptr::addr_of_mut!((*self.world).archetypes) }
    }

    pub fn change_tick(self) -> Tick {
        // SAFETY: a plain copy of a field nobody mutates during a system run.
        unsafe { *ptr::addr_of!((*self.world).change_tick) }
    }

    /// # Safety
    /// Nobody holds `&mut` to the resource `T`.
    pub unsafe fn resource<T: Send + Sync + 'static>(self) -> Option<&'w T> {
        // SAFETY: field projection; resources are only mutated through
        // `resource_mut` under the declared access rules.
        let map = unsafe { &*ptr::addr_of!((*self.world).resources) };
        map.get(&TypeId::of::<T>())?.downcast_ref()
    }

    /// # Safety
    /// Exclusive access to the resource `T`, and the cell was built from
    /// `&mut World`.
    pub unsafe fn resource_mut<T: Send + Sync + 'static>(
        self,
    ) -> Option<&'w mut T> {
        // SAFETY: as above; the boxed value is a separate allocation, so this
        // does not overlap other resources.
        let map = unsafe { &mut *ptr::addr_of_mut!((*self.world).resources) };
        map.get_mut(&TypeId::of::<T>())?.downcast_mut()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use proptest::prelude::*;

    use super::*;
    use crate::tick::MAX_CHANGE_AGE;

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct A(u32);
    #[derive(Debug, Clone, Copy, PartialEq)]
    #[repr(align(16))]
    struct B([u64; 2]);
    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Tag;
    #[derive(Debug, Clone, PartialEq)]
    struct S(String);

    #[test]
    fn insert_get_remove_despawn() {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, A(1)).unwrap();
        w.insert(e, S("x".into())).unwrap();
        w.insert(e, Tag).unwrap();
        assert_eq!(w.get::<A>(e), Some(&A(1)));
        assert!(w.has::<Tag>(e));
        w.insert(e, A(2)).unwrap(); // overwrite
        assert_eq!(w.get::<A>(e), Some(&A(2)));
        assert_eq!(w.remove::<A>(e), Ok(true));
        assert_eq!(w.remove::<A>(e), Ok(false));
        assert_eq!(w.get::<S>(e), Some(&S("x".into())));
        w.despawn(e).unwrap();
        assert!(!w.is_alive(e));
        assert_eq!(w.despawn(e), Err(DeadEntity(e)));
        assert_eq!(w.insert(e, A(3)), Err(DeadEntity(e)));
        w.validate();
    }

    fn changed_since(w: &World, last_run: Tick) -> usize {
        use crate::query::{QueryDesc, QueryState};
        let c = w.components().id_of::<A>().unwrap();
        let mut q = QueryState::new(QueryDesc {
            changed: vec![c],
            ..Default::default()
        });
        let mut n = 0;
        q.for_each_chunk(w, last_run, |chunk| n += chunk.len());
        n
    }

    /// Advances the tick like a long-running game, `steps` thresholds long.
    fn run_ticks(w: &mut World, steps: u32, check: bool) {
        for _ in 0..steps {
            let t = w.change_tick().get().wrapping_add(CHECK_TICK_THRESHOLD);
            w.set_change_tick(Tick::new(t));
            if check {
                w.check_change_ticks();
            }
        }
    }

    #[test]
    fn check_runs_only_once_per_threshold() {
        let mut w = World::new();
        assert_eq!(w.check_change_ticks(), None);
        w.set_change_tick(Tick::new(1 + CHECK_TICK_THRESHOLD - 1));
        assert_eq!(w.check_change_ticks(), None);
        w.set_change_tick(Tick::new(1 + CHECK_TICK_THRESHOLD));
        assert_eq!(w.check_change_ticks(), Some(w.change_tick()));
        assert_eq!(w.check_change_ticks(), None);
    }

    /// Advances the world by exactly 2^32 + 3 ticks, the point where an
    /// unchecked tick written at tick 1 aliases to "3 ticks ago".
    fn advance_to_alias_point(w: &mut World, check: bool) {
        run_ticks(w, 8, check);
        let extra = (1u64 << 32) + 3 - 8 * u64::from(CHECK_TICK_THRESHOLD);
        let t = w.change_tick().get().wrapping_add(extra as u32);
        w.set_change_tick(Tick::new(t));
        if check {
            w.check_change_ticks();
        }
    }

    #[test]
    fn stale_ticks_are_clamped_not_resurrected() {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, A(0)).unwrap(); // stamped at tick 1
        advance_to_alias_point(&mut w, true);
        let now = w.change_tick();
        assert_eq!(now, Tick::new(4), "the counter wrapped");

        // The stored tick was clamped to about the maximum age.
        let c = w.components().id_of::<A>().unwrap();
        let loc = w.location(e).unwrap();
        let col = w.archetypes().get(loc.archetype).column(c).unwrap();
        let age = now.relative_to(col.changed(loc.row)).get();
        assert!(
            (MAX_CHANGE_AGE..=MAX_CHANGE_AGE + CHECK_TICK_THRESHOLD)
                .contains(&age)
        );

        // A system that ran 5 ticks ago does not see that write as new...
        let last_run = Tick::new(now.get().wrapping_sub(5));
        assert_eq!(changed_since(&w, last_run), 0);
        // ...but a fresh write is still detected.
        w.get_mut::<A>(e).unwrap().0 = 1;
        assert_eq!(changed_since(&w, last_run), 1);
    }

    #[test]
    fn without_checks_old_ticks_alias_to_recent_ones() {
        // The negative control: skipping the checks lets the same stale
        // write look 3 ticks old once the counter has wrapped all the way
        // around. This is the bug `check_change_ticks` exists to prevent.
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, A(0)).unwrap();
        advance_to_alias_point(&mut w, false);
        let last_run = Tick::new(w.change_tick().get().wrapping_sub(5));
        assert_eq!(changed_since(&w, last_run), 1, "false positive");
    }

    #[test]
    fn detection_works_across_wraparound() {
        let mut w = World::new();
        w.set_change_tick(Tick::new(u32::MAX - 2));
        let e = w.spawn();
        w.insert(e, A(0)).unwrap(); // stamped u32::MAX - 2
        let last_run = w.change_tick();
        for _ in 0..5 {
            w.increment_change_tick(); // wraps past u32::MAX
        }
        assert!(w.change_tick().get() < 10);
        assert_eq!(changed_since(&w, last_run), 0);
        w.get_mut::<A>(e).unwrap().0 = 9; // stamped after the wrap
        assert_eq!(changed_since(&w, last_run), 1);
    }

    #[test]
    fn mut_only_marks_changed_when_written() {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, A(0)).unwrap();
        let last_run = w.change_tick();
        w.increment_change_tick();
        {
            let a = w.get_mut::<A>(e).unwrap();
            let _read = a.0; // Deref only
        }
        assert_eq!(changed_since(&w, last_run), 0);
        w.get_mut::<A>(e).unwrap().0 += 1; // DerefMut
        assert_eq!(changed_since(&w, last_run), 1);
        w.get_mut::<A>(e).unwrap().bypass_change_detection().0 += 1;
        assert_eq!(w.get::<A>(e), Some(&A(2)));
    }

    #[test]
    fn swap_remove_patches_moved_entity() {
        let mut w = World::new();
        let es: Vec<_> = (0..3).map(|_| w.spawn()).collect();
        for (i, &e) in es.iter().enumerate() {
            w.insert(e, A(i as u32)).unwrap();
        }
        w.despawn(es[0]).unwrap();
        assert_eq!(w.get::<A>(es[1]), Some(&A(1)));
        assert_eq!(w.get::<A>(es[2]), Some(&A(2)));
        w.validate();
    }

    #[test]
    fn edges_reuse_archetypes() {
        let mut w = World::new();
        let (e1, e2) = (w.spawn(), w.spawn());
        w.insert(e1, A(0)).unwrap();
        let n = w.archetypes().len();
        w.insert(e2, A(0)).unwrap();
        assert_eq!(w.archetypes().len(), n);
        // Order of insertion does not matter for archetype identity.
        w.insert(e1, B([0; 2])).unwrap();
        w.insert(e2, B([0; 2])).unwrap();
        assert_eq!(
            w.location(e1).unwrap().archetype,
            w.location(e2).unwrap().archetype
        );
    }

    #[test]
    fn reserved_entities_flush_into_world() {
        let mut w = World::new();
        let e = w.entities().reserve();
        assert!(!w.is_alive(e));
        w.flush_entities();
        assert!(w.is_alive(e));
        w.insert(e, A(5)).unwrap();
        assert_eq!(w.get::<A>(e), Some(&A(5)));
        w.validate();
    }

    #[test]
    fn change_ticks_are_kept_across_moves() {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, A(1)).unwrap();
        w.insert(e, B([1, 2])).unwrap();
        let c = w.components().id_of::<A>().unwrap();
        let loc = w.location(e).unwrap();
        let col = w.archetypes().get(loc.archetype).column(c).unwrap();
        assert_eq!(col.added(loc.row), w.change_tick());
    }

    #[derive(Debug, Default, Clone, PartialEq)]
    struct Model {
        a: Option<u32>,
        b: Option<[u64; 2]>,
        tag: bool,
        s: Option<String>,
    }

    #[derive(Debug, Clone)]
    enum Op {
        Spawn,
        Despawn(usize),
        InsertA(usize, u32),
        InsertB(usize, [u64; 2]),
        InsertTag(usize),
        InsertS(usize, String),
        RemoveA(usize),
        RemoveB(usize),
        RemoveTag(usize),
        RemoveS(usize),
        Reserve,
    }

    fn op() -> impl Strategy<Value = Op> {
        let i = any::<usize>();
        prop_oneof![
            3 => Just(Op::Spawn),
            2 => i.prop_map(Op::Despawn),
            3 => (i, any::<u32>()).prop_map(|(i, v)| Op::InsertA(i, v)),
            2 => (i, any::<[u64; 2]>()).prop_map(|(i, v)| Op::InsertB(i, v)),
            2 => i.prop_map(Op::InsertTag),
            2 => (i, "[a-z]{0,8}").prop_map(|(i, v)| Op::InsertS(i, v)),
            2 => i.prop_map(Op::RemoveA),
            2 => i.prop_map(Op::RemoveB),
            2 => i.prop_map(Op::RemoveTag),
            2 => i.prop_map(Op::RemoveS),
            1 => Just(Op::Reserve),
        ]
    }

    fn check(w: &World, model: &HashMap<Entity, Model>, all: &[Entity]) {
        w.validate();
        for &e in all {
            match model.get(&e) {
                None => {
                    assert!(!w.is_alive(e));
                    assert!(w.get::<A>(e).is_none());
                }
                Some(m) => {
                    assert!(w.is_alive(e));
                    assert_eq!(w.get::<A>(e).map(|a| a.0), m.a);
                    assert_eq!(w.get::<B>(e).map(|b| b.0), m.b);
                    assert_eq!(w.has::<Tag>(e), m.tag);
                    assert_eq!(w.get::<S>(e).map(|s| s.0.clone()), m.s);
                }
            }
        }
    }

    proptest! {
        #[test]
        fn matches_naive_model(ops in proptest::collection::vec(op(), 0..150)) {
            let mut w = World::new();
            let mut model: HashMap<Entity, Model> = HashMap::new();
            let mut all: Vec<Entity> = Vec::new();
            let pick = |all: &[Entity], i: usize| {
                (!all.is_empty()).then(|| all[i % all.len()])
            };
            for op in ops {
                match op {
                    Op::Spawn => {
                        let e = w.spawn();
                        prop_assert!(!model.contains_key(&e));
                        model.insert(e, Model::default());
                        all.push(e);
                    }
                    Op::Reserve => {
                        let e = w.entities().reserve();
                        w.flush_entities();
                        prop_assert!(!model.contains_key(&e));
                        model.insert(e, Model::default());
                        all.push(e);
                    }
                    Op::Despawn(i) => if let Some(e) = pick(&all, i) {
                        let r = w.despawn(e);
                        prop_assert_eq!(r.is_ok(), model.remove(&e).is_some());
                    },
                    Op::InsertA(i, v) => if let Some(e) = pick(&all, i) {
                        let r = w.insert(e, A(v));
                        match model.get_mut(&e) {
                            Some(m) => { prop_assert!(r.is_ok()); m.a = Some(v) }
                            None => prop_assert!(r.is_err()),
                        }
                    },
                    Op::InsertB(i, v) => if let Some(e) = pick(&all, i) {
                        let r = w.insert(e, B(v));
                        match model.get_mut(&e) {
                            Some(m) => { prop_assert!(r.is_ok()); m.b = Some(v) }
                            None => prop_assert!(r.is_err()),
                        }
                    },
                    Op::InsertTag(i) => if let Some(e) = pick(&all, i) {
                        let r = w.insert(e, Tag);
                        match model.get_mut(&e) {
                            Some(m) => { prop_assert!(r.is_ok()); m.tag = true }
                            None => prop_assert!(r.is_err()),
                        }
                    },
                    Op::InsertS(i, v) => if let Some(e) = pick(&all, i) {
                        let r = w.insert(e, S(v.clone()));
                        match model.get_mut(&e) {
                            Some(m) => { prop_assert!(r.is_ok()); m.s = Some(v) }
                            None => prop_assert!(r.is_err()),
                        }
                    },
                    Op::RemoveA(i) => if let Some(e) = pick(&all, i) {
                        let r = w.remove::<A>(e);
                        match model.get_mut(&e) {
                            Some(m) => prop_assert_eq!(r, Ok(m.a.take().is_some())),
                            None => prop_assert!(r.is_err()),
                        }
                    },
                    Op::RemoveB(i) => if let Some(e) = pick(&all, i) {
                        let r = w.remove::<B>(e);
                        match model.get_mut(&e) {
                            Some(m) => prop_assert_eq!(r, Ok(m.b.take().is_some())),
                            None => prop_assert!(r.is_err()),
                        }
                    },
                    Op::RemoveTag(i) => if let Some(e) = pick(&all, i) {
                        let r = w.remove::<Tag>(e);
                        match model.get_mut(&e) {
                            Some(m) => prop_assert_eq!(r, Ok(std::mem::take(&mut m.tag))),
                            None => prop_assert!(r.is_err()),
                        }
                    },
                    Op::RemoveS(i) => if let Some(e) = pick(&all, i) {
                        let r = w.remove::<S>(e);
                        match model.get_mut(&e) {
                            Some(m) => prop_assert_eq!(r, Ok(m.s.take().is_some())),
                            None => prop_assert!(r.is_err()),
                        }
                    },
                }
                check(&w, &model, &all);
            }
        }
    }
}
