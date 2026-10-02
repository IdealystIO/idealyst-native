//! Build this package's own source as the remote bundle (`remote-example-bundle`,
//! the same `src/main.rs` with `--cfg idealyst_stream_guest`) and embed it:
//! the app's built-in copy, used until it reloads a newer one.

use std::path::PathBuf;

include!("../../spike/src/guest_build.rs");

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    // Inside a bundle build there is nothing to embed (and building
    // ourselves again would recurse).
    if std::env::var("CARGO_ENCODED_RUSTFLAGS").is_ok_and(|f| f.contains("idealyst_stream_guest")) {
        return;
    }
    let target_dir = out.join("bundle-target");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let package = "remote-example-bundle";
    let status = guest_build_command(&cargo, &manifest, &target_dir, package)
        .status()
        .unwrap_or_else(|e| panic!("spawn cargo for {package}: {e}"));
    assert!(status.success(), "building {package} for wasm32-unknown-unknown failed");
    std::fs::copy(guest_wasm_path(&target_dir, package), out.join("bundle.wasm")).expect("copy bundle.wasm");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=../bundle/Cargo.toml");
}
