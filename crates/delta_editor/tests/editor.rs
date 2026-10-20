//! Undo, selection, play mode and the inspector, driven through the public
//! API the way an editor front end would.

use delta::{Entity, Query, Reflect, SceneId, Schedule, Stage, World};
use delta_editor::{Editor, EditorError, Inspector};
use serde_json::{Value, json};

#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
struct Vec3 {
    x: f32,
    y: f32,
    z: f32,
}

#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(name = "game::Transform")]
struct Transform {
    position: Vec3,
    scale: f32,
    rotation: [f32; 4],
}

#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(name = "game::Health")]
struct Health {
    current: i32,
    max: i32,
}

#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(name = "game::Follows")]
struct Follows {
    target: Entity,
    speed: f32,
}

#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(name = "game::Player")]
struct Player;

const TRANSFORM: &str = "game::Transform";
const HEALTH: &str = "game::Health";
const FOLLOWS: &str = "game::Follows";
const PLAYER: &str = "game::Player";

fn world() -> World {
    let mut w = World::new();
    w.register_reflected::<Transform>();
    w.register_reflected::<Health>();
    w.register_reflected::<Follows>();
    w.register_reflected::<Player>();
    w
}

fn sid(n: u128) -> SceneId {
    SceneId(n)
}

fn transform(x: f32) -> Value {
    json!({"position": {"x": x, "y": 0.0, "z": 0.0}, "scale": 1.0, "rotation": [0.0, 0.0, 0.0, 1.0]})
}

fn health(current: i32) -> Value {
    json!({"current": current, "max": 100})
}

fn follows(target: Option<SceneId>) -> Value {
    json!({"target": target.map(|t| t.to_string()), "speed": 2.5})
}

/// A -> B, C -> B: B is referenced by two entities.
fn level(ed: &mut Editor, w: &mut World) {
    ed.spawn_with_id(
        w,
        sid(1),
        &[
            (TRANSFORM, transform(1.0)),
            (HEALTH, health(50)),
            (PLAYER, json!({})),
        ],
    )
    .unwrap();
    ed.spawn_with_id(w, sid(2), &[(FOLLOWS, follows(Some(sid(1))))])
        .unwrap();
    ed.spawn_with_id(
        w,
        sid(3),
        &[(FOLLOWS, follows(Some(sid(1)))), (HEALTH, health(7))],
    )
    .unwrap();
    ed.clear_history();
}

fn ent(w: &World, id: u128) -> Entity {
    w.scene_entity(sid(id)).unwrap()
}

// --- the roadmap exit test -----------------------------------------------------

#[test]
fn undoing_a_delete_restores_the_same_entity_and_its_references() {
    let (mut w, mut ed) = (world(), Editor::new());
    level(&mut ed, &mut w);
    let before = w.save_scene();
    let old_handle = ent(&w, 1);

    ed.despawn(&mut w, &[sid(1)]).unwrap();
    assert!(!w.is_alive(old_handle));
    assert_eq!(w.scene_entity(sid(1)), None);
    // The referencing entities still hold the dead handle.
    assert_eq!(w.get::<Follows>(ent(&w, 2)).unwrap().target, old_handle);

    // Churn the allocator so the restored entity cannot reuse the old handle.
    for _ in 0..5 {
        let e = w.spawn();
        w.despawn(e).unwrap();
    }

    assert!(ed.undo(&mut w).unwrap());
    let restored = ent(&w, 1);
    assert!(w.is_alive(restored));
    assert_ne!(restored, old_handle, "a new runtime handle...");
    assert_eq!(
        w.scene_id_of(restored),
        Some(sid(1)),
        "...for the same scene entity"
    );
    // Same components, same values.
    assert_eq!(
        w.get::<Health>(restored),
        Some(&Health {
            current: 50,
            max: 100
        })
    );
    assert!(w.has::<Player>(restored));
    // Every reference points at the restored entity again.
    assert_eq!(w.get::<Follows>(ent(&w, 2)).unwrap().target, restored);
    assert_eq!(w.get::<Follows>(ent(&w, 3)).unwrap().target, restored);
    assert_eq!(w.save_scene(), before);
    assert!(ed.take_warnings().is_empty());

    // Delete, restore, delete, restore: the references follow each time.
    assert!(ed.redo(&mut w).unwrap());
    assert_eq!(w.scene_entity(sid(1)), None);
    assert!(ed.undo(&mut w).unwrap());
    let again = ent(&w, 1);
    assert_eq!(w.get::<Follows>(ent(&w, 2)).unwrap().target, again);
    assert_eq!(w.get::<Follows>(ent(&w, 3)).unwrap().target, again);
    assert_eq!(w.save_scene(), before);
}

