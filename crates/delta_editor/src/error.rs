use std::fmt;

use delta::{InspectError, SceneError, SceneId};

#[derive(Debug, PartialEq)]
pub enum EditorError {
    UnknownEntity(SceneId),
    /// `add_component` on an entity that already has it.
    AlreadyHas {
        id: SceneId,
        component: String,
    },
    /// `remove_component` on an entity that lacks it.
    Lacks {
        id: SceneId,
        component: String,
    },
    Inspect(InspectError),
    Scene(SceneError),
    AlreadyPlaying,
    NotPlaying,
}

impl fmt::Display for EditorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownEntity(id) => {
                write!(f, "no entity with scene id {id}")
            }
            Self::AlreadyHas { id, component } => {
                write!(f, "entity {id} already has `{component}`")
            }
            Self::Lacks { id, component } => {
                write!(f, "entity {id} has no `{component}`")
            }
            Self::Inspect(e) => write!(f, "{e}"),
            Self::Scene(e) => write!(f, "{e}"),
            Self::AlreadyPlaying => write!(f, "already in play mode"),
            Self::NotPlaying => write!(f, "not in play mode"),
        }
    }
}

impl std::error::Error for EditorError {}

impl From<InspectError> for EditorError {
    fn from(e: InspectError) -> Self {
        Self::Inspect(e)
    }
}

impl From<SceneError> for EditorError {
    fn from(e: SceneError) -> Self {
        Self::Scene(e)
    }
}
