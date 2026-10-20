//! delta: the archetype ECS at the core of the vortex engine.

pub mod archetype;
pub mod blob_vec;
pub mod column;
pub mod component;
pub mod entity;
pub mod query;
pub mod tick;
pub mod world;

pub use archetype::{Archetype, ArchetypeId, Archetypes};
pub use blob_vec::BlobVec;
pub use column::Column;
pub use component::{ComponentId, ComponentInfo, Components, Kind, Storage};
pub use entity::{Entities, Entity, EntityLocation};
pub use query::{Chunk, QueryDesc, QueryError, QueryState};
pub use tick::Tick;
pub use world::{DeadEntity, World};