#[test]
fn deleting_entities_that_reference_each_other_restores_those_links() {
    let (mut w, mut ed) = (world(), Editor::new());
    ed.spawn_with_id(&mut w, sid(1), &[(FOLLOWS, follows(None))])
        .unwrap();
    ed.spawn_with_id(&mut w, sid(2), &[(FOLLOWS, follows(Some(sid(1))))])
        .unwrap();
    // Close the loop: 1 follows 2.
    ed.set_field(&mut w, sid(1), FOLLOWS, "target", json!(sid(2).to_string()))
        .unwrap();
    ed.clear_history();
    let before = w.save_scene();

    // One step deletes both; their snapshots are taken while both exist.
    ed.despawn(&mut w, &[sid(1), sid(2)]).unwrap();
    assert_eq!(w.entities().len(), 0);
    ed.undo(&mut w).unwrap();

    let (a, b) = (ent(&w, 1), ent(&w, 2));
    assert_eq!(w.get::<Follows>(a).unwrap().target, b);
    assert_eq!(w.get::<Follows>(b).unwrap().target, a);
    assert_eq!(w.save_scene(), before);
}

#[test]
fn deleting_the_selected_entity_clears_the_selection_and_undo_brings_it_back() {
    let (mut w, mut ed) = (world(), Editor::new());
    level(&mut ed, &mut w);
    ed.select(&mut w, Some(sid(1))).unwrap();
    ed.despawn(&mut w, &[sid(1)]).unwrap();
    assert_eq!(ed.selection(), None);
    ed.undo(&mut w).unwrap(); // one step, so selection and entity return together
    assert_eq!(ed.selection(), Some(sid(1)));
    assert!(w.scene_entity(sid(1)).is_some());
    ed.undo(&mut w).unwrap(); // the selection itself was an edit
    assert_eq!(ed.selection(), None);
}

#[test]
fn a_reference_to_an_already_deleted_entity_survives_being_deleted_too() {
    let (mut w, mut ed) = (world(), Editor::new());
    level(&mut ed, &mut w);
    let before = w.save_scene();

    // Entity 3 points at entity 1. Delete 1 first, then delete 3 while its
    // reference dangles. Both snapshots must keep that link.
    ed.despawn(&mut w, &[sid(1)]).unwrap();
    ed.despawn(&mut w, &[sid(3)]).unwrap();

    ed.undo(&mut w).unwrap(); // bring 3 back: still pointing at the dead 1
    let dangling = w.get::<Follows>(ent(&w, 3)).unwrap().target;
    assert!(!w.is_alive(dangling), "still points at the deleted entity");
    assert_ne!(dangling, Entity::PLACEHOLDER, "and is not just 'nothing'");
    ed.undo(&mut w).unwrap(); // bring 1 back: 3 and 2 point at it again
    let one = ent(&w, 1);
    assert_eq!(w.get::<Follows>(ent(&w, 3)).unwrap().target, one);
    assert_eq!(w.get::<Follows>(ent(&w, 2)).unwrap().target, one);
    assert_eq!(w.save_scene(), before);
    assert!(ed.take_warnings().is_empty());

    // The same through redo, restoring in the opposite order of deletion.
    ed.redo(&mut w).unwrap();
    ed.redo(&mut w).unwrap();
    ed.undo(&mut w).unwrap();
    ed.undo(&mut w).unwrap();
    assert_eq!(w.save_scene(), before);
}

