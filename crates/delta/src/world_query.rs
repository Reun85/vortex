//! The typed Rust query layer: a thin wrapper that builds a [`QueryDesc`]
//! from types and hands out references from [`Chunk`]s.

use std::marker::PhantomData;

use crate::{
    component::ComponentId,
    entity::Entity,
    query::{Chunk, Cursor, QueryDesc, QueryState},
    system::{Access, SystemParam},
    tick::Tick,
    world::{World, WorldCell},
};

/// Something a [`Query`] can fetch per entity: `&T`, `&mut T`, `Option<&T>`,
/// [`Entity`], or tuples of those.
///
/// # Safety
/// `update_desc` must declare exactly the access `fetch` performs.
pub unsafe trait WorldQuery {
    type State: 'static;
    type Fetch<'w>;
    type Item<'w>;

    fn init_state(world: &mut World) -> Self::State;

    /// Adds this query's terms to the description.
    fn update_desc(state: &Self::State, desc: &mut QueryDesc);

    /// # Safety
    /// `chunk` must come from a query built from a description that all
    /// involved states added their terms to.
    unsafe fn init_fetch<'w>(
        state: &Self::State,
        desc: &QueryDesc,
        chunk: &Chunk<'w>,
    ) -> Self::Fetch<'w>;

    /// # Safety
    /// `row` is within the chunk the fetch was initialized for, and the
    /// caller never creates aliasing references (each row is fetched once
    /// per fetch).
    unsafe fn fetch<'w>(
        fetch: &mut Self::Fetch<'w>,
        row: usize,
    ) -> Self::Item<'w>;
}

/// A [`WorldQuery`] that never writes.
///
/// # Safety
/// Implementors must not declare write terms.
pub unsafe trait ReadOnlyWorldQuery: WorldQuery {}

unsafe impl<T: Send + Sync + 'static> WorldQuery for &T {
    type State = ComponentId;
    type Fetch<'w> = *const T;
    type Item<'w> = &'w T;

    fn init_state(world: &mut World) -> ComponentId {
        world.component_id::<T>()
    }

    fn update_desc(state: &ComponentId, desc: &mut QueryDesc) {
        desc.read.push(*state);
    }

    unsafe fn init_fetch<'w>(
        state: &ComponentId,
        desc: &QueryDesc,
        chunk: &Chunk<'w>,
    ) -> *const T {
        let term = desc.read.iter().position(|c| c == state).unwrap();
        chunk
            .ptr(term)
            .expect("required term")
            .cast::<T>()
            .cast_const()
    }

    unsafe fn fetch<'w>(fetch: &mut *const T, row: usize) -> &'w T {
        // SAFETY: per the trait contract the row is in bounds.
        unsafe { &*fetch.add(row) }
    }
}

unsafe impl<T: Send + Sync + 'static> ReadOnlyWorldQuery for &T {}

unsafe impl<T: Send + Sync + 'static> WorldQuery for &mut T {
    type State = ComponentId;
    type Fetch<'w> = *mut T;
    type Item<'w> = &'w mut T;

    fn init_state(world: &mut World) -> ComponentId {
        world.component_id::<T>()
    }

    fn update_desc(state: &ComponentId, desc: &mut QueryDesc) {
        desc.write.push(*state);
    }

    unsafe fn init_fetch<'w>(
        state: &ComponentId,
        desc: &QueryDesc,
        chunk: &Chunk<'w>,
    ) -> *mut T {
        let pos = desc.write.iter().position(|c| c == state).unwrap();
        chunk
            .ptr(desc.read.len() + pos)
            .expect("required term")
            .cast::<T>()
    }

    unsafe fn fetch<'w>(fetch: &mut *mut T, row: usize) -> &'w mut T {
        // SAFETY: per the trait contract the row is in bounds and unique.
        unsafe { &mut *fetch.add(row) }
    }
}

