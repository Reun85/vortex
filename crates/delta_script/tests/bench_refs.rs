//! Phase 5 measurement, not a correctness test: ns per component access for
//! `Get<T>()`, `ComponentRef<T>` and the chunk tier over 10k entities. Run
//! with `--ignored --nocapture`; numbers go in `Plan/phase5-measurements.md`.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use delta::{Schedule, World};
use delta_script::ScriptHost;

fn build_csharp(project: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let name = Path::new(project).file_name().unwrap();
    let out = root.join("target/scripts").join(name);
    let status = Command::new("dotnet")
        .args(["build", "-c", "Release", "-nologo", "-v", "q", "-o"])
        .arg(&out)
        .arg(root.join("scripting").join(project))
        .status()
        .expect("`dotnet` is needed");
    assert!(status.success());
    out
}

#[repr(C)]
#[derive(Clone, Copy)]
struct BenchResult {
    get_ns: f64,
    ref_ns: f64,
    chunk_ns: f64,
}

#[test]
#[ignore = "measurement; run with --release --ignored --nocapture"]
fn component_access_cost() {
    let bridge = build_csharp("Delta.Scripting");
    let dir = build_csharp("samples/Bench");
    let mut world = World::new();
    vortex_engine::register_script_types(&mut world);
    let host = ScriptHost::start(
        &mut world,
        &bridge.join("Delta.Scripting.runtimeconfig.json"),
        &bridge.join("Delta.Scripting.dll"),
    )
    .unwrap();
    let asm = host
        .load_assembly(&mut world, &dir.join("Bench.dll"))
        .unwrap();
    let mut schedule = Schedule::new();
    host.install(&mut schedule, &asm);
    for _ in 0..3 {
        schedule.run(&mut world);
    }
    let id = world.components().id_by_name("Bench.Result").unwrap();
    let e = world
        .archetypes()
        .iter()
        .find(|a| a.contains(id))
        .unwrap()
        .entities()[0];
    // SAFETY: `BenchResult` mirrors the C# `Result` struct.
    let r =
        unsafe { world.get_by_id(e, id).unwrap().cast::<BenchResult>().read() };
    println!(
        "ns/access over 10k entities: Get<T> {:.1}, ComponentRef<T> {:.1}, chunk {:.1}",
        r.get_ns, r.ref_ns, r.chunk_ns
    );
}