#[test]
fn editing_a_dangling_reference_keeps_what_it_pointed_at() {
    let (mut w, mut ed) = (world(), Editor::new());
    level(&mut ed, &mut w);
    let before = w.save_scene();
    ed.despawn(&mut w, &[sid(1)]).unwrap();
    // Entity 2 points at the deleted entity 1; retarget it to 3, then undo.
    ed.set_field(&mut w, sid(2), FOLLOWS, "target", json!(sid(3).to_string()))
        .unwrap();
    ed.undo(&mut w).unwrap();
    ed.undo(&mut w).unwrap();
    assert_eq!(w.save_scene(), before);
}

// --- field edits -----------------------------------------------------------------

#[test]
fn set_field_undo_redo_with_nested_and_array_paths() {
    let (mut w, mut ed) = (world(), Editor::new());
    level(&mut ed, &mut w);
    let before = w.save_scene();
    ed.set_field(&mut w, sid(1), TRANSFORM, "position.y", json!(9.5))
        .unwrap();
    ed.set_field(&mut w, sid(1), TRANSFORM, "rotation[2]", json!(-0.5))
        .unwrap();
    ed.set_field(&mut w, sid(1), HEALTH, "current", json!(1))
        .unwrap();
    let e = ent(&w, 1);
    assert_eq!(w.get::<Transform>(e).unwrap().position.y, 9.5);
    assert_eq!(w.get::<Transform>(e).unwrap().rotation[2], -0.5);
    assert_eq!(ed.undo_depth(), 3);
    assert_eq!(ed.undo_label().as_deref(), Some("Set Health.current"));

    while ed.undo(&mut w).unwrap() {}
    assert_eq!(w.save_scene(), before);
    assert!(!ed.can_undo() && ed.can_redo());
    while ed.redo(&mut w).unwrap() {}
    assert_eq!(w.get::<Health>(e).unwrap().current, 1);
    assert_eq!(w.get::<Transform>(e).unwrap().rotation[2], -0.5);
}

#[test]
fn a_drag_gesture_is_one_undo_step() {
    let (mut w, mut ed) = (world(), Editor::new());
    level(&mut ed, &mut w);
    let e = ent(&w, 1);

    ed.begin_gesture();
    for x in [2.0, 3.0, 4.0, 5.0] {
        ed.set_field(&mut w, sid(1), TRANSFORM, "position.x", json!(x))
            .unwrap();
    }
    // A different field in the same gesture is its own step.
    ed.set_field(&mut w, sid(1), TRANSFORM, "scale", json!(3.0))
        .unwrap();
    ed.end_gesture();
    assert_eq!(ed.undo_depth(), 2);

    // Without a gesture every edit counts.
    ed.set_field(&mut w, sid(1), TRANSFORM, "position.x", json!(6.0))
        .unwrap();
    ed.set_field(&mut w, sid(1), TRANSFORM, "position.x", json!(7.0))
        .unwrap();
    assert_eq!(ed.undo_depth(), 4);

    ed.undo(&mut w).unwrap();
    ed.undo(&mut w).unwrap();
    ed.undo(&mut w).unwrap(); // the scale change
    assert_eq!(w.get::<Transform>(e).unwrap().scale, 1.0);
    // The whole drag reverts to the value from *before* it started...
    assert_eq!(w.get::<Transform>(e).unwrap().position.x, 5.0);
    ed.undo(&mut w).unwrap();
    assert_eq!(w.get::<Transform>(e).unwrap().position.x, 1.0);
    // ...and redoes to where it ended.
    ed.redo(&mut w).unwrap();
    assert_eq!(w.get::<Transform>(e).unwrap().position.x, 5.0);
}

