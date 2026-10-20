//! delta: the archetype ECS at the core of the vortex engine.

pub mod archetype;
pub mod blob_vec;
pub mod change;
pub mod column;
pub mod commands;
pub mod component;
pub mod entity;
pub mod hooks;
pub mod message;
pub mod observer;
pub mod query;
pub mod schedule;
pub mod system;
pub mod tick;
pub mod world;
pub mod world_query;

pub use archetype::{Archetype, ArchetypeId, Archetypes};
pub use blob_vec::BlobVec;
pub use change::Mut;
pub use column::Column;
pub use commands::{CommandQueue, Commands, EntityCommands};
pub use component::{ComponentId, ComponentInfo, Components, Kind, Storage};
pub use entity::{Entities, Entity, EntityLocation};
pub use hooks::{ComponentHooks, DeferredWorld, Hook, HooksBuilder};
pub use message::{MessageReader, MessageWriter, Messages};
pub use observer::{Event, EventHeader, EventId, Events, Observer, Trigger};
pub use query::{Chunk, QueryDesc, QueryError, QueryState};
pub use schedule::{Schedule, Stage};
pub use system::{Access, IntoSystem, Local, Res, ResMut, System, SystemParam};
pub use tick::{CHECK_TICK_THRESHOLD, MAX_CHANGE_AGE, Tick};
pub use world::{DeadEntity, World, WorldCell};
pub use world_query::{
    Added,
    Changed,
    Query,
    QueryFilter,
    ReadOnlyWorldQuery,
    With,
    Without,
    WorldQuery,
};