unsafe impl<T: Send + Sync + 'static> WorldQuery for Option<&T> {
    type State = ComponentId;
    type Fetch<'w> = Option<*const T>;
    type Item<'w> = Option<&'w T>;

    fn init_state(world: &mut World) -> ComponentId {
        world.component_id::<T>()
    }

    fn update_desc(state: &ComponentId, desc: &mut QueryDesc) {
        desc.optional.push(*state);
    }

    unsafe fn init_fetch<'w>(
        state: &ComponentId,
        desc: &QueryDesc,
        chunk: &Chunk<'w>,
    ) -> Option<*const T> {
        let pos = desc.optional.iter().position(|c| c == state).unwrap();
        let term = desc.read.len() + desc.write.len() + pos;
        chunk.ptr(term).map(|p| p.cast::<T>().cast_const())
    }

    unsafe fn fetch<'w>(
        fetch: &mut Option<*const T>,
        row: usize,
    ) -> Option<&'w T> {
        // SAFETY: per the trait contract the row is in bounds.
        fetch.map(|p| unsafe { &*p.add(row) })
    }
}

unsafe impl<T: Send + Sync + 'static> ReadOnlyWorldQuery for Option<&T> {}

unsafe impl WorldQuery for Entity {
    type State = ();
    type Fetch<'w> = &'w [Entity];
    type Item<'w> = Entity;

    fn init_state(_: &mut World) {}

    fn update_desc(_: &(), _: &mut QueryDesc) {}

    unsafe fn init_fetch<'w>(
        _: &(),
        _: &QueryDesc,
        chunk: &Chunk<'w>,
    ) -> &'w [Entity] {
        chunk.entities()
    }

    unsafe fn fetch<'w>(
        fetch: &mut <Self as WorldQuery>::Fetch<'w>,
        row: usize,
    ) -> <Self as WorldQuery>::Item<'w> {
        fetch[row]
    }
}

unsafe impl ReadOnlyWorldQuery for Entity {}

macro_rules! impl_world_query_tuple {
    ($(($T:ident, $t:ident)),*) => {
        #[allow(unused_variables, clippy::unused_unit)]
        unsafe impl<$($T: WorldQuery),*> WorldQuery for ($($T,)*) {
            type State = ($($T::State,)*);
            type Fetch<'w> = ($($T::Fetch<'w>,)*);
            type Item<'w> = ($($T::Item<'w>,)*);

            fn init_state(world: &mut World) -> Self::State {
                ($($T::init_state(world),)*)
            }

            fn update_desc(state: &Self::State, desc: &mut QueryDesc) {
                let ($($t,)*) = state;
                $($T::update_desc($t, desc);)*
            }

            unsafe fn init_fetch<'w>(
                state: &Self::State,
                desc: &QueryDesc,
                chunk: &Chunk<'w>,
            ) -> Self::Fetch<'w> {
                let ($($t,)*) = state;
                // SAFETY: forwarded contract.
                ($(unsafe { $T::init_fetch($t, desc, chunk) },)*)
            }

            unsafe fn fetch<'w>(
                fetch: &mut Self::Fetch<'w>,
                row: usize,
            ) -> Self::Item<'w> {
                let ($($t,)*) = fetch;
                // SAFETY: forwarded contract.
                ($(unsafe { $T::fetch($t, row) },)*)
            }
        }

        unsafe impl<$($T: ReadOnlyWorldQuery),*> ReadOnlyWorldQuery
            for ($($T,)*) {}
    };
}

impl_world_query_tuple!((A, a));
impl_world_query_tuple!((A, a), (B, b));
impl_world_query_tuple!((A, a), (B, b), (C, c));
impl_world_query_tuple!((A, a), (B, b), (C, c), (D, d));
impl_world_query_tuple!((A, a), (B, b), (C, c), (D, d), (E, e));
impl_world_query_tuple!((A, a), (B, b), (C, c), (D, d), (E, e), (F, f));
impl_world_query_tuple!((A, a), (B, b), (C, c), (D, d), (E, e), (F, f), (G, g));
impl_world_query_tuple!(
    (A, a),
    (B, b),
    (C, c),
    (D, d),
    (E, e),
    (F, f),
    (G, g),
    (H, h)
);

/// Extra constraints on which entities a [`Query`] matches.
pub trait QueryFilter {
    type State: 'static;

    fn init_state(world: &mut World) -> Self::State;
    fn update_desc(state: &Self::State, desc: &mut QueryDesc);
}