#[test]
fn unchanged_and_invalid_edits_record_nothing_and_change_nothing() {
    let (mut w, mut ed) = (world(), Editor::new());
    level(&mut ed, &mut w);
    let before = w.save_scene();

    ed.set_field(&mut w, sid(1), HEALTH, "current", json!(50))
        .unwrap(); // same value
    assert_eq!(ed.undo_depth(), 0);

    let bad = |r: Result<(), EditorError>| assert!(r.is_err());
    bad(ed.set_field(&mut w, sid(1), HEALTH, "nope", json!(1)));
    bad(ed.set_field(&mut w, sid(1), HEALTH, "current", json!("many")));
    bad(ed.set_field(&mut w, sid(1), "game::Missing", "x", json!(1)));
    bad(ed.set_field(&mut w, sid(99), HEALTH, "current", json!(1)));
    bad(ed.set_field(&mut w, sid(2), HEALTH, "current", json!(1))); // has no Health
    assert_eq!(ed.undo_depth(), 0);
    assert_eq!(w.save_scene(), before);
}

#[test]
fn components_can_be_added_and_removed_with_undo() {
    let (mut w, mut ed) = (world(), Editor::new());
    level(&mut ed, &mut w);
    let before = w.save_scene();
    let e = ent(&w, 2);

    ed.add_component(&mut w, sid(2), HEALTH, None).unwrap(); // starts all zero
    assert_eq!(w.get::<Health>(e), Some(&Health { current: 0, max: 0 }));
    ed.set_field(&mut w, sid(2), HEALTH, "max", json!(80))
        .unwrap();
    ed.remove_component(&mut w, sid(1), TRANSFORM).unwrap();
    assert!(!w.has::<Transform>(ent(&w, 1)));

    assert!(matches!(
        ed.add_component(&mut w, sid(2), HEALTH, None),
        Err(EditorError::AlreadyHas { .. })
    ));
    assert!(matches!(
        ed.remove_component(&mut w, sid(2), TRANSFORM),
        Err(EditorError::Lacks { .. })
    ));

    while ed.undo(&mut w).unwrap() {}
    assert_eq!(
        w.save_scene(),
        before,
        "removed Transform came back exactly"
    );
    while ed.redo(&mut w).unwrap() {}
    assert_eq!(
        w.get::<Health>(e),
        Some(&Health {
            current: 0,
            max: 80
        })
    );
    assert!(!w.has::<Transform>(ent(&w, 1)));
}

#[test]
fn creating_entities_undoes_and_redoes() {
    let (mut w, mut ed) = (world(), Editor::new());
    level(&mut ed, &mut w);
    let before = w.save_scene();
    let id = ed
        .spawn(&mut w, &[(HEALTH, health(5)), (TRANSFORM, transform(2.0))])
        .unwrap();
    assert_eq!(w.entities().len(), 4);
    ed.undo(&mut w).unwrap();
    assert_eq!(w.scene_entity(id), None);
    assert_eq!(w.save_scene(), before);
    ed.redo(&mut w).unwrap();
    assert_eq!(
        w.get::<Health>(w.scene_entity(id).unwrap())
            .unwrap()
            .current,
        5
    );
}

#[test]
fn a_new_edit_discards_the_redo_history() {
    let (mut w, mut ed) = (world(), Editor::new());
    level(&mut ed, &mut w);
    ed.set_field(&mut w, sid(1), HEALTH, "current", json!(1))
        .unwrap();
    ed.undo(&mut w).unwrap();
    assert!(ed.can_redo());
    ed.set_field(&mut w, sid(1), HEALTH, "current", json!(2))
        .unwrap();
    assert!(!ed.can_redo());
}

// --- groups ------------------------------------------------------------------------

#[test]
fn a_group_is_one_step_and_failure_rolls_it_back() {
    let (mut w, mut ed) = (world(), Editor::new());
    level(&mut ed, &mut w);
    let before = w.save_scene();

    ed.group(&mut w, "Heal everyone", |ed, w| {
        ed.set_field(w, sid(1), HEALTH, "current", json!(100))?;
        ed.set_field(w, sid(3), HEALTH, "current", json!(100))
    })
    .unwrap();
    assert_eq!(ed.undo_depth(), 1);
    assert_eq!(ed.undo_label().as_deref(), Some("Heal everyone"));
    ed.undo(&mut w).unwrap();
    assert_eq!(w.save_scene(), before);

    let failed = ed.group(&mut w, "Doomed", |ed, w| {
        ed.set_field(w, sid(1), HEALTH, "current", json!(1))?;
        ed.set_field(w, sid(99), HEALTH, "current", json!(1)) // no such entity
    });
    assert!(failed.is_err());
    assert_eq!(ed.undo_depth(), 0, "nothing recorded");
    assert_eq!(w.save_scene(), before, "the first edit was rolled back");
}

