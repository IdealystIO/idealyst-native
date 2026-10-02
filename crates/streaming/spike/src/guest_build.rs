// How to build `spike-guest` to wasm32. Shared by `build.rs` (via
// `include!`, so it may use only std) and the `stream-serve` binary, so the
// bundle the server streams is built exactly like the one compiled into
// the app.

/// Sources whose edits change the bundle, relative to the stream-spike
/// crate directory.
pub const GUEST_SOURCES: &[&str] =
    &["guest/src", "guest/Cargo.toml", "camera/src", "../guest/src", "../abi/src", "../macros/src"];

/// Sources of the full-framework prototype bundle (`spike-fullguest`) that
/// live in this directory tree. It also depends on the framework crates;
/// cargo's own fingerprinting covers those inside the nested build.
pub const FULL_GUEST_SOURCES: &[&str] = &[
    "fullguest/src",
    "fullguest/Cargo.toml",
    "components/src",
    "kernelguest/src",
    "kernelguest/Cargo.toml",
    "remoteguest/src",
    "remoteguest/Cargo.toml",
    "../../runtime/vocabulary/src",
    "../../runtime/scene/src",
    "../../runtime/shared/src",
    "../abi/src",
    "../../runtime/world/src",
];

/// A `cargo build` of bundle crate `package` for wasm32 into `target_dir`.
///
/// Uses its own target dir: sharing an outer build's would deadlock on
/// cargo's build-directory lock. Flags the outer build exports for the HOST
/// target are stripped so they do not leak into the wasm build.
pub fn guest_build_command(
    cargo: &str,
    crate_dir: &std::path::Path,
    target_dir: &std::path::Path,
    package: &str,
) -> std::process::Command {
    let mut cmd = std::process::Command::new(cargo);
    cmd.current_dir(crate_dir)
        .args(["build", "--release", "--target", "wasm32-unknown-unknown", "-p", package])
        .arg("--target-dir")
        .arg(target_dir)
        .env_remove("RUSTFLAGS")
        // rustc's wasm32 default stack is 1 MB, which makes the module's
        // initial memory 17 pages — and the interpreter zeroes all of it at
        // instantiate. Measured: ~0.9 ms of a ~1.06 ms load. UI code does
        // not recurse deeply; 64 KB is a conventional embedded wasm stack.
        //
        // `--cfg idealyst_stream_guest` marks this as a BUNDLE build:
        // `#[host_fn]` emits stubs instead of real functions under it. It is
        // a build flag rather than a cargo feature so it can never reach an
        // app build through feature unification.
        //
        // (`CARGO_ENCODED_RUSTFLAGS` is 0x1f-separated and replaces the outer
        // build's host flags rather than adding to them.)
        .env("CARGO_ENCODED_RUSTFLAGS", "-Clink-arg=-zstack-size=65536\x1f--cfg=idealyst_stream_guest")
        .env_remove("CARGO_BUILD_TARGET")
        .env_remove("CARGO_TARGET_DIR");
    cmd
}

/// Where [`guest_build_command`] leaves package `package`'s bundle.
pub fn guest_wasm_path(target_dir: &std::path::Path, package: &str) -> std::path::PathBuf {
    target_dir.join(format!("wasm32-unknown-unknown/release/{}.wasm", package.replace('-', "_")))
}
