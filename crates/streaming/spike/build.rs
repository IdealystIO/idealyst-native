//! Build `spike-guest` for wasm32 and embed it — the app's built-in copy of
//! the bundle, used when no bundle server is reachable.

use std::path::PathBuf;

include!("src/guest_build.rs");

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let target_dir = out.join("guest-target");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());

    let status = guest_build_command(&cargo, &manifest, &target_dir)
        .status()
        .expect("spawn cargo for spike-guest");
    assert!(status.success(), "building spike-guest for wasm32-unknown-unknown failed");
    std::fs::copy(guest_wasm_path(&target_dir), out.join("spike_guest.wasm")).expect("copy spike_guest.wasm");

    println!("cargo:rerun-if-changed=src/guest_build.rs");
    for dir in GUEST_SOURCES {
        println!("cargo:rerun-if-changed={}", manifest.join(dir).display());
    }
}
