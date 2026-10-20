use std::{
    any::{TypeId, type_name},
    marker::PhantomData,
    ops::{Deref, DerefMut},
};

use crate::{
    component::ComponentId,
    observer::EventId,
    query::QueryDesc,
    tick::Tick,
    world::{World, WorldCell},
};

/// The components and resources a system reads and writes. Declared from its
/// parameters so a scheduler can later run non-conflicting systems in
/// parallel; today it only guards against aliasing inside one system.
#[derive(Clone, Debug, Default)]
pub struct Access {
    reads: Vec<ComponentId>,
    writes: Vec<ComponentId>,
    resource_reads: Vec<TypeId>,
    resource_writes: Vec<TypeId>,
    trigger: Option<EventId>,
}

impl Access {
    pub fn component_reads(&self) -> &[ComponentId] {
        &self.reads
    }

    pub fn component_writes(&self) -> &[ComponentId] {
        &self.writes
    }

    /// # Panics
    /// If a write of `c` is already declared.
    pub fn add_component_read(&mut self, c: ComponentId) {
        assert!(
            !self.writes.contains(&c),
            "conflicting access: component {c:?} is read and written by \
             parameters of the same system"
        );
        self.reads.push(c);
    }

    /// # Panics
    /// If a read or write of `c` is already declared.
    pub fn add_component_write(&mut self, c: ComponentId) {
        assert!(
            !self.writes.contains(&c) && !self.reads.contains(&c),
            "conflicting access: component {c:?} is written by a parameter \
             and accessed by another in the same system"
        );
        self.writes.push(c);
    }

    /// The event type this system observes, if it has a `Trigger` parameter.
    pub fn trigger_event(&self) -> Option<EventId> {
        self.trigger
    }

    pub fn set_trigger_event(&mut self, event: EventId) {
        self.trigger = Some(event);
    }

    pub fn add_query(&mut self, desc: &QueryDesc) {
        for &c in desc.read.iter().chain(&desc.optional) {
            self.add_component_read(c);
        }
        for &c in &desc.write {
            self.add_component_write(c);
        }
    }

    pub fn add_resource_read<T: 'static>(&mut self) {
        let id = TypeId::of::<T>();
        assert!(
            !self.resource_writes.contains(&id),
            "conflicting access: resource {} is read and written in the \
             same system",
            type_name::<T>()
        );
        self.resource_reads.push(id);
    }

    pub fn add_resource_write<T: 'static>(&mut self) {
        let id = TypeId::of::<T>();
        assert!(
            !self.resource_writes.contains(&id)
                && !self.resource_reads.contains(&id),
            "conflicting access: resource {} is written and accessed in the \
             same system",
            type_name::<T>()
        );
        self.resource_writes.push(id);
    }

    /// Whether running both at once could alias.
    pub fn conflicts_with(&self, other: &Access) -> bool {
        let overlap = |a: &[ComponentId], b: &[ComponentId]| {
            a.iter().any(|c| b.contains(c))
        };
        let res_overlap =
            |a: &[TypeId], b: &[TypeId]| a.iter().any(|c| b.contains(c));
        overlap(&self.writes, &other.writes)
            || overlap(&self.writes, &other.reads)
            || overlap(&self.reads, &other.writes)
            || res_overlap(&self.resource_writes, &other.resource_writes)
            || res_overlap(&self.resource_writes, &other.resource_reads)
            || res_overlap(&self.resource_reads, &other.resource_writes)
    }
}

/// A function-system argument: `Query`, `Res`, `ResMut`, `Commands`, `Local`.
///
/// # Safety
/// `init_state` must declare all access `get_param` hands out.
pub unsafe trait SystemParam: Sized {
    type State: 'static;
    /// The same parameter with concrete world and state lifetimes.
    type Item<'w, 's>: SystemParam<State = Self::State>;

    fn init_state(world: &mut World, access: &mut Access) -> Self::State;

    /// # Safety
    /// The cell must be valid for `'w`, and no other code may touch what
    /// `init_state` declared for the duration of the item.
    unsafe fn get_param<'w, 's>(
        state: &'s mut Self::State,
        last_run: Tick,
        world: WorldCell<'w>,
    ) -> Self::Item<'w, 's>;

    /// Runs at the sync point after the stage, with full world access.
    fn apply(_state: &mut Self::State, _world: &mut World) {}
}

pub type SystemParamItem<'w, 's, P> = <P as SystemParam>::Item<'w, 's>;

