//! The committed C# mirrors must match what the generator writes now.

use std::{fs, path::Path};

#[test]
fn generated_csharp_is_up_to_date() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripting/Delta.Scripting/Generated/Engine.g.cs");
    let committed = fs::read_to_string(&path).unwrap_or_default();
    assert!(
        committed == vortex_engine::csharp::generate(),
        "{} is stale: run `cargo xtask gen-csharp` and commit the result",
        path.display()
    );
}
