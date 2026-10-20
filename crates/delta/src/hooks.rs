use std::fmt;

use crate::{
    change::Mut,
    commands::Commands,
    component::Components,
    entity::Entity,
    observer::Event,
    world::World,
};

/// A component lifecycle callback. It receives restricted world access:
/// structural changes must go through [`DeferredWorld::commands`], and are
/// applied before the current sync point ends.
pub type Hook = fn(DeferredWorld<'_>, Entity);

/// Lifecycle hooks for one component type, owned by that component's author.
///
/// - `on_add`: the component was added to an entity that lacked it.
/// - `on_insert`: the component was added or overwritten.
/// - `on_replace`: the component is about to be overwritten or removed; its
///   old value is still readable.
/// - `on_remove`: the component is about to be removed (by `remove` or by
///   despawning); its value is still readable.
///
/// When several components are inserted at once, every value is in place
/// before any `on_add`/`on_insert` runs.
#[derive(Clone, Copy, Default)]
pub struct ComponentHooks {
    pub on_add: Option<Hook>,
    pub on_insert: Option<Hook>,
    pub on_replace: Option<Hook>,
    pub on_remove: Option<Hook>,
}

impl fmt::Debug for ComponentHooks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ComponentHooks")
            .field("on_add", &self.on_add.is_some())
            .field("on_insert", &self.on_insert.is_some())
            .field("on_replace", &self.on_replace.is_some())
            .field("on_remove", &self.on_remove.is_some())
            .finish()
    }
}

/// World access for hooks: reads and in-place writes are direct, structural
/// changes and events are deferred.
pub struct DeferredWorld<'w> {
    pub(crate) world: &'w mut World,
}

impl DeferredWorld<'_> {
    pub fn is_alive(&self, e: Entity) -> bool {
        self.world.is_alive(e)
    }

    pub fn components(&self) -> &Components {
        self.world.components()
    }

    pub fn has<T: 'static>(&self, e: Entity) -> bool {
        self.world.has::<T>(e)
    }

    pub fn get<T: 'static>(&self, e: Entity) -> Option<&T> {
        self.world.get::<T>(e)
    }

    pub fn get_mut<T: 'static>(&mut self, e: Entity) -> Option<Mut<'_, T>> {
        self.world.get_mut::<T>(e)
    }

    pub fn resource<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.world.resource::<T>()
    }

    pub fn resource_mut<T: Send + Sync + 'static>(&mut self) -> Option<&mut T> {
        self.world.resource_mut::<T>()
    }

    /// Records structural changes into a fresh buffer that is applied before
    /// the sync point ends.
    pub fn commands(&mut self) -> Commands<'_, '_> {
        let (queue, entities) = self.world.deferred_parts();
        Commands::new(queue, entities)
    }

    /// Queues an event for entity `target`; observers run at the sync point.
    pub fn emit<E: Event>(&mut self, target: Entity, event: E) {
        self.commands().emit(target, event);
    }

    pub fn emit_global<E: Event>(&mut self, event: E) {
        self.commands().emit_global(event);
    }
}

/// Registers hooks for a component type: `world.register_hooks::<T>()
/// .on_add(..).on_remove(..)`.
pub struct HooksBuilder<'w> {
    pub(crate) hooks: &'w mut ComponentHooks,
}

macro_rules! hook_setter {
    ($name:ident) => {
        /// # Panics
        /// If this hook is already registered for the component.
        pub fn $name(&mut self, hook: Hook) -> &mut Self {
            assert!(
                self.hooks.$name.is_none(),
                concat!(stringify!($name), " hook is already registered")
            );
            self.hooks.$name = Some(hook);
            self
        }
    };
}

impl HooksBuilder<'_> {
    hook_setter!(on_add);
    hook_setter!(on_insert);
    hook_setter!(on_replace);
    hook_setter!(on_remove);
}