macro_rules! impl_system_param_tuple {
    ($(($P:ident, $p:ident)),*) => {
        #[allow(unused_variables, clippy::unused_unit)]
        unsafe impl<$($P: SystemParam),*> SystemParam for ($($P,)*) {
            type State = ($($P::State,)*);
            type Item<'w, 's> = ($($P::Item<'w, 's>,)*);

            fn init_state(world: &mut World, access: &mut Access)
                -> Self::State
            {
                ($($P::init_state(world, access),)*)
            }

            unsafe fn get_param<'w, 's>(
                state: &'s mut Self::State,
                last_run: Tick,
                world: WorldCell<'w>,
            ) -> Self::Item<'w, 's> {
                let ($($p,)*) = state;
                // SAFETY: forwarded; each parameter declared its access.
                ($(unsafe { $P::get_param($p, last_run, world) },)*)
            }

            fn apply(state: &mut Self::State, world: &mut World) {
                let ($($p,)*) = state;
                $($P::apply($p, world);)*
            }
        }
    };
}

impl_system_param_tuple!();
impl_system_param_tuple!((A, a));
impl_system_param_tuple!((A, a), (B, b));
impl_system_param_tuple!((A, a), (B, b), (C, c));
impl_system_param_tuple!((A, a), (B, b), (C, c), (D, d));
impl_system_param_tuple!((A, a), (B, b), (C, c), (D, d), (E, e));
impl_system_param_tuple!((A, a), (B, b), (C, c), (D, d), (E, e), (F, f));
impl_system_param_tuple!(
    (A, a),
    (B, b),
    (C, c),
    (D, d),
    (E, e),
    (F, f),
    (G, g)
);
impl_system_param_tuple!(
    (A, a),
    (B, b),
    (C, c),
    (D, d),
    (E, e),
    (F, f),
    (G, g),
    (H, h)
);

/// Shared access to the resource `T`.
pub struct Res<'w, T: Send + Sync + 'static> {
    value: &'w T,
}

impl<T: Send + Sync + 'static> Deref for Res<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.value
    }
}

unsafe impl<T: Send + Sync + 'static> SystemParam for Res<'_, T> {
    type State = ();
    type Item<'w, 's> = Res<'w, T>;

    fn init_state(_: &mut World, access: &mut Access) {
        access.add_resource_read::<T>();
    }

    unsafe fn get_param<'w>(
        _: &mut (),
        _: Tick,
        world: WorldCell<'w>,
    ) -> Res<'w, T> {
        // SAFETY: read access was declared.
        let value = unsafe { world.resource::<T>() }.unwrap_or_else(|| {
            panic!("resource {} not found", type_name::<T>())
        });
        Res { value }
    }
}

/// Exclusive access to the resource `T`.
pub struct ResMut<'w, T: Send + Sync + 'static> {
    value: &'w mut T,
}

impl<T: Send + Sync + 'static> Deref for ResMut<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.value
    }
}

impl<T: Send + Sync + 'static> DerefMut for ResMut<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.value
    }
}

unsafe impl<T: Send + Sync + 'static> SystemParam for ResMut<'_, T> {
    type State = ();
    type Item<'w, 's> = ResMut<'w, T>;

    fn init_state(_: &mut World, access: &mut Access) {
        access.add_resource_write::<T>();
    }

    unsafe fn get_param<'w>(
        _: &mut (),
        _: Tick,
        world: WorldCell<'w>,
    ) -> ResMut<'w, T> {
        // SAFETY: write access was declared.
        let value = unsafe { world.resource_mut::<T>() }.unwrap_or_else(|| {
            panic!("resource {} not found", type_name::<T>())
        });
        ResMut { value }
    }
}

/// State private to one system, kept across its runs.
pub struct Local<'s, T: Default + Send + Sync + 'static> {
    value: &'s mut T,
}

impl<T: Default + Send + Sync + 'static> Deref for Local<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.value
    }
}

impl<T: Default + Send + Sync + 'static> DerefMut for Local<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.value
    }
}

unsafe impl<T: Default + Send + Sync + 'static> SystemParam for Local<'_, T> {
    type State = T;
    type Item<'w, 's> = Local<'s, T>;

    fn init_state(_: &mut World, _: &mut Access) -> T {
        T::default()
    }

    unsafe fn get_param<'w, 's>(
        state: &'s mut T,
        _: Tick,
        _: WorldCell<'w>,
    ) -> Local<'s, T> {
        Local { value: state }
    }
}

