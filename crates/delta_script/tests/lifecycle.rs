//! Behaviour lifecycle: `Awake`, `Start`, `Update`, `OnDestroy` in Unity's
//! order, driven by component hooks and the start pass, plus required
//! components. One scenario, because the runtime lives once per process.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    process::Command,
};

use delta::{Entity, Schedule, Stage, World};
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

const AWAKE: i32 = 0;
const START: i32 = 1;
const UPDATE: i32 = 2;
const DESTROY: i32 = 3;
const AFTER_RELOAD: i32 = 4;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct JournalEntry {
    seq: i32,
    frame: i32,
    kind: i32,
    who: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Trigger {
    spawn_in_pre_update: i32,
    spawn_in_update: i32,
    despawn_in_late_update: u64,
    add_then_remove: i32,
    spawn_spawner: i32,
    spawn_needy: i32,
}

const IDLE: Trigger = Trigger {
    spawn_in_pre_update: 0,
    spawn_in_update: 0,
    despawn_in_late_update: u64::MAX,
    add_then_remove: 0,
    spawn_spawner: 0,
    spawn_needy: 0,
};

fn component(world: &World, name: &str) -> delta::ComponentId {
    world
        .components()
        .id_by_name(name)
        .unwrap_or_else(|| panic!("{name} is not registered"))
}

fn entities_with(world: &World, name: &str) -> Vec<Entity> {
    let id = component(world, name);
    let mut out = Vec::new();
    for arch in world.archetypes().iter() {
        if arch.contains(id) {
            out.extend_from_slice(arch.entities());
        }
    }
    out.sort();
    out
}

/// The journal entries not seen before, in logging order.
fn new_entries(world: &World, seen: &mut HashSet<Entity>) -> Vec<JournalEntry> {
    let id = component(world, "Lifecycle.JournalEntry");
    let mut out = Vec::new();
    for e in entities_with(world, "Lifecycle.JournalEntry") {
        if seen.insert(e) {
            let p = world.get_by_id(e, id).unwrap();
            // SAFETY: the column holds a `JournalEntry`-shaped value.
            out.push(unsafe { p.cast::<JournalEntry>().read() });
        }
    }
    out.sort_by_key(|e| e.seq);
    out
}

fn events(entries: &[JournalEntry]) -> Vec<(i32, i32, i32)> {
    entries.iter().map(|e| (e.frame, e.kind, e.who)).collect()
}

fn set_trigger(world: &mut World, e: Entity, trigger: Trigger) {
    let id = component(world, "Lifecycle.Trigger");
    // SAFETY: `Trigger` mirrors the script component (checked on insert).
    unsafe { world.insert_by_id(e, id, (&raw const trigger).cast()) }.unwrap();
}

#[test]
fn behaviours_follow_the_unity_lifecycle() {
    let bridge = build_csharp("Delta.Scripting");
    let dir = build_csharp("samples/Lifecycle");
    let dll = dir.join("Lifecycle.dll");

    let mut world = World::new();
    vortex_engine::register_script_types(&mut world);
    let host = ScriptHost::start(
        &mut world,
        &bridge.join("Delta.Scripting.runtimeconfig.json"),
        &bridge.join("Delta.Scripting.dll"),
    )
    .expect("start scripting");
    let asm = host
        .load_assembly(&mut world, &dll)
        .expect("load Lifecycle");
    let mut schedule = Schedule::new();
    host.install(&mut schedule, &asm);

    assert_eq!(
        world
            .components()
            .info(component(&world, "Lifecycle.Needy"))
            .requires
            .len(),
        2
    );

    let control = world.spawn();
    set_trigger(&mut world, control, IDLE);
    let mut seen = HashSet::new();

    // Frame 1: nothing to do.
    schedule.run(&mut world);
    assert!(new_entries(&world, &mut seen).is_empty());

    // ---- spawned in PreUpdate: Awake at the sync point, then Start and
    // Update in the same frame.
    set_trigger(
        &mut world,
        control,
        Trigger {
            spawn_in_pre_update: 1,
            ..IDLE
        },
    );
    schedule.run(&mut world); // frame 2
    let p1 = entities_with(&world, "Lifecycle.Probe")[0];
    let (p1_id, _) = (p1.index() as i32, ());
    assert_eq!(
        events(&new_entries(&world, &mut seen)),
        [(2, AWAKE, p1_id), (2, START, p1_id), (2, UPDATE, p1_id)]
    );

    // ---- spawned in Update (after the Update dispatcher ran): Awake now,
    // Start and the first Update next frame, Start first.
    set_trigger(
        &mut world,
        control,
        Trigger {
            spawn_in_update: 1,
            ..IDLE
        },
    );
    schedule.run(&mut world); // frame 3
    let p2 = *entities_with(&world, "Lifecycle.Probe")
        .iter()
        .find(|&&e| e != p1)
        .unwrap();
    let p2_id = p2.index() as i32;
    assert_eq!(
        events(&new_entries(&world, &mut seen)),
        [(3, UPDATE, p1_id), (3, AWAKE, p2_id)]
    );
    schedule.run(&mut world); // frame 4
    assert_eq!(
        events(&new_entries(&world, &mut seen)),
        [(4, START, p2_id), (4, UPDATE, p1_id), (4, UPDATE, p2_id)]
    );

    // ---- despawned in LateUpdate: OnDestroy at the sync point, and no
    // further Update.
    set_trigger(
        &mut world,
        control,
        Trigger {
            despawn_in_late_update: p1.to_bits(),
            ..IDLE
        },
    );
    schedule.run(&mut world); // frame 5
    assert_eq!(
        events(&new_entries(&world, &mut seen)),
        [(5, UPDATE, p1_id), (5, UPDATE, p2_id), (5, DESTROY, p1_id)]
    );
    assert!(!world.is_alive(p1));
    schedule.run(&mut world); // frame 6
    assert_eq!(
        events(&new_entries(&world, &mut seen)),
        [(6, UPDATE, p2_id)]
    );

    // ---- added and removed in one command buffer: both run, in order, and
    // the instance never starts or updates.
    set_trigger(
        &mut world,
        control,
        Trigger {
            add_then_remove: 1,
            ..IDLE
        },
    );
    let before = world.entities().len();
    schedule.run(&mut world); // frame 7
    let log = events(&new_entries(&world, &mut seen));
    assert_eq!(log.len(), 3, "{log:?}");
    assert_eq!(log[0], (7, UPDATE, p2_id));
    let p3_id = log[1].2;
    assert_eq!(log[1], (7, AWAKE, p3_id));
    assert_eq!(log[2], (7, DESTROY, p3_id));
    assert_ne!(p3_id, p2_id);
    schedule.run(&mut world); // frame 8
    assert_eq!(
        events(&new_entries(&world, &mut seen)),
        [(8, UPDATE, p2_id)]
    );
    let _ = before;

    // ---- Awake may spawn: the entity exists once the stage's sync point
    // has run.
    set_trigger(
        &mut world,
        control,
        Trigger {
            spawn_spawner: 1,
            ..IDLE
        },
    );
    schedule.run_stage(Stage::PreUpdate, &mut world);
    // (The frame counter did not run this frame; stages are driven by hand.)
    assert_eq!(entities_with(&world, "Lifecycle.Marker").len(), 1);
    for stage in [
        Stage::Update,
        Stage::LateUpdate,
        Stage::PostUpdate,
        Stage::Extract,
    ] {
        schedule.run_stage(stage, &mut world);
    }
    let _ = new_entries(&world, &mut seen);

    // ---- a required component arrives in the same move, zeroed.
    let archetypes = world.archetypes().len();
    set_trigger(
        &mut world,
        control,
        Trigger {
            spawn_needy: 1,
            ..IDLE
        },
    );
    schedule.run(&mut world);
    let needy = entities_with(&world, "Lifecycle.Needy");
    assert_eq!(needy.len(), 1);
    let read_i32 = |name: &str| {
        let id = component(&world, name);
        let p = world.get_by_id(needy[0], id).expect(name);
        // SAFETY: both components are a single i32.
        unsafe { p.cast::<i32>().read() }
    };
    assert_eq!(read_i32("Lifecycle.Health"), 0);
    assert_eq!(read_i32("Lifecycle.Mana"), 0);
    assert_eq!(
        world.archetypes().len(),
        archetypes + 1,
        "Needy, Health and Mana arrive in one archetype move"
    );
    let _ = new_entries(&world, &mut seen);

    // ---- despawning from outside a system also runs OnDestroy.
    world.despawn(p2).unwrap();
    assert_eq!(
        events(&new_entries(&world, &mut seen))
            .iter()
            .map(|e| (e.1, e.2))
            .collect::<Vec<_>>(),
        [(DESTROY, p2_id)]
    );

    // ---- a hot reload rebuilds instances without Awake or Start.
    set_trigger(
        &mut world,
        control,
        Trigger {
            spawn_in_pre_update: 1,
            ..IDLE
        },
    );
    schedule.run(&mut world);
    let p4 = entities_with(&world, "Lifecycle.Probe")[0];
    let p4_id = p4.index() as i32;
    let _ = new_entries(&world, &mut seen);
    let (_asm, report) = host
        .reload(&mut world, &mut schedule, &asm, &dll)
        .expect("reload");
    // The Probe, the Needy and the Spawner are alive.
    assert_eq!(report.restored_behaviours, 3, "{report:?}");
    let after = new_entries(&world, &mut seen);
    assert!(
        after.iter().all(|e| e.kind == AFTER_RELOAD),
        "reload must not Awake or Start: {after:?}"
    );
    assert!(after.iter().any(|e| e.who == p4_id));
    schedule.run(&mut world);
    let kinds: Vec<_> = new_entries(&world, &mut seen)
        .iter()
        .filter(|e| e.who == p4_id)
        .map(|e| e.kind)
        .collect();
    assert_eq!(kinds, [UPDATE], "started state survived the reload");
}
