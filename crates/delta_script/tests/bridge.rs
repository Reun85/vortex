//! The CoreCLR bridge end to end: a C# assembly declares components and
//! systems, and the world runs them.
//!
//! The runtime and the C# registries live once per process, so everything
//! happens in one test.

use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

use delta::{
    Entity,
    FieldType,
    Schedule,
    Tick,
    World,
    query::{QueryDesc, QueryState},
};
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
#[derive(Clone, Copy, Debug, PartialEq)]
struct Vec2 {
    x: f32,
    y: f32,
}

fn insert_raw<T: Copy>(world: &mut World, e: Entity, name: &str, value: &T) {
    let id = world
        .components()
        .id_by_name(name)
        .unwrap_or_else(|| panic!("component {name} is not registered"));
    assert_eq!(world.components().info(id).layout.size(), size_of::<T>());
    // SAFETY: `T` mirrors the script component's layout (checked above) and
    // is plain data.
    unsafe { world.insert_by_id(e, id, std::ptr::from_ref(value).cast()) }
        .unwrap();
}

fn read_raw<T: Copy>(world: &World, e: Entity, name: &str) -> T {
    let id = world.components().id_by_name(name).unwrap();
    let p = world.get_by_id(e, id).expect("component present");
    // SAFETY: the column holds a `T`-shaped value (layout checked on insert).
    unsafe { p.cast::<T>().read() }
}

#[test]
fn csharp_scripts_drive_the_world() {
    let bridge_dir = build_csharp("Delta.Scripting");
    let movers_dir = build_csharp("samples/Movers");

    let mut world = World::new();
    let host = ScriptHost::start(
        &mut world,
        &bridge_dir.join("Delta.Scripting.runtimeconfig.json"),
        &bridge_dir.join("Delta.Scripting.dll"),
    )
    .expect("start scripting");

    // --- loading registers components, reflection data and systems -------
    let assembly = host
        .load_assembly(&mut world, &movers_dir.join("Movers.dll"))
        .expect("load Movers");
    let mut names: Vec<_> =
        assembly.systems.iter().map(|s| s.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "Movers.MoveSystem",
            "Movers.SpawnSpinners",
            "Movers.Spinner.LateUpdate",
            "Movers.Spinner.Update",
        ]
    );
    for name in [
        "Movers.Position",
        "Movers.Velocity",
        "Movers.Counter",
        "Movers.Frozen",
        "Movers.Link",
        "Movers.Spinner",
    ] {
        assert!(world.components().id_by_name(name).is_some(), "{name}");
    }

    // Script components are reflected like native ones: field names, byte
    // offsets, an entity reference, and a nested struct by id.
    let link = world.components().id_by_name("Movers.Link").unwrap();
    let link_reflect = world.components().info(link).reflect.unwrap();
    let link_type = world.reflection().get(link_reflect);
    assert_eq!(link_type.size, 24);
    assert_eq!(link_type.field("Target").unwrap().ty, FieldType::Entity);
    assert_eq!(link_type.field("Target").unwrap().offset, 0);
    assert_eq!(link_type.field("Strength").unwrap().ty, FieldType::U16);
    let FieldType::Struct(nested) = link_type.field("Offset").unwrap().ty
    else {
        panic!("Offset should be a nested struct");
    };
    assert_eq!(&*world.reflection().get(nested).name, "Movers.Position");
    assert_eq!(world.reflection().get(nested).fields.len(), 2);
    // Behaviour classes are components too, holding one GC handle per row.
    let spinner = world.components().id_by_name("Movers.Spinner").unwrap();
    assert_eq!(world.components().info(spinner).layout.size(), 8);

    // --- 10k entities, moved by a C# system through the chunk tier --------
    const N: usize = 10_000;
    const FROZEN_EVERY: usize = 100;
    let mut ents = Vec::with_capacity(N);
    for i in 0..N {
        let e = world.spawn();
        insert_raw(
            &mut world,
            e,
            "Movers.Position",
            &Vec2 {
                x: i as f32,
                y: 0.0,
            },
        );
        insert_raw(&mut world, e, "Movers.Velocity", &Vec2 { x: 1.0, y: 0.5 });
        if i % FROZEN_EVERY == 0 {
            insert_raw(&mut world, e, "Movers.Frozen", &());
        }
        ents.push(e);
    }

    let mut schedule = Schedule::new();
    host.install(&mut schedule, &assembly);

    let pos_id = world.components().id_by_name("Movers.Position").unwrap();
    let before = world.change_tick();
    schedule.run(&mut world);
    // The chunk-tier write stamped the moved rows as changed.
    let mut changed = QueryState::new(QueryDesc {
        changed: vec![pos_id],
        ..Default::default()
    });
    let mut rows = 0;
    changed.for_each_chunk(&world, before, |c| rows += c.len());
    assert_eq!(
        rows,
        N - N / FROZEN_EVERY,
        "only unfrozen rows were written"
    );

    const FRAMES: u32 = 200;
    let started = Instant::now();
    for _ in 1..FRAMES {
        schedule.run(&mut world);
    }
    let per_frame = started.elapsed() / (FRAMES - 1);
    eprintln!("10k entities, C# chunk-tier system: {per_frame:?} per frame");
    assert!(
        per_frame.as_secs_f64() < 0.008,
        "{per_frame:?} per frame is not frame rate for 10k entities"
    );

    for (i, &e) in ents.iter().enumerate() {
        let p: Vec2 = read_raw(&world, e, "Movers.Position");
        let moved = if i % FROZEN_EVERY == 0 {
            0.0
        } else {
            FRAMES as f32
        };
        assert_eq!(p.x, i as f32 + moved, "entity {i} x");
        assert_eq!(p.y, 0.5 * moved, "entity {i} y");
    }

    // --- Behaviour classes: dispatch, GC handles, drop ---------------------
    // SpawnSpinners ran in frame 1; each frame adds 1 (Update) + 100
    // (LateUpdate) to every spinner's Counter.
    let counter_id = world.components().id_by_name("Movers.Counter").unwrap();
    let mut spinners = Vec::new();
    for arch in world.archetypes().iter() {
        if arch.contains(spinner) {
            spinners.extend_from_slice(arch.entities());
        }
    }
    assert_eq!(spinners.len(), 3);
    for &e in &spinners {
        let c: i32 = read_raw(&world, e, "Movers.Counter");
        assert_eq!(c, FRAMES as i32 * 101);
    }
    assert!(world.components().info(counter_id).reflect.is_some());
    assert_eq!(host.live_handles(), 3);

    // Despawning drops the column value, which frees the managed object.
    world.despawn(spinners[0]).unwrap();
    assert_eq!(host.live_handles(), 2);
    for &e in &spinners[1..] {
        world.despawn(e).unwrap();
    }
    assert_eq!(host.live_handles(), 0);

    // --- script components save into scenes like any other ------------------
    let target = world.spawn_with_scene_id(delta::SceneId(1));
    let linker = world.spawn_with_scene_id(delta::SceneId(2));
    let value: (u64, Vec2, u16, [u8; 6]) =
        (target.to_bits(), Vec2 { x: 1.5, y: -2.0 }, 7, [0; 6]);
    insert_raw(&mut world, linker, "Movers.Link", &value);
    let json = world.save_scene();
    assert!(json.contains("\"Movers.Link\""), "{json}");
    assert!(json.contains("\"Strength\": 7"), "{json}");
    assert!(
        json.contains(&format!("\"Target\": \"{}\"", delta::SceneId(1))),
        "{json}"
    );
    let _ = Tick::new(0);
}