/// Matches entities that have `T`, without fetching it.
pub struct With<T>(PhantomData<fn() -> T>);
/// Matches entities that do not have `T`.
pub struct Without<T>(PhantomData<fn() -> T>);
/// Matches rows where `T` was added since the system last ran.
pub struct Added<T>(PhantomData<fn() -> T>);
/// Matches rows where `T` was written since the system last ran.
pub struct Changed<T>(PhantomData<fn() -> T>);

macro_rules! impl_marker_filter {
    ($name:ident, $field:ident) => {
        impl<T: Send + Sync + 'static> QueryFilter for $name<T> {
            type State = ComponentId;

            fn init_state(world: &mut World) -> ComponentId {
                world.component_id::<T>()
            }

            fn update_desc(state: &ComponentId, desc: &mut QueryDesc) {
                desc.$field.push(*state);
            }
        }
    };
}

impl_marker_filter!(With, with);
impl_marker_filter!(Without, without);
impl_marker_filter!(Added, added);
impl_marker_filter!(Changed, changed);

macro_rules! impl_filter_tuple {
    ($(($T:ident, $t:ident)),*) => {
        #[allow(unused_variables, clippy::unused_unit)]
        impl<$($T: QueryFilter),*> QueryFilter for ($($T,)*) {
            type State = ($($T::State,)*);

            fn init_state(world: &mut World) -> Self::State {
                ($($T::init_state(world),)*)
            }

            fn update_desc(state: &Self::State, desc: &mut QueryDesc) {
                let ($($t,)*) = state;
                $($T::update_desc($t, desc);)*
            }
        }
    };
}

impl_filter_tuple!();
impl_filter_tuple!((A, a));
impl_filter_tuple!((A, a), (B, b));
impl_filter_tuple!((A, a), (B, b), (C, c));
impl_filter_tuple!((A, a), (B, b), (C, c), (D, d));

/// Cached state behind a [`Query`] system parameter.
pub struct QueryParamState<D: WorldQuery, F: QueryFilter> {
    query: QueryState,
    fetch: D::State,
    /// Kept alive so the filter's component ids stay registered.
    _filter: F::State,
}

impl<D: WorldQuery, F: QueryFilter> QueryParamState<D, F> {
    pub(crate) fn new(world: &mut World) -> Self {
        let fetch = D::init_state(world);
        let filter = F::init_state(world);
        let mut desc = QueryDesc::default();
        D::update_desc(&fetch, &mut desc);
        F::update_desc(&filter, &mut desc);
        Self {
            query: QueryState::new(desc),
            fetch,
            _filter: filter,
        }
    }

    pub(crate) fn desc(&self) -> &QueryDesc {
        self.query.desc()
    }
}

/// Typed access to every entity matching `D` and `F`.
///
/// `Query<(&Transform, &mut Velocity), (With<Player>, Without<Frozen>)>`
pub struct Query<'w, 's, D: WorldQuery, F: QueryFilter = ()> {
    world: WorldCell<'w>,
    state: &'s QueryParamState<D, F>,
    last_run: Tick,
}

impl<D: WorldQuery, F: QueryFilter> Query<'_, '_, D, F> {
    fn make_iter(&self, stamp: bool) -> QueryIter<'_, D, F> {
        QueryIter {
            world: self.world,
            state: self.state,
            cursor: Cursor::default(),
            last_run: self.last_run,
            stamp,
            fetch: None,
            row: 0,
            len: 0,
        }
    }

    /// # Safety
    /// Stamping (`stamp`) requires the exclusive access `iter_mut` has.
    unsafe fn get_inner(
        &self,
        entity: Entity,
        stamp: bool,
    ) -> Option<D::Item<'_>> {
        // SAFETY: the entity allocator is only read during a system run.
        let loc = unsafe { self.world.entities() }.location(entity)?;
        // SAFETY: the query is up to date, and the caller's access is
        // guaranteed by the system's declared access set.
        let chunk = unsafe {
            self.state
                .query
                .chunk_at(self.world, loc, self.last_run, stamp)
        }?;
        // SAFETY: the chunk comes from this query; row 0 of a 1-row chunk.
        unsafe {
            let mut fetch = D::init_fetch(
                &self.state.fetch,
                self.state.query.desc(),
                &chunk,
            );
            Some(D::fetch(&mut fetch, 0))
        }
    }

    /// Mutable iteration. Write columns are stamped changed.
    pub fn iter_mut(&mut self) -> QueryIter<'_, D, F> {
        self.make_iter(true)
    }

    /// The item for `entity`, if it is alive and matches.
    pub fn get_mut(&mut self, entity: Entity) -> Option<D::Item<'_>> {
        // SAFETY: `&mut self` is exclusive over this query's access.
        unsafe { self.get_inner(entity, true) }
    }

    /// Whether `entity` matches the query.
    pub fn contains(&self, entity: Entity) -> bool {
        // SAFETY: only reads the allocator and metadata.
        let Some(loc) = (unsafe { self.world.entities() }).location(entity)
        else {
            return false;
        };
        // SAFETY: no stamping, nothing is fetched.
        unsafe {
            self.state
                .query
                .chunk_at(self.world, loc, self.last_run, false)
                .is_some()
        }
    }
}

