//! Undoable edits, expressed on reflected data.
//!
//! Commands refer to entities by [`SceneId`] and carry values as JSON in the
//! scene-file form, never raw bytes or runtime handles. That keeps them valid
//! across a script hot reload (layouts may change) and lets a restored
//! entity be the *same* entity.

use delta::{Entity, SceneId, World};
use serde_json::{Map, Value};

use crate::error::EditorError;

/// What a command may touch besides the world.
pub(crate) struct Ctx<'a> {
    pub world: &'a mut World,
    pub selection: &'a mut Option<SceneId>,
    /// Recoverable problems met while restoring entities.
    pub warnings: &'a mut Vec<String>,
}

impl Ctx<'_> {
    fn entity(&self, id: SceneId) -> Result<Entity, EditorError> {
        self.world
            .scene_entity(id)
            .ok_or(EditorError::UnknownEntity(id))
    }
}

/// A set of entities that can be destroyed and recreated.
///
/// Destroying captures every entity's current state *first*, then despawns,
/// so references between them are recorded while they all still exist.
/// Creating spawns them all before filling any, so those references
/// resolve, and then re-points everything that still holds an old handle.
#[derive(Clone, Debug)]
pub struct Existence {
    ids: Vec<SceneId>,
    /// Empty until first captured (a delete captures when it runs).
    snapshots: Vec<Value>,
    /// The runtime handles the entities last had, to find stale references.
    handles: Vec<Entity>,
}

impl Existence {
    pub(crate) fn of_ids(ids: Vec<SceneId>) -> Self {
        let handles = vec![Entity::PLACEHOLDER; ids.len()];
        Self {
            ids,
            snapshots: Vec::new(),
            handles,
        }
    }

    pub(crate) fn of_snapshots(
        ids: Vec<SceneId>,
        snapshots: Vec<Value>,
    ) -> Self {
        let handles = vec![Entity::PLACEHOLDER; ids.len()];
        Self {
            ids,
            snapshots,
            handles,
        }
    }

    pub fn ids(&self) -> &[SceneId] {
        &self.ids
    }

    fn destroy(&mut self, ctx: &mut Ctx<'_>) -> Result<(), EditorError> {
        let mut entities = Vec::with_capacity(self.ids.len());
        let mut snapshots = Vec::with_capacity(self.ids.len());
        for &id in &self.ids {
            let e = ctx.entity(id)?;
            snapshots.push(
                ctx.world
                    .entity_to_value(e)
                    .ok_or(EditorError::UnknownEntity(id))?,
            );
            entities.push(e);
        }
        for (&id, &e) in self.ids.iter().zip(&entities) {
            ctx.world
                .despawn(e)
                .map_err(|_| EditorError::UnknownEntity(id))?;
        }
        self.snapshots = snapshots;
        self.handles = entities;
        Ok(())
    }

