use std::collections::BTreeSet;

use delta::{SceneId, World};
use serde_json::Value;

/// What changed on the inspected entity since the last refresh.
#[derive(Debug, Default, PartialEq)]
pub struct InspectorUpdate {
    /// The entity no longer exists.
    pub missing: bool,
    /// Components the entity gained (all of them on the first refresh).
    pub added: Vec<String>,
    /// Components the entity lost.
    pub removed: Vec<String>,
    /// Current values of the components that are new or were written since
    /// the last refresh. Components that did not change are not re-read.
    pub changed: Vec<(String, Value)>,
}

/// Shows one entity's reflected components, re-reading only what changed.
///
/// A component counts as changed when its change tick is newer than the tick
/// of the previous refresh, the same mechanism `Changed<T>` filters use, so
/// it is exact and costs one tick comparison per component, not a diff of
/// values.
#[derive(Default)]
pub struct Inspector {
    target: Option<SceneId>,
    last_run: delta::Tick,
    known: BTreeSet<String>,
}

impl Inspector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn target(&self) -> Option<SceneId> {
        self.target
    }

    /// Points the inspector somewhere else; the next refresh reports
    /// everything.
    pub fn set_target(&mut self, target: Option<SceneId>) {
        self.target = target;
        self.known.clear();
        self.last_run = delta::Tick::new(0);
    }

    /// Reports what changed since the previous refresh.
    pub fn refresh(&mut self, world: &mut World) -> InspectorUpdate {
        let mut update = InspectorUpdate::default();
        let this_run = world.change_tick();

        let entity = self.target.and_then(|id| world.scene_entity(id));
        let Some(entity) = entity else {
            update.missing = self.target.is_some();
            update.removed =
                std::mem::take(&mut self.known).into_iter().collect();
            return update;
        };

        let current: BTreeSet<String> =
            world.reflected_components(entity).into_iter().collect();
        update.added = current.difference(&self.known).cloned().collect();
        update.removed = self.known.difference(&current).cloned().collect();

        for name in &current {
            let is_new = update.added.contains(name);
            let written = world
                .component_changed_tick(entity, name)
                .is_ok_and(|t| t.is_newer_than(self.last_run, this_run));
            if (is_new || written)
                && let Ok(value) = world.component_value(entity, name)
            {
                update.changed.push((name.clone(), value));
            }
        }

        self.known = current;
        // Whatever happens next gets a later tick than this refresh.
        self.last_run = this_run;
        world.increment_change_tick();
        update
    }
}