// --- picking ---------------------------------------------------------------------------

#[test]
fn picking_maps_id_buffer_bits_to_a_scene_id_and_selects_undoably() {
    let (mut w, mut ed) = (world(), Editor::new());
    level(&mut ed, &mut w);
    let bits = ent(&w, 3).to_bits();
    assert_eq!(ed.pick(&w, bits), Some(sid(3)));
    assert_eq!(ed.pick(&w, Entity::PLACEHOLDER.to_bits()), None);

    ed.pick_and_select(&mut w, bits).unwrap();
    assert_eq!(ed.selection(), Some(sid(3)));
    ed.pick_and_select(&mut w, Entity::PLACEHOLDER.to_bits())
        .unwrap(); // clicked the sky
    assert_eq!(ed.selection(), None);
    ed.undo(&mut w).unwrap();
    assert_eq!(ed.selection(), Some(sid(3)));
}

// --- inspector --------------------------------------------------------------------------

#[test]
fn the_inspector_rereads_only_components_that_changed() {
    let (mut w, mut ed) = (world(), Editor::new());
    level(&mut ed, &mut w);
    let mut inspector = Inspector::new();
    inspector.set_target(Some(sid(1)));

    // First refresh reports everything.
    let first = inspector.refresh(&mut w);
    assert_eq!(first.added, [HEALTH, PLAYER, TRANSFORM]);
    assert_eq!(first.changed.len(), 3);
    assert!(!first.missing);

    // Nothing happened: nothing is re-read.
    let quiet = inspector.refresh(&mut w);
    assert!(
        quiet.added.is_empty()
            && quiet.removed.is_empty()
            && quiet.changed.is_empty()
    );

    // An editor edit changes one component.
    ed.set_field(&mut w, sid(1), HEALTH, "current", json!(10))
        .unwrap();
    let update = inspector.refresh(&mut w);
    assert_eq!(update.changed.len(), 1);
    assert_eq!(update.changed[0].0, HEALTH);
    assert_eq!(update.changed[0].1["current"], 10);

    // So does gameplay code, through an ordinary system.
    fn drift(mut q: Query<&mut Transform>) {
        for mut t in &mut q {
            t.position.x += 1.0;
        }
    }
    let mut schedule = Schedule::new();
    schedule.add_system(Stage::Update, drift);
    schedule.run(&mut w);
    let update = inspector.refresh(&mut w);
    assert_eq!(
        update
            .changed
            .iter()
            .map(|c| c.0.as_str())
            .collect::<Vec<_>>(),
        [TRANSFORM]
    );
    assert_eq!(update.changed[0].1["position"]["x"], 2.0);

    // Structural changes show up as added / removed.
    ed.remove_component(&mut w, sid(1), PLAYER).unwrap();
    ed.add_component(&mut w, sid(1), FOLLOWS, None).unwrap();
    let update = inspector.refresh(&mut w);
    assert_eq!(update.removed, [PLAYER]);
    assert_eq!(update.added, [FOLLOWS]);

    // A different target starts over; a deleted target is reported.
    inspector.set_target(Some(sid(3)));
    assert_eq!(inspector.refresh(&mut w).added, [FOLLOWS, HEALTH]);
    ed.despawn(&mut w, &[sid(3)]).unwrap();
    let update = inspector.refresh(&mut w);
    assert!(update.missing);
    assert_eq!(update.removed, [FOLLOWS, HEALTH]);
}

// --- play mode -----------------------------------------------------------------------------