    fn create(&mut self, ctx: &mut Ctx<'_>) -> Result<(), EditorError> {
        let report = ctx.world.spawn_from_values(&self.snapshots)?;
        ctx.warnings.extend(report.warnings);
        for (&old, &new) in self.handles.iter().zip(&report.entities) {
            if old != Entity::PLACEHOLDER && old != new {
                ctx.world.remap_entity_references(old, new);
            }
        }
        self.handles = report.entities;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub enum Command {
    SetField {
        id: SceneId,
        component: String,
        path: String,
        old: Value,
        new: Value,
    },
    AddComponent {
        id: SceneId,
        component: String,
        value: Value,
    },
    RemoveComponent {
        id: SceneId,
        component: String,
        /// The full component as it was, so undo restores it exactly.
        value: Value,
    },
    Spawn(Existence),
    Despawn(Existence),
    Select {
        old: Option<SceneId>,
        new: Option<SceneId>,
    },
    Group {
        label: String,
        commands: Vec<Command>,
    },
}

impl Command {
    /// A short description for an Edit menu.
    pub fn label(&self) -> String {
        match self {
            Self::SetField {
                component, path, ..
            } => {
                let short = component.rsplit("::").next().unwrap_or(component);
                if path.is_empty() {
                    format!("Set {short}")
                } else {
                    format!("Set {short}.{path}")
                }
            }
            Self::AddComponent { component, .. } => format!("Add {component}"),
            Self::RemoveComponent { component, .. } => {
                format!("Remove {component}")
            }
            Self::Spawn(e) => format!("Create {} entities", e.ids.len()),
            Self::Despawn(e) => format!("Delete {} entities", e.ids.len()),
            Self::Select { .. } => "Select".into(),
            Self::Group { label, .. } => label.clone(),
        }
    }

    pub(crate) fn apply(
        &mut self,
        ctx: &mut Ctx<'_>,
    ) -> Result<(), EditorError> {
        match self {
            Self::SetField {
                id,
                component,
                path,
                new,
                ..
            } => {
                let e = ctx.entity(*id)?;
                ctx.world.set_field_value(e, component, path, new)?;
            }
            Self::AddComponent {
                id,
                component,
                value,
            } => {
                let e = ctx.entity(*id)?;
                ctx.world.insert_component_value(e, component, value)?;
            }
            Self::RemoveComponent { id, component, .. } => {
                let e = ctx.entity(*id)?;
                ctx.world.remove_component_named(e, component)?;
            }
            Self::Spawn(existence) => existence.create(ctx)?,
            Self::Despawn(existence) => existence.destroy(ctx)?,
            Self::Select { new, .. } => *ctx.selection = *new,
            Self::Group { commands, .. } => {
                for i in 0..commands.len() {
                    if let Err(e) = commands[i].apply(ctx) {
                        for done in commands[..i].iter_mut().rev() {
                            let _ = done.revert(ctx);
                        }
                        return Err(e);
                    }
                }
            }
        }
        Ok(())
    }

    pub(crate) fn revert(
        &mut self,
        ctx: &mut Ctx<'_>,
    ) -> Result<(), EditorError> {
        match self {
            Self::SetField {
                id,
                component,
                path,
                old,
                ..
            } => {
                let e = ctx.entity(*id)?;
                ctx.world.set_field_value(e, component, path, old)?;
            }
            Self::AddComponent { id, component, .. } => {
                let e = ctx.entity(*id)?;
                ctx.world.remove_component_named(e, component)?;
            }
            Self::RemoveComponent {
                id,
                component,
                value,
            } => {
                let e = ctx.entity(*id)?;
                ctx.world.insert_component_value(e, component, value)?;
            }
            Self::Spawn(existence) => existence.destroy(ctx)?,
            Self::Despawn(existence) => existence.create(ctx)?,
            Self::Select { old, .. } => *ctx.selection = *old,
            Self::Group { commands, .. } => {
                for i in (0..commands.len()).rev() {
                    if let Err(e) = commands[i].revert(ctx) {
                        for done in commands[i + 1..].iter_mut() {
                            let _ = done.apply(ctx);
                        }
                        return Err(e);
                    }
                }
            }
        }
        Ok(())
    }

    /// Two edits of the same field, which a drag gesture folds into one.
    pub(crate) fn merge_set_field(&mut self, next: &Command) -> bool {
        match (self, next) {
            (
                Self::SetField {
                    id,
                    component,
                    path,
                    new,
                    ..
                },
                Self::SetField {
                    id: id2,
                    component: c2,
                    path: p2,
                    new: new2,
                    ..
                },
            ) if id == id2 && component == c2 && path == p2 => {
                *new = new2.clone();
                true
            }
            _ => false,
        }
    }
}

/// Builds the scene-form value of a new entity.
pub(crate) fn entity_snapshot(
    id: SceneId,
    components: &[(&str, Value)],
) -> Value {
    let mut map = Map::new();
    for (name, value) in components {
        map.insert((*name).to_owned(), value.clone());
    }
    let mut entity = Map::new();
    entity.insert("id".into(), Value::String(id.to_string()));
    entity.insert("components".into(), Value::Object(map));
    Value::Object(entity)
}
