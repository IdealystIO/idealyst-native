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
    // The bundle must compile under THIS crate's name, or its imports of the
    // app's components name a crate the app doesn't have (see the bundle's
    // Cargo.toml). Checked here so a rename breaks the build, not the app.
    // A build script has no CARGO_CRATE_NAME; the app's binary crate is
    // named after its package.
    let app_crate = std::env::var("CARGO_PKG_NAME").unwrap().replace('-', "_");
    let bundle_manifest = std::fs::read_to_string(manifest.join("../bundle/Cargo.toml")).expect("read bundle manifest");
    let lib_name = bundle_manifest
        .split("[lib]")
        .nth(1)
        .and_then(|lib| lib.lines().find_map(|l| l.trim().strip_prefix("name = ")))
        .map(|n| n.trim_matches('"').to_string());
    assert_eq!(
        lib_name.as_deref(),
        Some(app_crate.as_str()),
        "{package}'s [lib] name must be this app's crate name, so its app-component imports match \
         what the app registers"
    );
    let status = guest_build_command(&cargo, &manifest, &target_dir, package)
        .status()
        .unwrap_or_else(|e| panic!("spawn cargo for {package}: {e}"));
    assert!(status.success(), "building {package} for wasm32-unknown-unknown failed");
    std::fs::copy(guest_wasm_path(&target_dir, &app_crate), out.join("bundle.wasm")).expect("copy bundle.wasm");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=../bundle/Cargo.toml");
}
