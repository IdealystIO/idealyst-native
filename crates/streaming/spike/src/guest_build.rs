// How to build `spike-guest` to wasm32. Shared by `build.rs` (via
// `include!`, so it may use only std) and the `stream-serve` binary, so the
// bundle the server streams is built exactly like the one compiled into
// the app.

/// Sources whose edits change the bundle, relative to the stream-spike
/// crate directory.
pub const GUEST_SOURCES: &[&str] =
    &["guest/src", "guest/Cargo.toml", "camera/src", "../guest/src", "../abi/src", "../macros/src"];

/// Sources whose edits change the bridged RemoteCounter bundle
/// (`spike-remoteguest`): the component itself, and its mount export.
pub const REMOTE_GUEST_SOURCES: &[&str] =
    &["components/src", "components/Cargo.toml", "remoteguest/src", "remoteguest/Cargo.toml"];

/// Sources of the single-file example's bundle (`remote-example-bundle`).
pub const EXAMPLE_SOURCES: &[&str] = &["../example/app/src", "../example/bundle/Cargo.toml"];

/// Sources of the bridged test bundles (`spike-kernelguest`,
/// `spike-remoteguest`, `spike-remoteattr`) and the framework crates they
/// compile, for the build script's rerun triggers (cargo's own
/// fingerprinting decides what the nested build actually rebuilds).
pub const BRIDGED_SOURCES: &[&str] = &[
    "components/src",
    "kernelguest/src",
    "kernelguest/Cargo.toml",
    "remoteguest/src",
    "remoteguest/Cargo.toml",
    "remoteattr/src",
    "remoteattr/Cargo.toml",
    "../../runtime/macros/src",
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
/// Model A's bundles (the first design, `stream-guest`'s runtime). They are
/// built with `--cfg idealyst_stream_model_a` on top of the bundle flag, so
/// `#[host_fn]` emits model A's stub for them and the bridged stub for
/// every other bundle — and into their own target dir, because a different
/// `--cfg` changes every crate's fingerprint: sharing a dir would rebuild
/// the framework each time the build switched between the two.
const MODEL_A_PACKAGES: &[&str] = &["spike-guest"];

fn is_model_a(package_or_artifact: &str) -> bool {
    MODEL_A_PACKAGES.iter().any(|p| p.replace('-', "_") == package_or_artifact.replace('-', "_"))
}

fn guest_target_dir(target_dir: &std::path::Path, package_or_artifact: &str) -> std::path::PathBuf {
    if is_model_a(package_or_artifact) {
        target_dir.join("model-a")
    } else {
        target_dir.to_path_buf()
    }
}

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
        .arg(guest_target_dir(target_dir, package))
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
        //
        // `-Aunused`: in a bundle build every app component's body is
        // compiled out (it is imported from the app), so the imports and
        // helpers only those bodies used read as unused. The app build lints
        // the same sources with the bodies in.
        .env(
            "CARGO_ENCODED_RUSTFLAGS",
            if is_model_a(package) {
                "-Clink-arg=-zstack-size=65536\x1f--cfg=idealyst_stream_guest\x1f--cfg=idealyst_stream_model_a\x1f-Aunused"
            } else {
                "-Clink-arg=-zstack-size=65536\x1f--cfg=idealyst_stream_guest\x1f-Aunused"
            },
        )
        .env_remove("CARGO_BUILD_TARGET")
        .env_remove("CARGO_TARGET_DIR");
    cmd
}

/// Where [`guest_build_command`] leaves package `package`'s bundle.
///
/// `artifact` is the LIBRARY name (`[lib] name`), which is the package name
/// unless the package renames its lib — as `remote-example-bundle` does, so
/// it compiles under the app's crate name (see its Cargo.toml).
pub fn guest_wasm_path(target_dir: &std::path::Path, artifact: &str) -> std::path::PathBuf {
    guest_target_dir(target_dir, artifact).join(format!("wasm32-unknown-unknown/release/{}.wasm", artifact.replace('-', "_")))
}
