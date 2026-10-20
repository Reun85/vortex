use std::{fmt, ops::Range};

use fixedbitset::FixedBitSet;

use crate::{
    archetype::{Archetype, ArchetypeId},
    column::Column,
    component::ComponentId,
    entity::Entity,
    tick::Tick,
    world::World,
};

/// Marks an optional term whose component the archetype lacks.
const ABSENT: usize = usize::MAX;

/// What a query reads, writes and filters on, as plain component ids.
///
/// Data terms are numbered in the order `read`, `write`, `optional`; that
/// number is what [`Chunk`] accessors take.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QueryDesc {
    pub read: Vec<ComponentId>,
    pub write: Vec<ComponentId>,
    /// Must be present; no data access.
    pub with: Vec<ComponentId>,
    /// Must be absent.
    pub without: Vec<ComponentId>,
    /// Read when present; may be absent.
    pub optional: Vec<ComponentId>,
    /// Must be present and added since the last run (per row).
    pub added: Vec<ComponentId>,
    /// Must be present and changed since the last run (per row).
    pub changed: Vec<ComponentId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueryError {
    /// A component is accessed by more than one data term.
    AccessConflict(ComponentId),
    /// A component is both required and excluded.
    RequiredAndExcluded(ComponentId),
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AccessConflict(c) => {
                write!(f, "component {c:?} is accessed more than once")
            }
            Self::RequiredAndExcluded(c) => {
                write!(f, "component {c:?} is both required and excluded")
            }
        }
    }
}

impl std::error::Error for QueryError {}

impl QueryDesc {
    pub fn term_count(&self) -> usize {
        self.read.len() + self.write.len() + self.optional.len()
    }

    fn terms(&self) -> impl Iterator<Item = ComponentId> + '_ {
        self.read
            .iter()
            .chain(&self.write)
            .chain(&self.optional)
            .copied()
    }

    pub fn validate(&self) -> Result<(), QueryError> {
        let mut seen = Vec::new();
        for c in self.terms() {
            if seen.contains(&c) {
                return Err(QueryError::AccessConflict(c));
            }
            seen.push(c);
        }
        for &c in &self.without {
            let required = self
                .terms()
                .chain(self.with.iter().copied())
                .chain(self.added.iter().copied())
                .chain(self.changed.iter().copied())
                .any(|r| r == c);
            if required {
                return Err(QueryError::RequiredAndExcluded(c));
            }
        }
        Ok(())
    }

    /// Components an archetype must have to match.
    fn required(&self) -> impl Iterator<Item = ComponentId> + '_ {
        self.read
            .iter()
            .chain(&self.write)
            .chain(&self.with)
            .chain(&self.added)
            .chain(&self.changed)
            .copied()
    }
}

struct Match {
    archetype: ArchetypeId,
    /// Column index per data term, [`ABSENT`] for a missing optional.
    columns: Box<[usize]>,
    added: Box<[usize]>,
    changed: Box<[usize]>,
}

impl Match {
    fn row_passes(&self, arch: &Archetype, row: usize, last_run: Tick) -> bool {
        let cols = arch.columns();
        self.added
            .iter()
            .all(|&i| cols[i].added(row).is_newer_than(last_run))
            && self
                .changed
                .iter()
                .all(|&i| cols[i].changed(row).is_newer_than(last_run))
    }

    fn has_row_filters(&self) -> bool {
        !self.added.is_empty() || !self.changed.is_empty()
    }

    /// The next maximal run of passing rows at or after `start`.
    fn next_run(
        &self,
        arch: &Archetype,
        start: usize,
        last_run: Tick,
    ) -> Option<Range<usize>> {
        let n = arch.len();
        if !self.has_row_filters() {
            return (start == 0 && n > 0).then_some(0..n);
        }
        let first = (start..n).find(|&r| self.row_passes(arch, r, last_run))?;
        let mut end = first + 1;
        while end < n && self.row_passes(arch, end, last_run) {
            end += 1;
        }
        Some(first..end)
    }
}

