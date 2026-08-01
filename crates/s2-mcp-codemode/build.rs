use std::{env, fs, path::PathBuf};

use deno_core::snapshot::{CreateSnapshotOptions, create_snapshot};

fn main() {
    if let Err(error) = build_snapshot() {
        eprintln!("failed to build the Code Mode startup snapshot: {error}");
        std::process::exit(1);
    }
}

fn build_snapshot() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=src/runtime.js");
    let snapshot = create_snapshot(
        CreateSnapshotOptions {
            cargo_manifest_dir: env!("CARGO_MANIFEST_DIR"),
            startup_snapshot: None,
            skip_op_registration: false,
            extensions: Vec::new(),
            extension_transpiler: None,
            with_runtime_cb: None,
        },
        Some(include_str!("src/runtime.js")),
    )?;
    let output_path = PathBuf::from(
        env::var_os("OUT_DIR").ok_or("OUT_DIR was not provided to the build script")?,
    )
    .join("codemode_runtime_snapshot.bin");
    fs::write(output_path, snapshot.output)?;
    for path in snapshot.files_loaded_during_snapshot {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    Ok(())
}
