//! Events and observers: entity-targeted or global reactions to emitted
//! events. Observers are ordinary entities with an [`Observer`] component,
//! so they show up in the world like everything else.

use std::{
    any::{TypeId, type_name},
    collections::HashMap,
    ops::Deref,
    ptr,
};

use crate::{
    entity::Entity,
    hooks::DeferredWorld,
    system::{Access, IntoSystem, System, SystemParam},
    tick::Tick,
    world::{World, WorldCell},
};

/// A value that can be emitted and observed.
pub trait Event: Send + Sync + 'static {
    /// Whether the event should bubble from the target to its parents once
    /// hierarchies exist. Carried in every [`EventHeader`] from the start.
    const PROPAGATE: bool = false;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EventId(u32);

/// Runtime registry of event types, by Rust type or by name (for scripts).
#[derive(Default)]
pub struct Events {
    by_type: HashMap<TypeId, EventId>,
    by_name: HashMap<Box<str>, EventId>,
    names: Vec<Box<str>>,
}

impl Events {
    pub fn register<E: Event>(&mut self) -> EventId {
        if let Some(&id) = self.by_type.get(&TypeId::of::<E>()) {
            return id;
        }
        let id = self.register_named(type_name::<E>());
        self.by_type.insert(TypeId::of::<E>(), id);
        id
    }

    /// Registers an event known only by name; the same name gives the same id.
    pub fn register_named(&mut self, name: &str) -> EventId {
        if let Some(&id) = self.by_name.get(name) {
            return id;
        }
        let id = EventId(u32::try_from(self.names.len()).expect("events"));
        self.names.push(name.into());
        self.by_name.insert(name.into(), id);
        id
    }

    pub fn id_by_name(&self, name: &str) -> Option<EventId> {
        self.by_name.get(name).copied()
    }

    pub fn name(&self, id: EventId) -> &str {
        &self.names[id.0 as usize]
    }
}

/// What every delivered event carries besides its payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventHeader {
    pub event: EventId,
    pub target: Option<Entity>,
    pub propagate: bool,
}

/// The event currently being delivered, visible to `Trigger` parameters.
pub(crate) struct TriggerFrame {
    pub(crate) header: EventHeader,
    pub(crate) data: *const u8,
}

// SAFETY: a frame only exists while an event is dispatched on the thread
// that owns the world, and points at a value that outlives the dispatch.
unsafe impl Send for TriggerFrame {}
unsafe impl Sync for TriggerFrame {}

/// The observer's view of the event that fired it. Use as the first
/// parameter of an observer function:
/// `|ev: Trigger<Hit>, mut commands: Commands| { .. }`.
pub struct Trigger<'w, E: Event> {
    event: &'w E,
    header: EventHeader,
}

impl<E: Event> Trigger<'_, E> {
    pub fn event(&self) -> &E {
        self.event
    }

    /// The entity the event was emitted at, if any.
    pub fn target(&self) -> Option<Entity> {
        self.header.target
    }

    pub fn header(&self) -> &EventHeader {
        &self.header
    }
}

impl<E: Event> Deref for Trigger<'_, E> {
    type Target = E;

    fn deref(&self) -> &E {
        self.event
    }
}

// SAFETY: declares no component or resource access; it only reads the frame
// of the event being dispatched.
unsafe impl<E: Event> SystemParam for Trigger<'_, E> {
    type State = EventId;
    type Item<'w, 's> = Trigger<'w, E>;

    fn init_state(world: &mut World, access: &mut Access) -> EventId {
        let id = world.event_id::<E>();
        access.set_trigger_event(id);
        id
    }

    unsafe fn get_param<'w>(
        state: &mut EventId,
        _: Tick,
        world: WorldCell<'w>,
    ) -> Trigger<'w, E> {
        // SAFETY: the trigger stack is only touched by dispatch.
        let frame = unsafe { world.current_trigger() }
            .expect("Trigger parameter used outside of an observer");
        assert_eq!(
            frame.header.event, *state,
            "observer ran for an event it did not register for"
        );
        Trigger {
            // SAFETY: the frame points at the emitted `E` for the whole
            // dispatch, and the id check above proves the type.
            event: unsafe { &*frame.data.cast::<E>() },
            header: frame.header,
        }
    }
}

/// A reaction to an event, stored as a component so observers are entities.
pub struct Observer {
    pub(crate) event: EventId,
    pub(crate) target: Option<Entity>,
    /// Taken out while it runs, so it can borrow the whole world.
    pub(crate) callback: Option<Box<dyn System>>,
}

// SAFETY: observers run only on the thread that owns the world (the plan
// keeps script and observer code on the main thread), never concurrently.
unsafe impl Send for Observer {}
unsafe impl Sync for Observer {}

impl Observer {
    pub fn event(&self) -> EventId {
        self.event
    }

    pub fn target(&self) -> Option<Entity> {
        self.target
    }
}

/// Which observer entities listen to what.
#[derive(Default)]
pub(crate) struct ObserverIndex {
    global: HashMap<EventId, Vec<Entity>>,
    targeted: HashMap<(EventId, Entity), Vec<Entity>>,
    /// Target entity -> observers watching it.
    watching: HashMap<Entity, Vec<Entity>>,
}

fn remove_from<K: std::hash::Hash + Eq>(
    map: &mut HashMap<K, Vec<Entity>>,
    key: &K,
    observer: Entity,
) {
    if let Some(list) = map.get_mut(key) {
        list.retain(|&o| o != observer);
        if list.is_empty() {
            map.remove(key);
        }
    }
}

