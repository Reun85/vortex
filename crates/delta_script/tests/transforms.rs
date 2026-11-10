//! Rust components in C#: a generated mirror binds to the same column, so
//! C# writes are visible to Rust (with change detection) and the reverse.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use delta::{
    Entity,
    Query,
    Schedule,
    Stage,
    World,
    query::{QueryDesc, QueryState},
};
use delta_script::ScriptHost;
use vortex_engine::Transform;
use vortex_math::Vec3;

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

fn at(x: f32) -> Transform {
    Transform {
        position: Vec3::new(x, 0.0, 0.0),
        ..Transform::default()
    }
}

#[test]
fn csharp_and_rust_share_transform_columns() {
    let bridge = build_csharp("Delta.Scripting");
    let dir = build_csharp("samples/Transforms");

    let mut world = World::new();
    vortex_engine::register_script_types(&mut world);
    let host = ScriptHost::start(
        &mut world,
        &bridge.join("Delta.Scripting.runtimeconfig.json"),
        &bridge.join("Delta.Scripting.dll"),
    )
    .expect("start scripting");
    let asm = host
        .load_assembly(&mut world, &dir.join("Transforms.dll"))
        .expect("load Transforms");
    let frozen = world.components().id_by_name("Transforms.Frozen").unwrap();

    // ---- 10k Rust transforms moved by a C# chunk-tier system -------------
    const N: usize = 10_000;
    let mut ents = Vec::new();
    for i in 0..N {
        let e = world.spawn();
        world.insert(e, at(i as f32)).unwrap();
        if i % 100 == 0 {
            // SAFETY: a tag has no data.
            unsafe { world.insert_by_id(e, frozen, std::ptr::dangling()) }
                .unwrap();
        }
        ents.push(e);
    }
    let mut schedule = Schedule::new();
    host.install(&mut schedule, &asm);
    let transform = world.components().id_of::<Transform>().unwrap();
    let before = world.change_tick();
    schedule.run(&mut world);

    let mut changed = QueryState::new(QueryDesc {
        changed: vec![transform],
        ..Default::default()
    });
    let mut rows = 0;
    changed.for_each_chunk(&world, before, |c| rows += c.len());
    // The Walker's entity also exists after frame one, with its own write.
    let walker_rows = 1;
    assert_eq!(
        rows,
        N - N / 100 + walker_rows,
        "Changed<Transform> sees exactly the rows C# wrote"
    );
    for (i, &e) in ents.iter().enumerate() {
        let moved = if i % 100 == 0 { 0.0 } else { 1.0 };
        assert_eq!(
            world.get::<Transform>(e).unwrap().position.x,
            i as f32 + moved
        );
    }

    // ---- alternating writers on one entity for 100 frames -----------------
    // The Walker (a Behaviour using Get<Transform>()) adds 1 per Update, and
    // a Rust system adds 10 in PostUpdate; each must see the other's writes.
    fn rust_step(mut q: Query<&mut Transform>) {
        for mut t in &mut q {
            if t.position.y == 123.0 {
                t.position.x += 10.0;
            }
        }
    }
    schedule.add_system(Stage::PostUpdate, rust_step);
    let walker: Entity = {
        let w = world.components().id_by_name("Transforms.Walker").unwrap();
        world
            .archetypes()
            .iter()
            .filter(|a| a.contains(w))
            .flat_map(|a| a.entities().iter().copied())
            .next()
            .expect("the walker spawned")
    };
    world.get_mut::<Transform>(walker).unwrap().position =
        Vec3::new(0.0, 123.0, 0.0);
    for _ in 0..100 {
        schedule.run(&mut world);
    }
    assert_eq!(
        world.get::<Transform>(walker).unwrap().position,
        Vec3::new(1100.0, 123.0, 0.0)
    );
}