impl<D: ReadOnlyWorldQuery, F: QueryFilter> Query<'_, '_, D, F> {
    pub fn iter(&self) -> QueryIter<'_, D, F> {
        self.make_iter(false)
    }

    pub fn get(&self, entity: Entity) -> Option<D::Item<'_>> {
        // SAFETY: read-only query, nothing is stamped.
        unsafe { self.get_inner(entity, false) }
    }
}

impl<'a, D: ReadOnlyWorldQuery, F: QueryFilter> IntoIterator
    for &'a Query<'_, '_, D, F>
{
    type Item = D::Item<'a>;
    type IntoIter = QueryIter<'a, D, F>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, D: WorldQuery, F: QueryFilter> IntoIterator
    for &'a mut Query<'_, '_, D, F>
{
    type Item = D::Item<'a>;
    type IntoIter = QueryIter<'a, D, F>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

/// Iterator over the items of a [`Query`], chunk by chunk.
pub struct QueryIter<'a, D: WorldQuery, F: QueryFilter> {
    world: WorldCell<'a>,
    state: &'a QueryParamState<D, F>,
    cursor: Cursor,
    last_run: Tick,
    stamp: bool,
    fetch: Option<D::Fetch<'a>>,
    row: usize,
    len: usize,
}

impl<'a, D: WorldQuery, F: QueryFilter> Iterator for QueryIter<'a, D, F> {
    type Item = D::Item<'a>;

    fn next(&mut self) -> Option<D::Item<'a>> {
        loop {
            if self.row < self.len {
                let fetch = self.fetch.as_mut().expect("chunk is loaded");
                // SAFETY: row < len of the loaded chunk, fetched once.
                let item = unsafe { D::fetch(fetch, self.row) };
                self.row += 1;
                return Some(item);
            }
            // SAFETY: the parameter's access was declared and checked, so
            // this iterator has the access it needs; the previous chunk's
            // pointers are raw and unaffected by stamping.
            let chunk = unsafe {
                self.state.query.next_chunk(
                    self.world,
                    &mut self.cursor,
                    self.last_run,
                    self.stamp,
                )
            }?;
            self.len = chunk.len();
            self.row = 0;
            // SAFETY: the chunk comes from this query.
            self.fetch = Some(unsafe {
                D::init_fetch(
                    &self.state.fetch,
                    self.state.query.desc(),
                    &chunk,
                )
            });
        }
    }
}

// SAFETY: the access declared is the query's description; `get_param`
// hands out the matching items.
unsafe impl<D: WorldQuery + 'static, F: QueryFilter + 'static> SystemParam
    for Query<'_, '_, D, F>
{
    type State = QueryParamState<D, F>;
    type Item<'w, 's> = Query<'w, 's, D, F>;

    fn init_state(world: &mut World, access: &mut Access) -> Self::State {
        let state = QueryParamState::<D, F>::new(world);
        access.add_query(state.desc());
        state
    }

    unsafe fn get_param<'w, 's>(
        state: &'s mut Self::State,
        last_run: Tick,
        world: WorldCell<'w>,
    ) -> Query<'w, 's, D, F> {
        // SAFETY: archetype metadata is only grown by structural changes,
        // which never happen during a system run.
        state.query.update_archetypes(unsafe { world.archetypes() });
        Query {
            world,
            state,
            last_run,
        }
    }
}
