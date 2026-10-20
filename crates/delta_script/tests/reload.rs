//! Hot reload: change scripts while the world runs, without losing state.
//!
//! Three builds of one source (see `scripting/samples/Reloadable`):
//!   V1 baseline -> V2 (layouts change, a class is removed, a leak) -> V3
//!   (the class returns).

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use delta::{Entity, Schedule, World};
use delta_script::ScriptHost;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn build(project: &str, variant: Option<&str>, out_name: &str) -> PathBuf {
    let root = workspace_root();
    let out = root.join("target/scripts").join(out_name);
    let mut cmd = Command::new("dotnet");
    cmd.args([
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
    .env("DOTNET_NOLOGO", "1");
    if let Some(v) = variant {
        cmd.arg(format!("-p:Variant={v}"));
    }
    let status = cmd.status().expect(
        "`dotnet` is needed to build the C# side; run inside the devenv shell",
    );
    assert!(status.success(), "building {project} {variant:?} failed");
    out
}

// V1 layouts.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct Vec2 {
    x: f32,
    y: f32,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct StatsV1 {
    hp: i32,
    speed: f32,
    old: i32,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct BodyV1 {
    pos: Vec2,
    mass: i32,
}
// V2/V3 layouts.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct Vec3 {
    x: f32,
    y: f32,
    z: f32,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct StatsV2 {
    hp: i32,
    speed: f64,
    armor: i32,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct BodyV2 {
    pos: Vec3,
    mass: i32,
}
// Unchanged across versions.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct Report {
    version: i32,
    static_runs: i32,
    goner_secret: i32,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct Tally {
    count: i32,
    history: i32,
    extra: i32,
}

fn insert_raw<T: Copy>(world: &mut World, e: Entity, name: &str, value: &T) {
    let id = world.components().id_by_name(name).unwrap();
    assert_eq!(
        world.components().info(id).layout.size(),
        size_of::<T>(),
        "{name}"
    );
    // SAFETY: `T` mirrors the script component (size checked) and is plain data.
    unsafe { world.insert_by_id(e, id, std::ptr::from_ref(value).cast()) }
        .unwrap();
}

fn read_raw<T: Copy>(world: &World, e: Entity, name: &str) -> T {
    let id = world.components().id_by_name(name).unwrap();
    assert_eq!(
        world.components().info(id).layout.size(),
        size_of::<T>(),
        "{name}"
    );
    let p = world
        .get_by_id(e, id)
        .unwrap_or_else(|| panic!("{e:?} has no {name}"));
    // SAFETY: the column holds a `T`-shaped value (size checked).
    unsafe { p.cast::<T>().read() }
}

fn entities_with(world: &World, name: &str) -> Vec<Entity> {
    let id = world.components().id_by_name(name).unwrap();
    let mut out = Vec::new();
    for arch in world.archetypes().iter() {
        if arch.contains(id) {
            out.extend_from_slice(arch.entities());
        }
    }
    out.sort();
    out
}

#[test]
fn scripts_reload_while_the_world_keeps_its_state() {
    let bridge = build("Delta.Scripting", None, "Delta.Scripting");
    let dll = |variant: &str| {
        build(
            "samples/Reloadable",
            Some(variant),
            &format!("Reloadable_{variant}"),
        )
        .join("Reloadable.dll")
    };
    let (v1, v2, v3) = (dll("V1"), dll("V2"), dll("V3"));

    let mut world = World::new();
    let host = ScriptHost::start(
        &mut world,
        &bridge.join("Delta.Scripting.runtimeconfig.json"),
        &bridge.join("Delta.Scripting.dll"),
    )
    .expect("start scripting");

    // ===== V1: set up a world and run it ====================================
    let asm1 = host.load_assembly(&mut world, &v1).expect("load V1");
    let mut schedule = Schedule::new();
    host.install(&mut schedule, &asm1);

    let (s1, s2, b1, rep) =
        (world.spawn(), world.spawn(), world.spawn(), world.spawn());
    insert_raw(
        &mut world,
        s1,
        "Reloadable.Stats",
        &StatsV1 {
            hp: 90,
            speed: 2.5,
            old: 7,
        },
    );
    insert_raw(
        &mut world,
        s2,
        "Reloadable.Stats",
        &StatsV1 {
            hp: 40,
            speed: 1.0,
            old: 8,
        },
    );
    insert_raw(
        &mut world,
        b1,
        "Reloadable.Body",
        &BodyV1 {
            pos: Vec2 { x: 1.5, y: -2.5 },
            mass: 77,
        },
    );
    insert_raw(
        &mut world,
        rep,
        "Reloadable.Report",
        &Report {
            version: 0,
            static_runs: 0,
            goner_secret: 0,
        },
    );

    for _ in 0..5 {
        schedule.run(&mut world);
    }
    let counters = entities_with(&world, "Reloadable.Counter");
    let goners = entities_with(&world, "Reloadable.Goner");
    assert_eq!((counters.len(), goners.len()), (3, 1));
    for &c in &counters {
        let t: Tally = read_raw(&world, c, "Reloadable.Tally");
        assert_eq!(
            t,
            Tally {
                count: 5,
                history: 5,
                extra: -1
            }
        );
    }
    let r: Report = read_raw(&world, rep, "Reloadable.Report");
    assert_eq!((r.version, r.static_runs), (1, 5));
    assert_eq!(
        read_raw::<Report>(&world, goners[0], "Reloadable.Report").goner_secret,
        47
    );
    assert_eq!(host.live_handles(), 4);
    let entity_count = world.entities().len();

    // ===== V1 -> V2 ==========================================================
    let (asm2, report) = host
        .reload(&mut world, &mut schedule, &asm1, &v2)
        .expect("reload to V2");
    eprintln!("V1 -> V2 warnings: {:#?}", report.warnings);

    // The old code is really gone, because Subscriber undid its subscription.
    assert!(report.unloaded, "V1 should have unloaded cleanly");

    // Layout changes migrated by field name, entities untouched.
    let mut migrated = report.migrated.clone();
    migrated.sort();
    assert_eq!(migrated, ["Reloadable.Body", "Reloadable.Stats"]);
    assert_eq!(world.entities().len(), entity_count);
    assert_eq!(
        read_raw::<StatsV2>(&world, s1, "Reloadable.Stats"),
        // Hp kept; Speed changed type so it resets; Old is gone; Armor is new.
        StatsV2 {
            hp: 90,
            speed: 0.0,
            armor: 0
        }
    );
    assert_eq!(read_raw::<StatsV2>(&world, s2, "Reloadable.Stats").hp, 40);
    assert_eq!(
        read_raw::<BodyV2>(&world, b1, "Reloadable.Body"),
        // The nested Vec gained Z; X, Y and Mass moved with their names.
        BodyV2 {
            pos: Vec3 {
                x: 1.5,
                y: -2.5,
                z: 0.0
            },
            mass: 77
        }
    );
    let has = |needle: &str| report.warnings.iter().any(|w| w.contains(needle));
    assert!(
        has("Reloadable.Stats.Speed") && has("type changed"),
        "{:?}",
        report.warnings
    );
    assert!(
        has("Reloadable.Stats.Old") && has("removed"),
        "{:?}",
        report.warnings
    );
    assert!(has("`Label` no longer exists"), "{:?}", report.warnings);

    // Behaviours moved into instances of the new class; Goner has no class.
    assert_eq!(report.restored_behaviours, 3);
    assert_eq!(report.removed_behaviours.len(), 1);
    assert_eq!(report.removed_behaviours[0].0, "Reloadable.Goner");
    assert_eq!(report.removed_behaviours[0].1, goners);
    assert_eq!(entities_with(&world, "Reloadable.Goner"), []);
    assert_eq!(host.live_handles(), 3);
    assert_eq!(
        entities_with(&world, "Reloadable.Counter"),
        counters,
        "same entities"
    );

    for _ in 0..3 {
        schedule.run(&mut world);
    }
    for &c in &counters {
        // Counting continued from the saved state: 5 before + 3 after, and
        // the managed list survived. `Extra` is the new field's initializer.
        let t: Tally = read_raw(&world, c, "Reloadable.Tally");
        assert_eq!(
            t,
            Tally {
                count: 8,
                history: 8,
                extra: 5
            }
        );
    }
    // New code is running, and its statics started over.
    let r: Report = read_raw(&world, rep, "Reloadable.Report");
    assert_eq!((r.version, r.static_runs), (2, 3));

    // ===== V2 -> V3 ==========================================================
    let (_asm3, report) = host
        .reload(&mut world, &mut schedule, &asm2, &v3)
        .expect("reload to V3");
    eprintln!("V2 -> V3 warnings: {:#?}", report.warnings);

    // V2's Leaky system never undid its subscription: the reload notices that
    // the old code could not be unloaded.
    assert!(!report.unloaded, "the leak should have been detected");
    assert!(report.warnings.iter().any(|w| w.contains("still loaded")));

    // The Goner class is back, and with it the state saved when it was removed.
    assert_eq!(report.restored_from_records, 1);
    assert_eq!(report.restored_behaviours, 3);
    assert_eq!(entities_with(&world, "Reloadable.Goner"), goners);
    assert_eq!(host.live_handles(), 4);

    schedule.run(&mut world);
    // Secret was 47 when saved, and counts on from there: 48.
    assert_eq!(
        read_raw::<Report>(&world, goners[0], "Reloadable.Report").goner_secret,
        48
    );
    for &c in &counters {
        assert_eq!(read_raw::<Tally>(&world, c, "Reloadable.Tally").count, 9);
    }
    let r: Report = read_raw(&world, rep, "Reloadable.Report");
    assert_eq!((r.version, r.static_runs), (3, 1));
    assert_eq!(world.entities().len(), entity_count);
}
