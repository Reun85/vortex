//! Does the runtime start, and can Rust call managed code?

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use delta_script::DotnetHost;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// Builds the C# project and returns the output directory.
fn build_csharp(project: &str) -> PathBuf {
    let root = workspace_root();
    let out = root.join("target/scripts").join(project);
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

#[test]
fn rust_calls_managed_code() {
    let out = build_csharp("Delta.Scripting");
    let host = DotnetHost::get_or_init(
        &out.join("Delta.Scripting.runtimeconfig.json"),
    )
    .expect("start the runtime");
    // SAFETY: `Bridge.Add` is `[UnmanagedCallersOnly] int Add(int, int)`.
    let add: unsafe extern "C" fn(i32, i32) -> i32 = unsafe {
        host.function(
            &out.join("Delta.Scripting.dll"),
            "Delta.Bridge, Delta.Scripting",
            "Add",
        )
    }
    .expect("resolve Bridge.Add");
    // SAFETY: calling the resolved managed function.
    assert_eq!(unsafe { add(40, 2) }, 42);
}