impl ObserverIndex {
    pub(crate) fn insert(
        &mut self,
        event: EventId,
        target: Option<Entity>,
        observer: Entity,
    ) {
        match target {
            Some(t) => {
                self.targeted.entry((event, t)).or_default().push(observer);
                self.watching.entry(t).or_default().push(observer);
            }
            None => self.global.entry(event).or_default().push(observer),
        }
    }

    pub(crate) fn remove(
        &mut self,
        event: EventId,
        target: Option<Entity>,
        observer: Entity,
    ) {
        match target {
            Some(t) => {
                remove_from(&mut self.targeted, &(event, t), observer);
                remove_from(&mut self.watching, &t, observer);
            }
            None => remove_from(&mut self.global, &event, observer),
        }
    }

    /// Observers to run for an event: those on its target, then global ones.
    pub(crate) fn matching(&self, header: &EventHeader) -> Vec<Entity> {
        let mut out = Vec::new();
        if let Some(t) = header.target
            && let Some(list) = self.targeted.get(&(header.event, t))
        {
            out.extend_from_slice(list);
        }
        if let Some(list) = self.global.get(&header.event) {
            out.extend_from_slice(list);
        }
        out
    }

    pub(crate) fn watchers(&self, target: Entity) -> Vec<Entity> {
        self.watching.get(&target).cloned().unwrap_or_default()
    }
}

fn observer_added(world: DeferredWorld<'_>, e: Entity) {
    let (event, target) = {
        let o = world
            .get::<Observer>(e)
            .expect("hook runs with the component");
        (o.event, o.target)
    };
    world.world.observers.insert(event, target, e);
}

fn observer_removed(world: DeferredWorld<'_>, e: Entity) {
    let (event, target) = {
        let o = world
            .get::<Observer>(e)
            .expect("hook runs with the component");
        (o.event, o.target)
    };
    world.world.observers.remove(event, target, e);
}

impl World {
    pub fn event_id<E: Event>(&mut self) -> EventId {
        self.events.register::<E>()
    }

    pub fn events(&self) -> &Events {
        &self.events
    }

    /// Runs `observer` whenever its event is emitted at `target`. The
    /// observer is a function whose parameters include a `Trigger<E>`.
    /// Returns the observer entity; despawn it to stop listening. It is
    /// removed automatically when `target` is despawned.
    pub fn observe<M>(
        &mut self,
        target: Entity,
        observer: impl IntoSystem<M>,
    ) -> Entity {
        assert!(self.is_alive(target), "observing a dead entity");
        self.spawn_observer(Some(target), Box::new(observer.into_system()))
    }

    /// Like [`observe`](Self::observe) but for events emitted anywhere.
    pub fn observe_global<M>(
        &mut self,
        observer: impl IntoSystem<M>,
    ) -> Entity {
        self.spawn_observer(None, Box::new(observer.into_system()))
    }

    fn spawn_observer(
        &mut self,
        target: Option<Entity>,
        mut system: Box<dyn System>,
    ) -> Entity {
        if !self.observer_hooks_registered {
            self.observer_hooks_registered = true;
            self.register_hooks::<Observer>()
                .on_add(observer_added)
                .on_remove(observer_removed);
        }
        system.initialize(self);
        let event = system
            .access()
            .trigger_event()
            .expect("an observer needs a Trigger<E> parameter");
        let e = self.spawn();
        self.insert(
            e,
            Observer {
                event,
                target,
                callback: Some(system),
            },
        )
        .expect("just spawned");
        e
    }

    /// Emits `event` at `target`. Observers run at the next sync point; when
    /// called outside a system that is before this call returns.
    pub fn emit<E: Event>(&mut self, target: Entity, event: E) {
        self.deferred
            .push(move |w| w.trigger_or_defer(Some(target), event));
        self.finish_op();
    }

    pub fn emit_global<E: Event>(&mut self, event: E) {
        self.deferred.push(move |w| w.trigger_or_defer(None, event));
        self.finish_op();
    }

    /// Delivers now, unless already delivering: then it waits its turn in the
    /// deferred buffer so observers never nest.
    pub(crate) fn trigger_or_defer<E: Event>(
        &mut self,
        target: Option<Entity>,
        event: E,
    ) {
        if self.dispatching > 0 {
            self.deferred
                .push(move |w| w.trigger_or_defer(target, event));
            return;
        }
        let header = EventHeader {
            event: self.event_id::<E>(),
            target,
            propagate: E::PROPAGATE,
        };
        self.dispatching += 1;
        self.trigger_stack.push(TriggerFrame {
            header,
            data: ptr::from_ref(&event).cast(),
        });
        for observer in self.observers.matching(&header) {
            self.run_observer(observer);
        }
        self.trigger_stack.pop();
        self.dispatching -= 1;
    }

    fn run_observer(&mut self, observer: Entity) {
        let mut callback = match self.get_mut::<Observer>(observer) {
            Some(mut component) => {
                component.bypass_change_detection().callback.take()
            }
            // Despawned by an earlier observer of the same event.
            None => return,
        };
        if let Some(system) = callback.as_mut() {
            system.run(self);
            system.apply_deferred(self);
        }
        if let Some(mut component) = self.get_mut::<Observer>(observer) {
            component.bypass_change_detection().callback = callback;
        }
    }
}
