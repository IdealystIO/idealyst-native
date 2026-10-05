//! Build this package's own library as the remote bundle (the same
//! `src/lib.rs`, with `--cfg idealyst_stream_guest`) and embed it: the
//! app's built-in copy, used until it reloads a newer one. A release build
//! of the same bundle is `idealyst build --remote`.

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
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let (libdir, wasm32) = wasm32_libdir(&rustc);
    if !wasm32 {
        return skip_bundles(&out, &["bundle.wasm"], libdir.as_deref());
    }
    let target_dir = out.join("bundle-target");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    // The bundle is this crate's own library, compiled as a bundle under
    // its own name — what `idealyst build --remote` builds for a release
    // (`[package.metadata.idealyst.remote]` in Cargo.toml).
    let package = std::env::var("CARGO_PKG_NAME").unwrap();
    let app_crate = package.replace('-', "_");
    let status = guest_build_command(&cargo, &manifest, &target_dir, &package)
        .status()
        .unwrap_or_else(|e| panic!("spawn cargo for {package}: {e}"));
    assert!(status.success(), "building {package} for wasm32-unknown-unknown failed");
    std::fs::copy(guest_wasm_path(&target_dir, &app_crate), out.join("bundle.wasm")).expect("copy bundle.wasm");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");
    // And every source the bundle compiled — the framework, idea-ui, … —
    // so editing a library it uses rebuilds the embedded copy.
    for source in bundle_sources(&target_dir, &app_crate) {
        println!("cargo:rerun-if-changed={}", source.display());
    }
}
