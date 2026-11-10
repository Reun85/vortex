//! `cargo xtask gen-csharp`: regenerates the C# mirrors of the engine's
//! script-visible types.

use std::{env, fs, path::Path, process::ExitCode};

fn main() -> ExitCode {
    match env::args().nth(1).as_deref() {
        Some("gen-csharp") => {
            let out = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../scripting/Delta.Scripting/Generated/Engine.g.cs");
            fs::create_dir_all(out.parent().unwrap()).unwrap();
            fs::write(&out, vortex_engine::csharp::generate()).unwrap();
            println!("wrote {}", out.display());
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("usage: cargo xtask gen-csharp");
            ExitCode::FAILURE
        }
    }
}
