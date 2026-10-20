use std::collections::HashMap;

use fixedbitset::FixedBitSet;

use crate::{
    column::Column,
    component::{ComponentId, Components},
    entity::Entity,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ArchetypeId(u32);

impl ArchetypeId {
    /// The archetype with no components; every new entity starts here.
    pub const EMPTY: Self = Self(0);

    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// Cached single-component transitions out of an archetype.
#[derive(Default)]
pub struct ArchetypeEdges {
    pub add: HashMap<ComponentId, ArchetypeId>,
    pub remove: HashMap<ComponentId, ArchetypeId>,
}

/// All entities sharing one exact set of table components, stored as one
/// column per component plus a column of entities.
pub struct Archetype {
    id: ArchetypeId,
    components: Box<[ComponentId]>,
    columns: Box<[Column]>,
    entities: Vec<Entity>,
    bits: FixedBitSet,
    pub(crate) edges: ArchetypeEdges,
}

impl Archetype {
    fn new(
        id: ArchetypeId,
        components: Box<[ComponentId]>,
        infos: &Components,
    ) -> Self {
        debug_assert!(components.windows(2).all(|w| w[0] < w[1]));
        let columns = components
            .iter()
            .map(|&c| {
                let info = infos.info(c);
                // SAFETY: the registered drop fn matches the layout.
                unsafe { Column::new(info.layout, info.drop) }
            })
            .collect();
        let mut bits = FixedBitSet::with_capacity(infos.len());
        for c in &components {
            bits.insert(c.index());
        }
        Self {
            id,
            components,
            columns,
            entities: Vec::new(),
            bits,
            edges: ArchetypeEdges::default(),
        }
    }

    pub fn id(&self) -> ArchetypeId {
        self.id
    }

    /// Sorted component ids; the archetype's identity.
    pub fn components(&self) -> &[ComponentId] {
        &self.components
    }

    pub fn entities(&self) -> &[Entity] {
        &self.entities
    }

    pub fn len(&self) -> usize {
        self.entities.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    pub fn contains(&self, c: ComponentId) -> bool {
        self.bits.contains(c.index())
    }

    pub fn bits(&self) -> &FixedBitSet {
        &self.bits
    }

    /// Index of `c`'s column, which is also its position in `components`.
    pub fn column_index(&self, c: ComponentId) -> Option<usize> {
        self.components.binary_search(&c).ok()
    }

    pub fn column(&self, c: ComponentId) -> Option<&Column> {
        Some(&self.columns[self.column_index(c)?])
    }

    pub(crate) fn column_mut(&mut self, c: ComponentId) -> Option<&mut Column> {
        let i = self.column_index(c)?;
        Some(&mut self.columns[i])
    }

    pub fn columns(&self) -> &[Column] {
        &self.columns
    }

    pub(crate) fn columns_mut(&mut self) -> &mut [Column] {
        &mut self.columns
    }

    pub(crate) fn components_and_columns_mut(
        &mut self,
    ) -> (&[ComponentId], &mut [Column]) {
        (&self.components, &mut self.columns)
    }

    pub(crate) fn push_entity(&mut self, e: Entity) -> usize {
        self.entities.push(e);
        self.entities.len() - 1
    }

    /// Swap-removes the entity at `row` from the entity column only. Returns
    /// the entity that now occupies `row`, if one was moved into it.
    pub(crate) fn swap_remove_entity(&mut self, row: usize) -> Option<Entity> {
        self.entities.swap_remove(row);
        self.entities.get(row).copied()
    }

    /// Checks internal consistency; used by tests.
    #[cfg(test)]
    pub(crate) fn validate(&self) {
        assert!(self.components.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(self.components.len(), self.columns.len());
        for col in &self.columns {
            assert_eq!(col.len(), self.entities.len());
        }
        assert_eq!(self.bits.count_ones(..), self.components.len());
    }
}

/// All archetypes of a world, found by their sorted component set.
pub struct Archetypes {
    archetypes: Vec<Archetype>,
    lookup: HashMap<Box<[ComponentId]>, ArchetypeId>,
}

impl Archetypes {
    pub fn new(infos: &Components) -> Self {
        let mut this = Self {
            archetypes: Vec::new(),
            lookup: HashMap::new(),
        };
        let empty = this.get_or_create(&[], infos);
        debug_assert_eq!(empty, ArchetypeId::EMPTY);
        this
    }

    pub fn len(&self) -> usize {
        self.archetypes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.archetypes.is_empty()
    }

    pub fn get(&self, id: ArchetypeId) -> &Archetype {
        &self.archetypes[id.index()]
    }

    pub fn get_mut(&mut self, id: ArchetypeId) -> &mut Archetype {
        &mut self.archetypes[id.index()]
    }

    pub fn iter(&self) -> impl Iterator<Item = &Archetype> {
        self.archetypes.iter()
    }

    pub fn find(&self, sorted: &[ComponentId]) -> Option<ArchetypeId> {
        self.lookup.get(sorted).copied()
    }

    /// Finds or creates the archetype with exactly these (sorted, unique)
    /// components.
    pub fn get_or_create(
        &mut self,
        sorted: &[ComponentId],
        infos: &Components,
    ) -> ArchetypeId {
        if let Some(id) = self.find(sorted) {
            return id;
        }
        let id = ArchetypeId(
            u32::try_from(self.archetypes.len()).expect("too many archetypes"),
        );
        let boxed: Box<[ComponentId]> = sorted.into();
        self.archetypes
            .push(Archetype::new(id, boxed.clone(), infos));
        self.lookup.insert(boxed, id);
        id
    }

    /// Archetype reached by adding `c` to `from`, via the edge cache.
    pub fn with_added(
        &mut self,
        from: ArchetypeId,
        c: ComponentId,
        infos: &Components,
    ) -> ArchetypeId {
        if let Some(&to) = self.get(from).edges.add.get(&c) {
            return to;
        }
        let mut comps = self.get(from).components.to_vec();
        let at = comps
            .binary_search(&c)
            .expect_err("component already present");
        comps.insert(at, c);
        let to = self.get_or_create(&comps, infos);
        self.get_mut(from).edges.add.insert(c, to);
        self.get_mut(to).edges.remove.insert(c, from);
        to
    }

    /// Archetype reached by removing `c` from `from`, via the edge cache.
    pub fn with_removed(
        &mut self,
        from: ArchetypeId,
        c: ComponentId,
        infos: &Components,
    ) -> ArchetypeId {
        if let Some(&to) = self.get(from).edges.remove.get(&c) {
            return to;
        }
        let mut comps = self.get(from).components.to_vec();
        let at = comps.binary_search(&c).expect("component not present");
        comps.remove(at);
        let to = self.get_or_create(&comps, infos);
        self.get_mut(from).edges.remove.insert(c, to);
        self.get_mut(to).edges.add.insert(c, from);
        to
    }

    /// Mutable access to two distinct archetypes at once.
    pub fn get_two_mut(
        &mut self,
        a: ArchetypeId,
        b: ArchetypeId,
    ) -> (&mut Archetype, &mut Archetype) {
        assert_ne!(a, b);
        let (ai, bi) = (a.index(), b.index());
        if ai < bi {
            let (l, r) = self.archetypes.split_at_mut(bi);
            (&mut l[ai], &mut r[0])
        } else {
            let (l, r) = self.archetypes.split_at_mut(ai);
            (&mut r[0], &mut l[bi])
        }
    }
}
