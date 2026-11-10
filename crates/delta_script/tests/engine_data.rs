//! Engine data in C#: `Time` (with the fixed-step rule) and `Input`, both
//! reached through exposed resources, and the layout check that guards the
//! mirrors. One scenario: the runtime lives once per process.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use delta::{Entity, InputMap, InputState, KeyCode, Schedule, Time, World};
use delta_script::ScriptHost;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn build_csharp(project: &str) -> PathBuf {
    let root = workspace_root();
    let name = Path::new(project).file_name().unwrap();
    let out = root.join("target/scripts").join(name);
    let status = Command::new("dotnet")
        .args(["build", "-c", "Release", "-nologo", "-v", "q", "-o"])
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
struct TimeLog {
    update_dt: f32,
    fixed_dt: f32,
    updates: i32,
    fixeds: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct InputLog {
    axis: f32,
    held_frames: i32,
    down_frames: i32,
    up_frames: i32,
}

fn only<T: Copy>(world: &World, name: &str) -> T {
    let id = world.components().id_by_name(name).unwrap();
    let mut found: Vec<Entity> = Vec::new();
    for arch in world.archetypes().iter() {
        if arch.contains(id) {
            found.extend_from_slice(arch.entities());
        }
    }
    assert_eq!(found.len(), 1, "{name}");
    let p = world.get_by_id(found[0], id).unwrap();
    // SAFETY: the column holds a `T`-shaped value (size checked by callers'
    // own structs mirroring the script component).
    unsafe { p.cast::<T>().read() }
}

#[test]
fn scripts_read_time_and_input() {
    let bridge = build_csharp("Delta.Scripting");
    let engine = build_csharp("samples/EngineData");
    let bad = build_csharp("samples/BadMirror");

    let mut world = World::new();
    vortex_engine::register_script_types(&mut world);
    let host = ScriptHost::start(
        &mut world,
        &bridge.join("Delta.Scripting.runtimeconfig.json"),
        &bridge.join("Delta.Scripting.dll"),
    )
    .expect("start scripting");

    // A mirror with its fields in the wrong order is refused, and the error
    // says which field.
    let err = host
        .load_assembly(&mut world, &bad.join("BadMirror.dll"))
        .expect_err("the swapped mirror must not load");
    let text = err.to_string();
    assert!(
        text.contains("vortex::Time")
            && (text.contains("`Delta`") || text.contains("`FixedDelta`")),
        "{text}"
    );

    let asm = host
        .load_assembly(&mut world, &engine.join("EngineData.dll"))
        .expect("load EngineData");
    let mut schedule = Schedule::new();
    schedule.set_fixed_delta(1.0 / 50.0);
    host.install(&mut schedule, &asm);
    world
        .resource_mut::<InputMap>()
        .unwrap()
        .bind("Vertical", 3);

    // ---- 60 frames at 60 Hz with a 50 Hz fixed step -----------------------
    let dt = 1.0f32 / 60.0;
    for frame in 0..60 {
        world.resource_mut::<InputState>().unwrap().begin_frame();
        let input = world.resource_mut::<InputState>().unwrap();
        input.set_axis(3, 1.0);
        match frame {
            10 => input.press(KeyCode::Space as usize),
            20 => input.release(KeyCode::Space as usize),
            _ => {}
        }
        schedule.run_frame(&mut world, dt);
    }

    let t: TimeLog = only(&world, "EngineData.TimeLog");
    assert_eq!(t.update_dt, dt, "DeltaTime in Update");
    assert_eq!(t.fixed_dt, 1.0 / 50.0, "DeltaTime in FixedUpdate");
    // The behaviours spawn at frame 0's PreUpdate sync point, so they see
    // every update and every fixed step.
    assert_eq!(t.updates, 60);
    assert_eq!(t.fixeds, 50, "50 Hz for one simulated second");
    let time = *world.resource::<Time>().unwrap();
    assert_eq!(time.frame, 60);
    assert_eq!(time.in_fixed, 0);

    // ---- input reaches the script the same frame --------------------------
    let i: InputLog = only(&world, "EngineData.InputLog");
    assert_eq!(i.axis, 1.0);
    assert_eq!(i.down_frames, 1, "GetKeyDown is true for exactly one frame");
    assert_eq!(i.held_frames, 10, "frames 10..20 held");
    assert_eq!(i.up_frames, 1);
}