#[test]
fn play_mode_restores_the_scene_and_leaves_the_history_alone() {
    let (mut w, mut ed) = (world(), Editor::new());
    level(&mut ed, &mut w);
    ed.set_field(&mut w, sid(1), HEALTH, "current", json!(60))
        .unwrap();
    // Editor machinery without a SceneId must survive play.
    let machinery = w.spawn();
    let before = w.save_scene();
    let depth = ed.undo_depth();

    ed.enter_play(&w).unwrap();
    assert!(ed.is_playing());
    assert_eq!(ed.enter_play(&w), Err(EditorError::AlreadyPlaying));

    // The game runs: it mutates, spawns and destroys things.
    fn rampage(mut q: Query<&mut Health>) {
        for mut h in &mut q {
            h.current -= 40;
        }
    }
    let mut schedule = Schedule::new();
    schedule.add_system(Stage::Update, rampage);
    schedule.run(&mut w);
    let runtime_only = w.spawn();
    w.spawn_scene_entity();
    ed.despawn(&mut w, &[sid(2)]).unwrap(); // edits apply during play...
    assert!(w.scene_entity(sid(2)).is_none());
    // ...but history is suspended.
    assert!(!ed.undo(&mut w).unwrap());
    assert_eq!(ed.undo_depth(), depth);

    ed.stop_play(&mut w).unwrap();
    assert!(!ed.is_playing());
    assert_eq!(w.save_scene(), before, "the scene is exactly as it was");
    assert!(!w.is_alive(runtime_only), "entities made by play are gone");
    assert!(w.is_alive(machinery), "editor machinery survived");
    assert_eq!(ed.stop_play(&mut w), Err(EditorError::NotPlaying));

    // The undo stack is exactly as before play.
    assert_eq!(ed.undo_depth(), depth);
    ed.undo(&mut w).unwrap();
    assert_eq!(w.get::<Health>(ent(&w, 1)).unwrap().current, 50);
}

// --- property: any history can be undone to the start and redone to the end -------------------

mod history {
    use proptest::prelude::*;

    use super::*;

    #[derive(Clone, Debug)]
    enum Op {
        SetHealth(usize, i32),
        SetX(usize, f32),
        Drag(usize, Vec<f32>),
        AddFollows(usize, usize),
        RemoveFollows(usize),
        RemoveHealth(usize),
        AddHealth(usize),
        Spawn(i32),
        Delete(usize),
        DeletePair(usize, usize),
        Retarget(usize, usize),
        Select(usize),
        Undo,
        Redo,
    }

    fn op() -> impl Strategy<Value = Op> {
        let i = 0usize..8;
        prop_oneof![
            3 => (i.clone(), -100i32..100).prop_map(|(i, v)| Op::SetHealth(i, v)),
            2 => (i.clone(), -10.0f32..10.0).prop_map(|(i, v)| Op::SetX(i, v)),
            1 => (i.clone(), proptest::collection::vec(-5.0f32..5.0, 1..5)).prop_map(|(i, v)| Op::Drag(i, v)),
            2 => (i.clone(), i.clone()).prop_map(|(a, b)| Op::AddFollows(a, b)),
            1 => i.clone().prop_map(Op::RemoveFollows),
            1 => i.clone().prop_map(Op::RemoveHealth),
            1 => i.clone().prop_map(Op::AddHealth),
            2 => (-5i32..5).prop_map(Op::Spawn),
            2 => i.clone().prop_map(Op::Delete),
            1 => (i.clone(), i.clone()).prop_map(|(a, b)| Op::DeletePair(a, b)),
            2 => (i.clone(), i.clone()).prop_map(|(a, b)| Op::Retarget(a, b)),
            1 => i.prop_map(Op::Select),
            3 => Just(Op::Undo),
            2 => Just(Op::Redo),
        ]
    }

    /// The scene entities currently alive, by id.
    fn alive(w: &World) -> Vec<SceneId> {
        let mut ids: Vec<_> = w
            .all_entities()
            .into_iter()
            .filter_map(|e| w.scene_id_of(e))
            .collect();
        ids.sort();
        ids
    }

