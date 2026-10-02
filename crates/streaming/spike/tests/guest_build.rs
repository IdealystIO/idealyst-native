//! How bundles are built (`src/guest_build.rs`, shared by the build scripts
//! and `stream-serve`).

use std::ffi::OsStr;
use std::path::Path;

/// A profile override given to the outer (app) build must not reach the
/// bundle's nested cargo: `measure.sh` builds the app with cargo's default
/// release profile through `CARGO_PROFILE_RELEASE_*`, and the build script's
/// cargo inherited it — the showcase bundle came out at opt 3 without LTO,
/// twice its size.
#[test]
fn regression_bundle_build_ignores_the_apps_profile_overrides() {
    // This test file is its own process; nothing else reads the env.
    std::env::set_var("CARGO_PROFILE_RELEASE_OPT_LEVEL", "3");
    std::env::set_var("CARGO_PROFILE_RELEASE_LTO", "false");
    let cmd = stream_spike::guest_build::guest_build_command("cargo", Path::new("."), Path::new("t"), "spike-remoteattr");
    for key in ["CARGO_PROFILE_RELEASE_OPT_LEVEL", "CARGO_PROFILE_RELEASE_LTO"] {
        let set = cmd.get_envs().find(|(k, _)| *k == OsStr::new(key)).map(|(_, v)| v);
        assert_eq!(set, Some(None), "{key} must be removed from the bundle build's environment");
    }
}

#[test]
fn dep_info_lists_every_prerequisite_including_escaped_spaces() {
    let d = "/t/x.wasm: /a/lib.rs /b/my\\ crate/src/lib.rs /c/mod.rs\n\n/a/lib.rs:\n";
    let got = stream_spike::guest_build::parse_dep_info(d);
    let want: Vec<std::path::PathBuf> = ["/a/lib.rs", "/b/my crate/src/lib.rs", "/c/mod.rs"].iter().map(Into::into).collect();
    assert_eq!(got, want);
}

/// What a bundle build reruns on includes the libraries it compiles, not
/// just its own source: the showcase's embedded bundle stayed stale after
/// edits to idea-ui and the framework, because its build script watched
/// only `src/`.
#[test]
fn regression_bundle_sources_include_the_libraries_the_bundle_compiles() {
    let target_dir = Path::new(env!("OUT_DIR")).join("guest-target");
    let sources = stream_spike::guest_build::bundle_sources(&target_dir, "spike_remoteattr");
    let has = |part: &str| sources.iter().any(|p| p.to_string_lossy().contains(part));
    assert!(has("spike/remoteattr/src"), "the bundle's own source: {sources:?}");
    assert!(has("runtime/vocabulary/src/remote"), "a library it compiles: {sources:?}");
}
