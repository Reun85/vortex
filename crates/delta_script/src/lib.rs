//! The CoreCLR scripting bridge for delta.
//!
//! The ECS crate knows nothing about .NET; this crate hosts the runtime
//! through `hostfxr`, exchanges two tables of function pointers with the C#
//! side ([`ffi::HostApi`] down, [`ffi::ScriptApi`] up), and exposes
//! script-declared components, queries and systems to the world.

mod bridge;
pub mod ffi;
pub mod host;
mod script;

pub use bridge::{PendingSystem, RemovedRecord};
pub use host::{DotnetHost, HostError};
pub use script::{
    ReloadReport,
    ScriptAssembly,
    ScriptError,
    ScriptHost,
    ScriptSystem,
};
