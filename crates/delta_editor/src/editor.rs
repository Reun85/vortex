use std::collections::HashSet;

use delta::{Entity, SceneId, World};
use serde_json::Value;

use crate::{
    command::{Command, Ctx, Existence, entity_snapshot},
    error::EditorError,
};

/// How many undo steps are kept.
const HISTORY_LIMIT: usize = 1000;

struct Entry {
    command: Command,
    /// The drag gesture this edit belongs to, if any.
    gesture: Option<u64>,
}

/// What play mode must put back when it stops.
struct PlaySnapshot {
    scene: Value,
    /// Entities that existed before play and have no `SceneId`: editor
    /// machinery such as observers. They survive play untouched.
    kept: HashSet<Entity>,
}

/// Editing state for a world: undo/redo history, selection, and play mode.
///
/// Every edit goes through the editor so it can be undone; each takes the
/// world as an argument. Entities are named by [`SceneId`], never by the
/// runtime [`Entity`], which changes when a deleted entity is restored.
///
/// Edits and undo operate on *reflected* data (the same data scene files
/// hold). Components without reflection data, such as managed `Behaviour`
/// instances, are not part of undo or play-mode snapshots yet.
#[derive(Default)]
pub struct Editor {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    selection: Option<SceneId>,
    /// Commands being collected by `begin_group`, innermost last.
    groups: Vec<(String, Vec<Command>)>,
    gesture: Option<u64>,
    next_gesture: u64,
    play: Option<PlaySnapshot>,
    warnings: Vec<String>,
}