/// A compiled query: caches the archetypes that match and learns about new
/// ones incrementally, so iteration never re-matches.
pub struct QueryState {
    desc: QueryDesc,
    required: FixedBitSet,
    excluded: FixedBitSet,
    matched: Vec<Match>,
    /// Number of archetypes already checked.
    seen_archetypes: usize,
}

impl QueryState {
    /// # Panics
    /// If the description is invalid, see [`QueryDesc::validate`].
    pub fn new(desc: QueryDesc) -> Self {
        if let Err(e) = desc.validate() {
            panic!("invalid query: {e}");
        }
        let mut required = FixedBitSet::new();
        for c in desc.required() {
            required.grow(c.index() + 1);
            required.insert(c.index());
        }
        let mut excluded = FixedBitSet::new();
        for c in &desc.without {
            excluded.grow(c.index() + 1);
            excluded.insert(c.index());
        }
        Self {
            desc,
            required,
            excluded,
            matched: Vec::new(),
            seen_archetypes: 0,
        }
    }

    pub fn desc(&self) -> &QueryDesc {
        &self.desc
    }

    /// Archetypes currently known to match (after the last update).
    pub fn matched_archetypes(&self) -> impl Iterator<Item = ArchetypeId> + '_ {
        self.matched.iter().map(|m| m.archetype)
    }

    /// Checks archetypes created since the last call.
    pub fn update(&mut self, world: &World) {
        let archetypes = world.archetypes();
        for arch in archetypes.iter().skip(self.seen_archetypes) {
            if self.required.is_subset(arch.bits())
                && self.excluded.is_disjoint(arch.bits())
            {
                self.matched.push(self.make_match(arch));
            }
        }
        self.seen_archetypes = archetypes.len();
    }

    fn make_match(&self, arch: &Archetype) -> Match {
        let col = |c: ComponentId| {
            arch.column_index(c).expect("archetype was matched on this")
        };
        let columns = self
            .desc
            .read
            .iter()
            .chain(&self.desc.write)
            .map(|&c| col(c))
            .chain(
                self.desc
                    .optional
                    .iter()
                    .map(|&c| arch.column_index(c).unwrap_or(ABSENT)),
            )
            .collect();
        Match {
            archetype: arch.id(),
            columns,
            added: self.desc.added.iter().map(|&c| col(c)).collect(),
            changed: self.desc.changed.iter().map(|&c| col(c)).collect(),
        }
    }

    /// Calls `f` with every run of matching rows. Read-only queries only.
    ///
    /// `last_run` is the tick at which the caller last ran, for the `added`
    /// and `changed` filters.
    ///
    /// # Panics
    /// If the query has write terms; use [`for_each_chunk_mut`].
    ///
    /// [`for_each_chunk_mut`]: Self::for_each_chunk_mut
    pub fn for_each_chunk(
        &mut self,
        world: &World,
        last_run: Tick,
        mut f: impl FnMut(Chunk<'_>),
    ) {
        assert!(
            self.desc.write.is_empty(),
            "query has write terms; use for_each_chunk_mut"
        );
        self.update(world);
        for m in &self.matched {
            let arch = world.archetypes().get(m.archetype);
            let mut start = 0;
            while let Some(rows) = m.next_run(arch, start, last_run) {
                start = rows.end;
                f(Chunk::new(arch, m, rows));
            }
        }
    }

    /// Like [`for_each_chunk`](Self::for_each_chunk), but permits write terms.
    /// Each yielded run of rows has its write columns stamped as changed at
    /// the world's current tick, the same coarse tracking a span write gets.
    pub fn for_each_chunk_mut(
        &mut self,
        world: &mut World,
        last_run: Tick,
        mut f: impl FnMut(Chunk<'_>),
    ) {
        self.update(world);
        let tick = world.change_tick();
        let writes =
            self.desc.read.len()..self.desc.read.len() + self.desc.write.len();
        for m in &self.matched {
            let mut start = 0;
            loop {
                let arch = world.archetypes().get(m.archetype);
                let Some(rows) = m.next_run(arch, start, last_run) else {
                    break;
                };
                start = rows.end;
                let arch = world.archetypes_mut().get_mut(m.archetype);
                for term in writes.clone() {
                    arch.columns_mut()[m.columns[term]]
                        .set_changed_range(rows.clone(), tick);
                }
                let arch = world.archetypes().get(m.archetype);
                f(Chunk::new(arch, m, rows));
            }
        }
    }
}

/// A run of consecutive rows of one archetype: the shared iteration
/// primitive. Typed Rust and C# layers wrap it as slices and spans.
pub struct Chunk<'w> {
    archetype: &'w Archetype,
    columns: &'w [usize],
    rows: Range<usize>,
}

