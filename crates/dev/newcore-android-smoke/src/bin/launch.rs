//! Host-side launcher: builds this project through the EXISTING
//! Android tooling (`build-android` wrapper generation → NDK cargo
//! build → `run-android`'s javac/aapt2/d8/apksigner APK assembly →
//! `adb install` + launch). The generated wrapper's `NativeBridge.attach`
//! mounts [`newcore_android_smoke::scene_app`] through
//! `backend_android::newcore::start`, passing the crate's
//! `register_scene_extensions` seam alongside it.
//!
//! Prereqs: `ANDROID_HOME` (SDK w/ platform-tools + build-tools) and
//! `ANDROID_NDK_HOME`; a booted emulator or connected device (the run
//! tool can also boot an AVD itself). Then:
//!
//! ```text
//! cargo run -p newcore-android-smoke --features launcher --bin launch
//! adb logcat -s idealyst | grep SMOKE-SELFTEST   # expect verdict=PASS
//! ```

use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let project = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // crates/dev/newcore-android-smoke → workspace root.
    let root = project
        .ancestors()
        .nth(3)
        .expect("workspace root above crates/dev/<crate>")
        .to_path_buf();

    let artifact = run_android::run(
        &project,
        run_android::RunOptions {
            release: false,
            avd: None,
            mode: run_android::RunMode::Local,
            runtime_server_port: None,
            source: build_ios::FrameworkSource::Workspace { root },
            // No extra features: the generated wrapper's `attach` mounts
            // through `backend_android::newcore::start` unconditionally.
            user_features: vec![],
            robot_relay_url: None,
        },
    )?;
    eprintln!(
        "[newcore-android-smoke] installed {} on {}",
        artifact.apk.display(),
        artifact.serial,
    );
    eprintln!("self-test: adb logcat -s idealyst | grep SMOKE-SELFTEST");
    Ok(())
}
