//! The acceptance test of the Unity-style behaviours plan: a `PlayerMovement`
//! ported from Unity runs, survives a hot reload, and is editable with undo.
//! One scenario, because the runtime lives once per process.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use delta::{Entity, InputMap, InputState, Schedule, World};
use delta_editor::{Editor, Inspector};
use delta_script::ScriptHost;
use serde_json::json;
use vortex_engine::{CharacterBody, Transform};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn build_csharp(project: &str, out_name: &str) -> PathBuf {
    let root = workspace_root();
    let out = root.join("target/scripts").join(out_name);
    let status = Command::new("dotnet")
        .args([
            "build",
            "-c",
            "Release",
            "-nologo",
            "-v",
            "q",
            "--no-incremental",
            "-o",
        ])
        .arg(&out)
        .arg(root.join("scripting").join(project))
        .env("DOTNET_CLI_TELEMETRY_OPTOUT", "1")
        .env("DOTNET_NOLOGO", "1")
        .status()
        .expect("`dotnet` is needed to build the C# side; run inside the devenv shell");
    assert!(status.success(), "building {project} failed");
    out
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct PlayerEvents {
    awakes: i32,
    starts: i32,
    updates: i32,
    started_before_first_update: i32,
    camera_alive: i32,
    instance_hash: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct Destroyed {
    count: i32,
}

const MOVEMENT: &str = "Player.PlayerMovement";
const DT: f32 = 1.0 / 60.0;

fn read<T: Copy>(world: &World, e: Entity, name: &str) -> T {
    let id = world.components().id_by_name(name).unwrap();
    let p = world.get_by_id(e, id).unwrap();
    // SAFETY: `T` mirrors the component's layout (checked on registration).
    unsafe { p.cast::<T>().read() }
}

fn all_with<T: Copy>(world: &World, name: &str) -> Vec<T> {
    let id = world.components().id_by_name(name).unwrap();
    let mut out = Vec::new();
    for arch in world.archetypes().iter() {
        if arch.contains(id) {
            for &e in arch.entities() {
                out.push(read(world, e, name));
            }
        }
    }
    out
}

fn walk_speed(world: &World, e: Entity) -> f64 {
    world
        .field_value(e, MOVEMENT, "walkSpeed")
        .unwrap()
        .as_f64()
        .unwrap()
}

fn frames(world: &mut World, schedule: &mut Schedule, n: usize, vertical: f32) {
    for _ in 0..n {
        world.resource_mut::<InputState>().unwrap().begin_frame();
        world
            .resource_mut::<InputState>()
            .unwrap()
            .set_axis(3, vertical);
        schedule.run_frame(world, DT);
    }
}

/// `MoveTowards` on the one axis the test moves along, in `f32` like C#.
fn expected_run(
    frames: usize,
    speed: f32,
    mut v: f32,
    mut z: f32,
) -> (f32, f32) {
    for _ in 0..frames {
        let target = speed;
        let step = 20.0 * DT;
        v = if (target - v).abs() <= step {
            target
        } else {
            v + step * (target - v).signum()
        };
        z -= v * DT;
    }
    (v, z)
}

#[test]
fn player_movement_runs_reloads_and_edits() {
    let bridge = build_csharp("Delta.Scripting", "Delta.Scripting");
    let v1 = build_csharp("samples/Player", "Player_v1");
    let v2 = build_csharp("samples/Player", "Player_v2");

    let mut world = World::new();
    vortex_engine::register_script_types(&mut world);
    let host = ScriptHost::start(
        &mut world,
        &bridge.join("Delta.Scripting.runtimeconfig.json"),
        &bridge.join("Delta.Scripting.dll"),
    )
    .expect("start scripting");
    let asm = host
        .load_assembly(&mut world, &v1.join("Player.dll"))
        .expect("load Player");
    let mut schedule = Schedule::new();
    vortex_engine::add_engine_systems(&mut schedule);
    host.install(&mut schedule, &asm);
    world
        .resource_mut::<InputMap>()
        .unwrap()
        .bind("Vertical", 3);
    world
        .resource_mut::<InputMap>()
        .unwrap()
        .bind("Horizontal", 0);

    let mut editor = Editor::new();
    let camera = editor.spawn(&mut world, &[]).unwrap();
    let player = editor.spawn(&mut world, &[]).unwrap();
    editor
        .add_component(&mut world, player, MOVEMENT, None)
        .expect("add PlayerMovement");
    editor
        .set_field(
            &mut world,
            player,
            MOVEMENT,
            "cameraEntity",
            json!(camera.to_string()),
        )
        .expect("point at the camera");
    let e = world.scene_entity(player).unwrap();

    // ---- 1. lifecycle ------------------------------------------------------
    assert!(world.components().id_by_name("vortex::Transform").is_some());
    let _: Transform = read(&world, e, "vortex::Transform");
    let _: CharacterBody = read(&world, e, "vortex::CharacterBody");
    let ev: PlayerEvents = read(&world, e, "Player.PlayerEvents");
    assert_eq!((ev.awakes, ev.starts, ev.updates), (1, 0, 0));

    // ---- 2. engine data ----------------------------------------------------
    frames(&mut world, &mut schedule, 60, 1.0);
    let ev: PlayerEvents = read(&world, e, "Player.PlayerEvents");
    assert_eq!((ev.awakes, ev.starts, ev.updates), (1, 1, 60));
    assert_eq!(ev.started_before_first_update, 1);
    let t: Transform = read(&world, e, "vortex::Transform");
    let (v, z) = expected_run(60, 5.0, 0.0, 0.0);
    assert!((t.position.z - z).abs() < 1e-3, "{} vs {z}", t.position.z);
    let hash_before = ev.instance_hash;

    // ---- 3. hot reload keeps private state --------------------------------
    let (asm2, report) = host
        .reload(&mut world, &mut schedule, &asm, &v2.join("Player.dll"))
        .expect("reload");
    assert!(report.unloaded, "{report:?}");
    assert_eq!(report.restored_behaviours, 1, "{report:?}");
    frames(&mut world, &mut schedule, 1, 1.0);
    let ev: PlayerEvents = read(&world, e, "Player.PlayerEvents");
    assert_eq!((ev.awakes, ev.starts, ev.updates), (1, 1, 61));
    assert_ne!(ev.instance_hash, hash_before, "a new instance");
    let body: CharacterBody = read(&world, e, "vortex::CharacterBody");
    assert!(
        (body.velocity.z + v).abs() < 1e-4,
        "continues from the saved velocity: {} vs {v}",
        body.velocity.z
    );
    let (_, z61) = expected_run(1, 5.0, v, z);
    let t: Transform = read(&world, e, "vortex::Transform");
    assert!((t.position.z - z61).abs() < 1e-3);

    // ---- 4. editor ---------------------------------------------------------
    let mut inspector = Inspector::new();
    inspector.set_target(Some(player));
    let update = inspector.refresh(&mut world);
    assert!(update.added.iter().any(|n| n == MOVEMENT), "{update:?}");
    let schema = world.component_schema(MOVEMENT).unwrap();
    let walk = schema
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "walkSpeed")
        .unwrap_or_else(|| panic!("walkSpeed in {schema}"));
    let range: Vec<f64> = walk["range"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_f64().unwrap())
        .collect();
    assert_eq!(range, [0.0, 20.0]);

    let hash = |w: &World| -> i32 {
        read::<PlayerEvents>(
            w,
            w.scene_entity(player).unwrap(),
            "Player.PlayerEvents",
        )
        .instance_hash
    };
    let before = hash(&world);
    editor
        .set_field(&mut world, player, MOVEMENT, "walkSpeed", json!(10.0))
        .unwrap();
    frames(&mut world, &mut schedule, 60, 1.0);
    let body: CharacterBody = read(&world, e, "vortex::CharacterBody");
    assert!((body.velocity.z + 10.0).abs() < 1e-3, "{:?}", body.velocity);
    assert_eq!(hash(&world), before, "set_field edits the live object");

    assert!(editor.undo(&mut world).unwrap());
    assert_eq!(walk_speed(&world, e), 5.0);
    frames(&mut world, &mut schedule, 60, 1.0);
    let body: CharacterBody = read(&world, e, "vortex::CharacterBody");
    assert!((body.velocity.z + 5.0).abs() < 1e-3, "{:?}", body.velocity);

    editor.enter_play(&world).unwrap();
    editor
        .set_field(&mut world, player, MOVEMENT, "walkSpeed", json!(12.0))
        .unwrap();
    editor.stop_play(&mut world).unwrap();
    let e = world.scene_entity(player).unwrap();
    assert_eq!(walk_speed(&world, e), 5.0, "play mode restores the edit");

    // ---- 5. scenes ---------------------------------------------------------
    let want = json!(camera.to_string());
    assert_eq!(
        world.field_value(e, MOVEMENT, "cameraEntity").unwrap(),
        want
    );
    let saved = world.save_scene();
    for s in [player, camera] {
        let ent = world.scene_entity(s).unwrap();
        world.despawn(ent).unwrap();
    }
    world.load_scene(&saved).expect("load scene");
    let e = world.scene_entity(player).unwrap();
    assert_eq!(
        world.field_value(e, MOVEMENT, "cameraEntity").unwrap(),
        want
    );
    let cam: Option<Entity> = world.scene_entity(camera);
    assert!(cam.is_some());

    // ---- 1 (end). OnDestroy once on despawn -------------------------------
    // Earlier despawns (play stop, scene reload) also ran `OnDestroy`.
    let destroyed = |w: &World| -> i32 {
        all_with::<Destroyed>(w, "Player.Destroyed")
            .iter()
            .map(|d| d.count)
            .sum()
    };
    let before = destroyed(&world);
    editor.despawn(&mut world, &[player]).unwrap();
    assert_eq!(destroyed(&world), before + 1, "OnDestroy once on despawn");
    let _ = asm2;
}
