// How to build a bundle crate to wasm32. Shared by `build.rs` (via
// `include!`, so it may use only std) and the `stream-serve` binary, so the
// bundle the server streams is built exactly like the one compiled into
// the app.

/// Sources whose edits change the bridged RemoteCounter bundle
/// (`spike-remoteguest`): the component itself, and its mount export.
/// Paths here and below are relative to the stream-spike crate directory.
pub const REMOTE_GUEST_SOURCES: &[&str] =
    &["components/src", "components/Cargo.toml", "remoteguest/src", "remoteguest/Cargo.toml"];

/// Sources of the showcase's bundle (`remote-showcase`'s own library).
pub const SHOWCASE_SOURCES: &[&str] = &["../showcase/app/src", "../showcase/app/Cargo.toml"];

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
    "camera/src",
    "camera/Cargo.toml",
    "../../runtime/macros/src",
    "../../runtime/vocabulary/src",
    "../../runtime/scene/src",
    "../../runtime/shared/src",
    "../abi/src",
    "../../runtime/world/src",
];

/// The `wasm32-unknown-unknown` standard library's directory for `rustc`
/// (a build script passes cargo's `RUSTC`), and whether it is installed.
/// Without it a bundle can't be built.
pub fn wasm32_libdir(rustc: &str) -> (Option<std::path::PathBuf>, bool) {
    let dir = std::process::Command::new(rustc)
        .args(["--print", "target-libdir", "--target", "wasm32-unknown-unknown"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| std::path::PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()));
    let installed = dir.as_ref().is_some_and(|d| std::fs::read_dir(d).is_ok_and(|mut e| e.next().is_some()));
    (dir, installed)
}

/// A build script's way out when wasm32 is not installed: write each of
/// `files` empty into `out` (so `include_bytes!` still compiles), and say
/// why. Everything else in the crate builds; loading one of these bundles
/// fails with a load error. `libdir` (when known) is watched, so installing
/// the target reruns the script.
pub fn skip_bundles(out: &std::path::Path, files: &[&str], libdir: Option<&std::path::Path>) {
    for file in files {
        std::fs::write(out.join(file), b"").unwrap_or_else(|e| panic!("write placeholder {file}: {e}"));
    }
    println!(
        "cargo:warning=the wasm32-unknown-unknown target is not installed, so the remote-component \
         bundles were not built (empty placeholders embedded; loading one fails). Install it with \
         `rustup target add wasm32-unknown-unknown`."
    );
    if let Some(dir) = libdir {
        println!("cargo:rerun-if-changed={}", dir.display());
    }
}

/// Compile `package`'s LIBRARY for wasm32 into `target_dir`, as a cdylib
/// bundle — the compile `idealyst build --remote` runs
/// (`build_remote::bundle_command`), so a dev bundle is built exactly like
/// a release one. `cargo rustc --crate-type cdylib` asks for the cdylib, so
/// a crate that is also the app (the showcase) needn't declare one, and its
/// bundle compiles under its own crate name.
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
        .args(["rustc", "-p", package, "--lib", "--release", "--target", "wasm32-unknown-unknown", "--crate-type", "cdylib"])
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
        //
        // `-Aunused`: in a bundle build every app component's body is
        // compiled out (it is imported from the app), so the imports and
        // helpers only those bodies used read as unused. The app build lints
        // the same sources with the bodies in.
        .env("CARGO_ENCODED_RUSTFLAGS", "-Clink-arg=-zstack-size=65536\x1f--cfg=idealyst_stream_guest\x1f-Aunused")
        .env_remove("CARGO_BUILD_TARGET")
        .env_remove("CARGO_TARGET_DIR");
    // Profile overrides the outer build was given are for the APP: inherited,
    // they would rebuild the bundle with the app's profile (an opt-3,
    // no-LTO native profile doubled the showcase bundle, 282 → 575 KB). The
    // bundle builds with the workspace's release profile.
    for (key, _) in std::env::vars_os() {
        if key.to_str().is_some_and(|k| k.starts_with("CARGO_PROFILE_")) {
            cmd.env_remove(key);
        }
    }
    cmd
}

/// Where [`guest_build_command`] leaves package `package`'s bundle.
///
/// `artifact` is the LIBRARY name (`[lib] name`), which is the package name
/// unless the package renames its lib — as `remote-example-bundle` does, so
/// it compiles under the app's crate name (see its Cargo.toml).
pub fn guest_wasm_path(target_dir: &std::path::Path, artifact: &str) -> std::path::PathBuf {
    target_dir.join(format!("wasm32-unknown-unknown/release/{}.wasm", artifact.replace('-', "_")))
}

/// Every source file the last build of `artifact` read, from cargo's
/// dep-info (`<artifact>.d`, next to the wasm): the bundle's own source AND
/// every local crate it compiles (the framework, idea-ui, …). What a build
/// script reruns on and what `stream-serve` watches — so editing a library
/// the bundle uses rebuilds it, which a hand-kept list of directories
/// missed. Empty before the first build.
pub fn bundle_sources(target_dir: &std::path::Path, artifact: &str) -> Vec<std::path::PathBuf> {
    let wasm = guest_wasm_path(target_dir, artifact);
    let Ok(dep_info) = std::fs::read_to_string(wasm.with_extension("d")) else {
        return Vec::new();
    };
    parse_dep_info(&dep_info)
}

/// The prerequisites of the first rule in a make-style dep-info file
/// (`target: a.rs b\ c.rs …`; a space inside a path is escaped).
pub fn parse_dep_info(dep_info: &str) -> Vec<std::path::PathBuf> {
    let Some(line) = dep_info.lines().next() else {
        return Vec::new();
    };
    // The target is a path too, so split at the first unescaped ": ".
    let Some(deps) = line.split_once(": ").map(|(_, d)| d) else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    let mut current = String::new();
    let mut chars = deps.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&' ') => {
                current.push(' ');
                chars.next();
            }
            ' ' => {
                if !current.is_empty() {
                    paths.push(std::path::PathBuf::from(std::mem::take(&mut current)));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        paths.push(std::path::PathBuf::from(current));
    }
    paths
}