    fn pick(ids: &[SceneId], i: usize) -> Option<SceneId> {
        (!ids.is_empty()).then(|| ids[i % ids.len()])
    }

    proptest! {
        #[test]
        fn undo_returns_to_the_start_and_redo_to_the_end(ops in proptest::collection::vec(op(), 0..60)) {
            let (mut w, mut ed) = (world(), Editor::new());
            level(&mut ed, &mut w);
            let start = w.save_scene();
            let mut next_id = 100u128;

            for op in ops {
                let ids = alive(&w);
                let before = w.save_scene();
                let depth = ed.undo_depth();
                match op {
                    Op::SetHealth(i, v) => if let Some(id) = pick(&ids, i) {
                        let _ = ed.set_field(&mut w, id, HEALTH, "current", json!(v));
                    },
                    Op::SetX(i, v) => if let Some(id) = pick(&ids, i) {
                        let _ = ed.set_field(&mut w, id, TRANSFORM, "position.x", json!(v));
                    },
                    Op::Drag(i, xs) => if let Some(id) = pick(&ids, i) {
                        ed.begin_gesture();
                        for x in xs { let _ = ed.set_field(&mut w, id, TRANSFORM, "position.x", json!(x)); }
                        ed.end_gesture();
                    },
                    Op::AddFollows(a, b) => if let Some(id) = pick(&ids, a) {
                        let target = pick(&ids, b);
                        let _ = ed.add_component(&mut w, id, FOLLOWS, Some(follows(target)));
                    },
                    Op::RemoveFollows(i) => if let Some(id) = pick(&ids, i) {
                        let _ = ed.remove_component(&mut w, id, FOLLOWS);
                    },
                    Op::RemoveHealth(i) => if let Some(id) = pick(&ids, i) {
                        let _ = ed.remove_component(&mut w, id, HEALTH);
                    },
                    Op::AddHealth(i) => if let Some(id) = pick(&ids, i) {
                        let _ = ed.add_component(&mut w, id, HEALTH, None);
                    },
                    Op::Spawn(v) => {
                        next_id += 1;
                        let _ = ed.spawn_with_id(&mut w, sid(next_id), &[(HEALTH, health(v))]);
                    }
                    Op::Delete(i) => if let Some(id) = pick(&ids, i) {
                        let _ = ed.despawn(&mut w, &[id]);
                    },
                    Op::DeletePair(a, b) => {
                        if let (Some(x), Some(y)) = (pick(&ids, a), pick(&ids, b))
                            && x != y
                        {
                            let _ = ed.despawn(&mut w, &[x, y]);
                        }
                    }
                    Op::Retarget(a, b) => if let (Some(x), Some(y)) = (pick(&ids, a), pick(&ids, b)) {
                        let _ = ed.set_field(&mut w, x, FOLLOWS, "target", json!(y.to_string()));
                    },
                    Op::Select(i) => { let _ = ed.select(&mut w, pick(&ids, i)); }
                    Op::Undo => { let _ = ed.undo(&mut w); }
                    Op::Redo => { let _ = ed.redo(&mut w); }
                }
                // Any edit that was recorded can be undone and redone exactly.
                if ed.undo_depth() > depth {
                    let after = w.save_scene();
                    prop_assert!(ed.undo(&mut w).unwrap());
                    prop_assert_eq!(&w.save_scene(), &before);
                    prop_assert!(ed.redo(&mut w).unwrap());
                    prop_assert_eq!(&w.save_scene(), &after);
                }
                prop_assert!(ed.take_warnings().is_empty());
            }

            // Whatever happened, the whole history unwinds to the start and
            // replays to the end.
            while ed.redo(&mut w).unwrap() {}
            let end = w.save_scene();
            while ed.undo(&mut w).unwrap() {}
            prop_assert_eq!(&w.save_scene(), &start);
            while ed.redo(&mut w).unwrap() {}
            prop_assert_eq!(&w.save_scene(), &end);
        }
    }
}
