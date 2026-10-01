//! Platform-agnostic tests for the screen-recorder. These run on the host and
//! exercise the API surface and — on targets that
//! still have no capture backend — the `Unsupported` fallback contract.
//!
//! The `*_on_unsupported_target` tests drive `start` / `request_permission`
//! end-to-end, so they are gated to the genuinely-unsupported fallback target:
//! every implemented backend (macOS TCC, the Linux xdg-desktop-portal share
//! dialog, ReplayKit/MediaProjection consent, web `getDisplayMedia`) drives a
//! real, interactive OS consent flow that cannot run unattended in `cargo test`
//! — the Linux portal `start`, in particular, blocks on a user-approved dialog.
//! Those live paths are covered by each backend's own `#[ignore]`d test.

use screen_recorder::{PrivateLayer, RecordingConfig, Source, DEFAULT_FPS};

// Only referenced by the unsupported-target fallback tests below.
#[cfg(all(
    not(target_arch = "wasm32"),
    not(target_os = "ios"),
    not(target_os = "macos"),
    not(target_os = "android"),
    not(target_os = "windows"),
    not(target_os = "linux")
))]
use screen_recorder::{RecorderError, ScreenRecorder};

#[test]
fn config_defaults_are_sane() {
    let cfg = RecordingConfig::new();
    assert!(matches!(cfg.source, Source::ThisApp));
    assert_eq!(cfg.fps, DEFAULT_FPS);
    assert!(cfg.size.is_none());
}

#[test]
fn config_builders_apply() {
    let cfg = RecordingConfig::new()
        .source(Source::FullScreen)
        .fps(60)
        .size(1280, 720);
    assert!(matches!(cfg.source, Source::FullScreen));
    assert_eq!(cfg.fps, 60);
    assert_eq!(cfg.size, Some((1280, 720)));
}

#[test]
fn private_layer_constructs_without_panicking() {
    // It builds a scene item; constructing it must not panic and must
    // accept a children vec. The mount-shape contract (children realize
    // INTO the external node) is pinned in tests/private_layer.rs.
    let _layer = PrivateLayer(Vec::new());
}

// The `Unsupported` fallback contract — only on targets with no capture
// backend. Every implemented backend drives a real, interactive OS consent
// flow here (see the module docs), so these end-to-end calls can't run
// unattended on those platforms.
#[cfg(all(
    not(target_arch = "wasm32"),
    not(target_os = "ios"),
    not(target_os = "macos"),
    not(target_os = "android"),
    not(target_os = "windows"),
    not(target_os = "linux")
))]
#[tokio::test]
async fn start_reports_unsupported_on_unsupported_target() {
    let recorder = ScreenRecorder::new();
    // `MediaStream` (the Ok variant) isn't `Debug`, so match rather than
    // `expect_err`.
    let result = recorder.start(RecordingConfig::new()).await;
    assert!(matches!(result, Err(RecorderError::Unsupported)));
}

#[cfg(all(
    not(target_arch = "wasm32"),
    not(target_os = "ios"),
    not(target_os = "macos"),
    not(target_os = "android"),
    not(target_os = "windows"),
    not(target_os = "linux")
))]
#[tokio::test]
async fn request_permission_reports_unsupported_on_unsupported_target() {
    let recorder = ScreenRecorder::new();
    let result = recorder.request_permission(&Source::ThisApp).await;
    assert!(matches!(result, Err(RecorderError::Unsupported)));
}