impl<'w> Chunk<'w> {
    fn new(archetype: &'w Archetype, m: &'w Match, rows: Range<usize>) -> Self {
        Self {
            archetype,
            columns: &m.columns,
            rows,
        }
    }

    pub fn archetype(&self) -> ArchetypeId {
        self.archetype.id()
    }

    pub fn rows(&self) -> Range<usize> {
        self.rows.clone()
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn entities(&self) -> &'w [Entity] {
        &self.archetype.entities()[self.rows.clone()]
    }

    /// The column behind data term `term`, or `None` for an absent optional.
    pub fn column(&self, term: usize) -> Option<&'w Column> {
        match self.columns[term] {
            ABSENT => None,
            i => Some(&self.archetype.columns()[i]),
        }
    }

    /// Pointer to the first row of this chunk in term `term`'s column, or
    /// `None` for an absent optional. Valid for `len()` consecutive elements
    /// (none, for zero-sized tags). Writing through it is only allowed for
    /// write terms.
    pub fn ptr(&self, term: usize) -> Option<*mut u8> {
        self.column(term).map(|c| c.get_ptr(self.rows.start))
    }

    /// # Safety
    /// `T` must be the component type of `term`, which must be present.
    pub unsafe fn slice<T>(&self, term: usize) -> &'w [T] {
        let ptr = self.ptr(term).expect("term is absent");
        // SAFETY: the column holds `len()` initialized `T`s from this row.
        unsafe { std::slice::from_raw_parts(ptr.cast::<T>(), self.len()) }
    }

    /// # Safety
    /// `T` must be the component type of `term`, which must be a present
    /// write term, and no other reference to it may be live.
    pub unsafe fn slice_mut<T>(&mut self, term: usize) -> &'w mut [T] {
        let ptr = self.ptr(term).expect("term is absent");
        // SAFETY: as above, and exclusivity is the caller's contract.
        unsafe { std::slice::from_raw_parts_mut(ptr.cast::<T>(), self.len()) }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap};

    use proptest::prelude::*;

    use super::*;
    use crate::world::World;

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct A(u32);
    #[derive(Debug, Clone, Copy, PartialEq)]
    struct B(u32);
    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Tag;

    fn ids(w: &mut World) -> (ComponentId, ComponentId, ComponentId) {
        (
            w.component_id::<A>(),
            w.component_id::<B>(),
            w.component_id::<Tag>(),
        )
    }

    fn collect(
        q: &mut QueryState,
        w: &World,
        last_run: Tick,
    ) -> BTreeSet<Entity> {
        let mut out = BTreeSet::new();
        q.for_each_chunk(w, last_run, |chunk| {
            assert!(!chunk.is_empty());
            out.extend(chunk.entities().iter().copied());
        });
        out
    }

    #[test]
    fn validate_rejects_conflicts() {
        let mut w = World::new();
        let (a, b, _) = ids(&mut w);
        let desc = QueryDesc {
            read: vec![a],
            write: vec![a],
            ..Default::default()
        };
        assert_eq!(desc.validate(), Err(QueryError::AccessConflict(a)));
        let desc = QueryDesc {
            read: vec![a],
            without: vec![a],
            ..Default::default()
        };
        assert_eq!(desc.validate(), Err(QueryError::RequiredAndExcluded(a)));
        let desc = QueryDesc {
            read: vec![a],
            without: vec![b],
            ..Default::default()
        };
        assert_eq!(desc.validate(), Ok(()));
    }

    #[test]
    fn matching_is_incremental() {
        let mut w = World::new();
        let (a, b, _) = ids(&mut w);
        let mut q = QueryState::new(QueryDesc {
            read: vec![a],
            without: vec![b],
            ..Default::default()
        });
        q.update(&w);
        assert_eq!(q.matched_archetypes().count(), 0);

        let e1 = w.spawn();
        w.insert(e1, A(1)).unwrap();
        assert_eq!(collect(&mut q, &w, Tick::new(0)), BTreeSet::from([e1]));
        assert_eq!(q.matched_archetypes().count(), 1);

        // A new archetype appears later and is picked up on next use.
        let e2 = w.spawn();
        w.insert(e2, A(2)).unwrap();
        w.insert(e2, Tag).unwrap();
        w.insert(e1, B(0)).unwrap(); // e1 leaves the query
        assert_eq!(collect(&mut q, &w, Tick::new(0)), BTreeSet::from([e2]));
        assert_eq!(q.matched_archetypes().count(), 2);
    }

    #[test]
    fn data_pointers_and_optionals() {
        let mut w = World::new();
        let (a, b, _) = ids(&mut w);
        let e1 = w.spawn();
        let e2 = w.spawn();
        w.insert(e1, A(1)).unwrap();
        w.insert(e2, A(2)).unwrap();
        w.insert(e2, B(20)).unwrap();
        let mut q = QueryState::new(QueryDesc {
            read: vec![a],
            optional: vec![b],
            ..Default::default()
        });
        let mut seen = HashMap::new();
        q.for_each_chunk(&w, Tick::new(0), |chunk| {
            // SAFETY: term 0 is A.
            let a = unsafe { chunk.slice::<A>(0) };
            let b = chunk.ptr(1).map(|_| {
                // SAFETY: term 1 is B and present.
                unsafe { chunk.slice::<B>(1) }
            });
            for (i, &e) in chunk.entities().iter().enumerate() {
                seen.insert(e, (a[i].0, b.map(|b| b[i].0)));
            }
        });
        assert_eq!(seen[&e1], (1, None));
        assert_eq!(seen[&e2], (2, Some(20)));
    }

    #[test]
    fn writes_stamp_changed_and_filters_see_them() {
        let mut w = World::new();
        let (a, b, _) = ids(&mut w);
        let es: Vec<_> = (0..4).map(|_| w.spawn()).collect();
        for &e in &es {
            w.insert(e, A(0)).unwrap();
            w.insert(e, B(0)).unwrap();
        }
        let start = w.change_tick();

        // Next "system run": only write to A on all rows.
        w.increment_change_tick();
        let mut writer = QueryState::new(QueryDesc {
            write: vec![a],
            ..Default::default()
        });
        writer.for_each_chunk_mut(&mut w, start, |mut chunk| {
            // SAFETY: term 0 is a write term of type A.
            for x in unsafe { chunk.slice_mut::<A>(0) } {
                x.0 += 1;
            }
        });
        assert!(es.iter().all(|&e| w.get::<A>(e) == Some(&A(1))));

        let filtered = |c| QueryDesc {
            with: vec![b],
            changed: vec![c],
            ..Default::default()
        };
        let mut qa = QueryState::new(filtered(a));
        let mut qb = QueryState::new(filtered(b));
        assert_eq!(collect(&mut qa, &w, start).len(), 4);
        assert_eq!(collect(&mut qb, &w, start).len(), 0);

        // Single-row writes via get_mut split the matching rows into runs.
        w.increment_change_tick();
        let after = w.change_tick();
        w.increment_change_tick();
        w.get_mut::<B>(es[1]).unwrap().0 = 9;
        let mut runs = Vec::new();
        qb.for_each_chunk(&w, after, |chunk| {
            runs.push(chunk.entities().to_vec())
        });
        assert_eq!(runs, vec![vec![es[1]]]);
    }

    #[test]
    fn added_filter_selects_new_rows() {
        let mut w = World::new();
        let (a, ..) = ids(&mut w);
        let old = w.spawn();
        w.insert(old, A(0)).unwrap();
        let last = w.change_tick();
        w.increment_change_tick();
        let new = w.spawn();
        w.insert(new, A(0)).unwrap();
        let mut q = QueryState::new(QueryDesc {
            added: vec![a],
            ..Default::default()
        });
        assert_eq!(collect(&mut q, &w, last), BTreeSet::from([new]));
        assert_eq!(collect(&mut q, &w, Tick::new(0)).len(), 2);
    }

    #[test]
    #[should_panic(expected = "write terms")]
    fn read_only_entry_rejects_writes() {
        let mut w = World::new();
        let (a, ..) = ids(&mut w);
        let mut q = QueryState::new(QueryDesc {
            write: vec![a],
            ..Default::default()
        });
        q.for_each_chunk(&w, Tick::new(0), |_| {});
    }

    #[derive(Debug, Clone)]
    enum Op {
        Spawn,
        Despawn(usize),
        Toggle(usize, u8),
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            3 => Just(Op::Spawn),
            1 => any::<usize>().prop_map(Op::Despawn),
            6 => (any::<usize>(), 0u8..3).prop_map(|(i, c)| Op::Toggle(i, c)),
        ]
    }

    proptest! {
        /// One long-lived query must always agree with a naive scan, no
        /// matter when archetypes appear relative to its updates.
        #[test]
        fn matches_naive_scan(
            ops in proptest::collection::vec(op(), 0..120),
            kind in 0usize..4,
        ) {
            let mut w = World::new();
            let (a, b, t) = ids(&mut w);
            let desc = match kind {
                0 => QueryDesc { read: vec![a], ..Default::default() },
                1 => QueryDesc { read: vec![a], without: vec![b], ..Default::default() },
                2 => QueryDesc { with: vec![t], optional: vec![b], ..Default::default() },
                _ => QueryDesc { read: vec![a, b], without: vec![t], ..Default::default() },
            };
            let mut q = QueryState::new(desc);
            let mut all: Vec<Entity> = Vec::new();
            for op in ops {
                match op {
                    Op::Spawn => all.push(w.spawn()),
                    Op::Despawn(i) if !all.is_empty() => {
                        let e = all.swap_remove(i % all.len());
                        w.despawn(e).unwrap();
                    }
                    Op::Toggle(i, c) if !all.is_empty() => {
                        let e = all[i % all.len()];
                        match c {
                            0 => if w.has::<A>(e) { w.remove::<A>(e).unwrap(); } else { w.insert(e, A(0)).unwrap(); },
                            1 => if w.has::<B>(e) { w.remove::<B>(e).unwrap(); } else { w.insert(e, B(0)).unwrap(); },
                            _ => if w.has::<Tag>(e) { w.remove::<Tag>(e).unwrap(); } else { w.insert(e, Tag).unwrap(); },
                        }
                    }
                    _ => {}
                }
                let expect: BTreeSet<Entity> = all.iter().copied().filter(|&e| {
                    let (ha, hb, ht) = (w.has::<A>(e), w.has::<B>(e), w.has::<Tag>(e));
                    match kind {
                        0 => ha,
                        1 => ha && !hb,
                        2 => ht,
                        _ => ha && hb && !ht,
                    }
                }).collect();
                prop_assert_eq!(collect(&mut q, &w, Tick::new(0)), expect);
            }
        }
    }
}
