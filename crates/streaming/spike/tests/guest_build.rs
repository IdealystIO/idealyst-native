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
