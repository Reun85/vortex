//! Editor-side support for delta worlds: undo/redo on reflected data,
//! selection and picking, play-mode snapshots, and an inspector that
//! refreshes by change tick. No UI lives here; a front end drives these.

mod command;
mod editor;
mod error;
mod inspector;

pub use command::{Command, Existence};
pub use editor::Editor;
pub use error::EditorError;
pub use inspector::{Inspector, InspectorUpdate};