/// A unit of work run by the schedule.
pub trait System: 'static {
    fn name(&self) -> &str;
    /// For recognizing a system's concrete type, e.g. to remove all the
    /// systems of a script assembly that is being reloaded.
    fn as_any(&self) -> &dyn std::any::Any;
    fn access(&self) -> &Access;
    /// Idempotent; resolves parameter state against the world.
    fn initialize(&mut self, world: &mut World);
    /// Runs once, then advances the world's change tick.
    fn run(&mut self, world: &mut World);
    /// Applies deferred work (commands) at a sync point.
    fn apply_deferred(&mut self, world: &mut World);
    /// Clamps the system's remembered `last_run`, see
    /// [`World::check_change_ticks`].
    fn check_change_tick(&mut self, now: Tick);
}

pub trait IntoSystem<Marker> {
    type System: System;

    fn into_system(self) -> Self::System;
}

/// A function whose arguments are all [`SystemParam`]s.
pub trait SystemParamFunction<Marker>: 'static {
    type Param: SystemParam;

    fn run(&mut self, param: SystemParamItem<'_, '_, Self::Param>);
}

macro_rules! impl_system_function {
    ($(($P:ident, $p:ident)),*) => {
        impl<Func, $($P: SystemParam),*> SystemParamFunction<fn($($P,)*)>
            for Func
        where
            Func: 'static,
            for<'a> &'a mut Func:
                FnMut($($P),*) + FnMut($(SystemParamItem<$P>),*),
        {
            type Param = ($($P,)*);

            #[allow(non_snake_case, clippy::too_many_arguments)]
            fn run(&mut self, param: SystemParamItem<'_, '_, Self::Param>) {
                fn call_inner<$($P),*>(
                    mut f: impl FnMut($($P),*),
                    $($p: $P),*
                ) {
                    f($($p),*)
                }
                let ($($p,)*) = param;
                call_inner(self, $($p),*)
            }
        }
    };
}

impl_system_function!();
impl_system_function!((A, a));
impl_system_function!((A, a), (B, b));
impl_system_function!((A, a), (B, b), (C, c));
impl_system_function!((A, a), (B, b), (C, c), (D, d));
impl_system_function!((A, a), (B, b), (C, c), (D, d), (E, e));
impl_system_function!((A, a), (B, b), (C, c), (D, d), (E, e), (F, f));
impl_system_function!((A, a), (B, b), (C, c), (D, d), (E, e), (F, f), (G, g));
impl_system_function!(
    (A, a),
    (B, b),
    (C, c),
    (D, d),
    (E, e),
    (F, f),
    (G, g),
    (H, h)
);

pub struct FunctionSystem<Marker, F: SystemParamFunction<Marker>> {
    func: F,
    state: Option<<F::Param as SystemParam>::State>,
    access: Access,
    last_run: Tick,
    _marker: PhantomData<fn() -> Marker>,
}

impl<Marker: 'static, F: SystemParamFunction<Marker>> System
    for FunctionSystem<Marker, F>
{
    fn name(&self) -> &str {
        type_name::<F>()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn access(&self) -> &Access {
        &self.access
    }

    fn initialize(&mut self, world: &mut World) {
        if self.state.is_none() {
            let mut access = Access::default();
            self.state = Some(F::Param::init_state(world, &mut access));
            self.access = access;
        }
    }

    fn run(&mut self, world: &mut World) {
        self.initialize(world);
        let this_run = world.change_tick();
        let state = self.state.as_mut().expect("initialized above");
        let cell = WorldCell::new(world);
        // SAFETY: `world` is exclusively borrowed for this call, and the
        // parameters' access sets were checked for conflicts in
        // `initialize`.
        let param = unsafe { F::Param::get_param(state, self.last_run, cell) };
        self.func.run(param);
        self.last_run = this_run;
        world.increment_change_tick();
    }

    fn check_change_tick(&mut self, now: Tick) {
        self.last_run.check_tick(now);
    }

    fn apply_deferred(&mut self, world: &mut World) {
        if let Some(state) = self.state.as_mut() {
            F::Param::apply(state, world);
        }
    }
}

impl<Marker: 'static, F: SystemParamFunction<Marker>> IntoSystem<Marker> for F {
    type System = FunctionSystem<Marker, F>;

    fn into_system(self) -> Self::System {
        FunctionSystem {
            func: self,
            state: None,
            access: Access::default(),
            last_run: Tick::new(0),
            _marker: PhantomData,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tick::MAX_CHANGE_AGE;

    #[test]
    fn check_change_tick_clamps_last_run() {
        fn noop() {}
        let mut world = World::new();
        let mut system = IntoSystem::into_system(noop);
        system.run(&mut world);
        assert_eq!(system.last_run, Tick::new(1));

        let now = Tick::new(MAX_CHANGE_AGE + 100);
        system.check_change_tick(now);
        assert_eq!(now.relative_to(system.last_run).get(), MAX_CHANGE_AGE);
    }
}