impl Editor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn selection(&self) -> Option<SceneId> {
        self.selection
    }

    pub fn can_undo(&self) -> bool {
        self.play.is_none() && !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        self.play.is_none() && !self.redo.is_empty()
    }

    /// The name of the edit `undo` would revert, for an Edit menu.
    pub fn undo_label(&self) -> Option<String> {
        self.undo.last().map(|e| e.command.label())
    }

    pub fn redo_label(&self) -> Option<String> {
        self.redo.last().map(|e| e.command.label())
    }

    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    pub fn is_playing(&self) -> bool {
        self.play.is_some()
    }

    /// Problems met while restoring entities (such as references to
    /// something that no longer exists). Drains the list.
    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    pub fn clear_history(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    // ---- recording ---------------------------------------------------------

    fn ctx<'a>(
        world: &'a mut World,
        selection: &'a mut Option<SceneId>,
        warnings: &'a mut Vec<String>,
    ) -> Ctx<'a> {
        Ctx {
            world,
            selection,
            warnings,
        }
    }

    /// Applies `command` and records it (unless playing, where edits are
    /// applied but discarded when play stops).
    fn execute(
        &mut self,
        world: &mut World,
        mut command: Command,
    ) -> Result<(), EditorError> {
        // Remember who deleted entities were, so references to them survive
        // being saved in a snapshot and restored.
        world.track_retired_entities();
        let mut ctx = Self::ctx(world, &mut self.selection, &mut self.warnings);
        command.apply(&mut ctx)?;
        // Each edit gets its own change tick, so tools that watch for
        // changes (the inspector) can tell consecutive edits apart.
        world.increment_change_tick();
        if self.play.is_some() {
            return Ok(());
        }
        self.record(command);
        Ok(())
    }

    fn record(&mut self, command: Command) {
        if let Some((_, open)) = self.groups.last_mut() {
            open.push(command);
            return;
        }
        self.redo.clear();
        // A drag folds consecutive edits of one field into a single step:
        // the first old value, the last new one.
        if let Some(gesture) = self.gesture
            && let Some(top) = self.undo.last_mut()
            && top.gesture == Some(gesture)
            && top.command.merge_set_field(&command)
        {
            return;
        }
        self.undo.push(Entry {
            command,
            gesture: self.gesture,
        });
        if self.undo.len() > HISTORY_LIMIT {
            self.undo.remove(0);
        }
    }

    fn entity(
        &self,
        world: &World,
        id: SceneId,
    ) -> Result<Entity, EditorError> {
        world.scene_entity(id).ok_or(EditorError::UnknownEntity(id))
    }

    // ---- edits ---------------------------------------------------------------

    /// Sets one field of a component, e.g. `("game::Transform",
    /// "position.x", 3.0)`. Setting the value a field already has records
    /// nothing.
    pub fn set_field(
        &mut self,
        world: &mut World,
        id: SceneId,
        component: &str,
        path: &str,
        value: Value,
    ) -> Result<(), EditorError> {
        world.track_retired_entities();
        let e = self.entity(world, id)?;
        let old = world.field_value(e, component, path)?;
        if old == value {
            return Ok(());
        }
        self.execute(
            world,
            Command::SetField {
                id,
                component: component.to_owned(),
                path: path.to_owned(),
                old,
                new: value,
            },
        )
    }

    /// Adds a component, starting from `value` or, if none, all zero.
    pub fn add_component(
        &mut self,
        world: &mut World,
        id: SceneId,
        component: &str,
        value: Option<Value>,
    ) -> Result<(), EditorError> {
        world.track_retired_entities();
        let e = self.entity(world, id)?;
        if world.component_value(e, component).is_ok() {
            return Err(EditorError::AlreadyHas {
                id,
                component: component.to_owned(),
            });
        }
        let value = match value {
            Some(v) => v,
            None => world.default_component_value(component)?,
        };
        self.execute(
            world,
            Command::AddComponent {
                id,
                component: component.to_owned(),
                value,
            },
        )
    }

    pub fn remove_component(
        &mut self,
        world: &mut World,
        id: SceneId,
        component: &str,
    ) -> Result<(), EditorError> {
        world.track_retired_entities();
        let e = self.entity(world, id)?;
        let value = world.component_value(e, component).map_err(|_| {
            EditorError::Lacks {
                id,
                component: component.to_owned(),
            }
        })?;
        self.execute(
            world,
            Command::RemoveComponent {
                id,
                component: component.to_owned(),
                value,
            },
        )
    }

    /// Creates an entity with a fresh [`SceneId`].
    pub fn spawn(
        &mut self,
        world: &mut World,
        components: &[(&str, Value)],
    ) -> Result<SceneId, EditorError> {
        let id = SceneId::random();
        self.spawn_with_id(world, id, components)?;
        Ok(id)
    }

    pub fn spawn_with_id(
        &mut self,
        world: &mut World,
        id: SceneId,
        components: &[(&str, Value)],
    ) -> Result<(), EditorError> {
        let existence = Existence::of_snapshots(
            vec![id],
            vec![entity_snapshot(id, components)],
        );
        self.execute(world, Command::Spawn(existence))
    }

    /// Deletes entities as one undoable step. Undoing brings back the same
    /// [`SceneId`]s with the same components, and every reference to them
    /// from elsewhere points at the restored entities again. The selection is
    /// cleared if it was among them.
    pub fn despawn(
        &mut self,
        world: &mut World,
        ids: &[SceneId],
    ) -> Result<(), EditorError> {
        world.track_retired_entities();
        for &id in ids {
            self.entity(world, id)?;
        }
        let delete = Command::Despawn(Existence::of_ids(ids.to_vec()));
        match self.selection.filter(|s| ids.contains(s)) {
            None => self.execute(world, delete),
            Some(selected) => self.execute(
                world,
                Command::Group {
                    label: delete.label(),
                    commands: vec![
                        Command::Select {
                            old: Some(selected),
                            new: None,
                        },
                        delete,
                    ],
                },
            ),
        }
    }

    /// Changes the selection, as an undoable step.
    pub fn select(
        &mut self,
        world: &mut World,
        id: Option<SceneId>,
    ) -> Result<(), EditorError> {
        if let Some(id) = id {
            self.entity(world, id)?;
        }
        if self.selection == id {
            return Ok(());
        }
        self.execute(
            world,
            Command::Select {
                old: self.selection,
                new: id,
            },
        )
    }

    /// Maps the entity bits a picking pass wrote into an ID buffer back to
    /// the entity's [`SceneId`].
    pub fn pick(&self, world: &World, entity_bits: u64) -> Option<SceneId> {
        world.scene_id_of(Entity::from_bits(entity_bits))
    }

    /// Selects whatever a picking pass found at a pixel (nothing clears the
    /// selection).
    pub fn pick_and_select(
        &mut self,
        world: &mut World,
        entity_bits: u64,
    ) -> Result<(), EditorError> {
        let picked = self.pick(world, entity_bits);
        self.select(world, picked)
    }

    // ---- grouping and gestures -------------------------------------------------

    /// Starts collecting edits into one undo step.
    pub fn begin_group(&mut self, label: &str) {
        self.groups.push((label.to_owned(), Vec::new()));
    }

    /// Finishes the innermost group. An empty group records nothing.
    pub fn end_group(&mut self) {
        let Some((label, commands)) = self.groups.pop() else {
            return;
        };
        if commands.is_empty() {
            return;
        }
        self.record(Command::Group { label, commands });
    }

    /// Runs `edits` as one undo step. If any edit fails, the ones already
    /// made are reverted and the error is returned.
    pub fn group<T>(
        &mut self,
        world: &mut World,
        label: &str,
        edits: impl FnOnce(&mut Self, &mut World) -> Result<T, EditorError>,
    ) -> Result<T, EditorError> {
        self.begin_group(label);
        match edits(self, world) {
            Ok(value) => {
                self.end_group();
                Ok(value)
            }
            Err(error) => {
                let (_, mut done) = self.groups.pop().expect("group is open");
                let mut ctx =
                    Self::ctx(world, &mut self.selection, &mut self.warnings);
                for command in done.iter_mut().rev() {
                    let _ = command.revert(&mut ctx);
                }
                Err(error)
            }
        }
    }

    /// Starts a drag: consecutive edits of one field until `end_gesture`
    /// become a single undo step.
    pub fn begin_gesture(&mut self) {
        self.next_gesture += 1;
        self.gesture = Some(self.next_gesture);
    }

    pub fn end_gesture(&mut self) {
        self.gesture = None;
    }

    // ---- undo / redo -----------------------------------------------------------

    /// Reverts the last edit. Returns whether there was one (never during
    /// play, when the history is suspended).
    pub fn undo(&mut self, world: &mut World) -> Result<bool, EditorError> {
        if self.play.is_some() {
            return Ok(false);
        }
        let Some(mut entry) = self.undo.pop() else {
            return Ok(false);
        };
        let mut ctx = Self::ctx(world, &mut self.selection, &mut self.warnings);
        if let Err(e) = entry.command.revert(&mut ctx) {
            self.undo.push(entry);
            return Err(e);
        }
        world.increment_change_tick();
        self.redo.push(entry);
        Ok(true)
    }

    pub fn redo(&mut self, world: &mut World) -> Result<bool, EditorError> {
        if self.play.is_some() {
            return Ok(false);
        }
        let Some(mut entry) = self.redo.pop() else {
            return Ok(false);
        };
        let mut ctx = Self::ctx(world, &mut self.selection, &mut self.warnings);
        if let Err(e) = entry.command.apply(&mut ctx) {
            self.redo.push(entry);
            return Err(e);
        }
        world.increment_change_tick();
        self.undo.push(entry);
        Ok(true)
    }

    // ---- play mode ---------------------------------------------------------------

    /// Saves the scene and starts play. While playing, edits still apply but
    /// are not recorded, and undo/redo do nothing; the history is intact when
    /// play stops.
    pub fn enter_play(&mut self, world: &World) -> Result<(), EditorError> {
        if self.play.is_some() {
            return Err(EditorError::AlreadyPlaying);
        }
        let kept = world
            .all_entities()
            .into_iter()
            .filter(|&e| world.scene_id_of(e).is_none())
            .collect();
        self.play = Some(PlaySnapshot {
            scene: world.scene_to_value(),
            kept,
        });
        Ok(())
    }

    /// Ends play and puts the scene back as it was when play started:
    /// everything play spawned is gone, everything it changed or deleted is
    /// restored. Entities get new runtime handles but keep their
    /// [`SceneId`]s. Resources and non-reflected components are not part of
    /// the snapshot.
    pub fn stop_play(&mut self, world: &mut World) -> Result<(), EditorError> {
        let snapshot = self.play.take().ok_or(EditorError::NotPlaying)?;
        for e in world.all_entities() {
            let is_scene = world.scene_id_of(e).is_some();
            if is_scene || !snapshot.kept.contains(&e) {
                let _ = world.despawn(e);
            }
        }
        let report = world.load_scene_value(&snapshot.scene)?;
        self.warnings.extend(report.warnings);
        // Every handle from before play is stale now.
        world.forget_retired_entities();
        world.increment_change_tick();
        Ok(())
    }
}
