use crate::{
    entity::{Entities, Entity},
    system::{Access, SystemParam},
    tick::Tick,
    world::{World, WorldCell},
};

type Command = Box<dyn FnOnce(&mut World)>;

/// Deferred world edits, applied at the next sync point.
///
/// Closure-based for now; the byte-encoded, batching format arrives with the
/// command-buffer milestone.
#[derive(Default)]
pub struct CommandQueue {
    commands: Vec<Command>,
}

impl CommandQueue {
    pub fn push(&mut self, command: impl FnOnce(&mut World) + 'static) {
        self.commands.push(Box::new(command));
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    pub fn apply(&mut self, world: &mut World) {
        for command in self.commands.drain(..) {
            command(world);
        }
    }
}

/// Records structural changes instead of performing them, so no column
/// pointer or slice held by a running system is ever invalidated.
pub struct Commands<'w, 's> {
    queue: &'s mut CommandQueue,
    entities: &'w Entities,
}

impl<'w, 's> Commands<'w, 's> {
    /// Reserves an entity now; it gets its components at the sync point.
    pub fn spawn(&mut self) -> EntityCommands<'_, 'w, 's> {
        let entity = self.entities.reserve();
        self.queue.push(|world| world.flush_entities());
        EntityCommands {
            entity,
            commands: self,
        }
    }

    pub fn entity(&mut self, entity: Entity) -> EntityCommands<'_, 'w, 's> {
        EntityCommands {
            entity,
            commands: self,
        }
    }

    pub fn despawn(&mut self, entity: Entity) {
        self.queue.push(move |world| {
            let _ = world.despawn(entity);
        });
    }

    pub fn insert_resource<T: Send + Sync + 'static>(&mut self, value: T) {
        self.queue.push(move |world| world.insert_resource(value));
    }

    /// Queues arbitrary work with full world access.
    pub fn add(&mut self, command: impl FnOnce(&mut World) + 'static) {
        self.queue.push(command);
    }
}

pub struct EntityCommands<'a, 'w, 's> {
    entity: Entity,
    commands: &'a mut Commands<'w, 's>,
}

impl EntityCommands<'_, '_, '_> {
    pub fn id(&self) -> Entity {
        self.entity
    }

    /// Adds (or overwrites) a component. Inserts on a dead entity are ignored.
    pub fn insert<T: Send + Sync + 'static>(&mut self, value: T) -> &mut Self {
        let entity = self.entity;
        self.commands.queue.push(move |world| {
            let _ = world.insert(entity, value);
        });
        self
    }

    pub fn remove<T: Send + Sync + 'static>(&mut self) -> &mut Self {
        let entity = self.entity;
        self.commands.queue.push(move |world| {
            let _ = world.remove::<T>(entity);
        });
        self
    }

    pub fn despawn(&mut self) {
        self.commands.despawn(self.entity);
    }
}

// SAFETY: only shared access to the entity allocator (atomic reservation),
// and it declares no component or resource access.
unsafe impl SystemParam for Commands<'_, '_> {
    type State = CommandQueue;
    type Item<'w, 's> = Commands<'w, 's>;

    fn init_state(_: &mut World, _: &mut Access) -> CommandQueue {
        CommandQueue::default()
    }

    unsafe fn get_param<'w, 's>(
        state: &'s mut CommandQueue,
        _: Tick,
        world: WorldCell<'w>,
    ) -> Commands<'w, 's> {
        Commands {
            queue: state,
            // SAFETY: nothing mutates the allocator during a system run.
            entities: unsafe { world.entities() },
        }
    }

    fn apply(state: &mut CommandQueue, world: &mut World) {
        state.apply(world);
    }
}
