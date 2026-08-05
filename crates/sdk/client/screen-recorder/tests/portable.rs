//! Platform-agnostic tests for the screen-recorder. These run on the host and
//! exercise the API surface, the private-layer lowering, and — on targets that
//! still have no capture backend — the `Unsupported` fallback contract.
//!
//! The `*_on_unsupported_target` tests drive `start` / `request_permission`
//! end-to-end, so they are gated to the genuinely-unsupported fallback target:
//! every implemented backend (macOS TCC, the Linux xdg-desktop-portal share
//! dialog, ReplayKit/MediaProjection consent, web `getDisplayMedia`) drives a
//! real, interactive OS consent flow that cannot run unattended in `cargo test`
//! — the Linux portal `start`, in particular, blocks on a user-approved dialog.
//! Those live paths are covered by each backend's own `#[ignore]`d test.

use screen_recorder::{
    PrivateLayer, RecorderError, RecordingConfig, ScreenRecorder, Source, DEFAULT_FPS,
};

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

/// Regression coverage for the private-layer capture-exclusion wiring
/// (CLAUDE.md §8 — named after the behavior, not the function).
///
/// The capture-exclusion mechanism itself is native: a separate
/// `UIWindow` on iOS / `WindowManager` window on Android that the
/// recorder omits. Those need a live UIKit main thread / a JVM + an
/// Android `WindowManager`, so they're verified on-device by the
/// orchestrator, not in `cargo test`.
///
/// What IS host-checkable — and what the whole design hinges on — is
/// that `PrivateLayer(children)` lowers to an `Element::External` keyed
/// by `PrivateLayerProps`'s `TypeId` and CARRIES its children. The
/// backend handler returns the detached window root, and the framework
/// walker parents these children into it. If this contract broke (wrong
/// TypeId → handler never dispatched, or children dropped → empty
/// overlay), the on-device run would show a blank/recorded layer. So we
/// assert the lowering deterministically here.
#[test]
fn private_layer_lowers_to_external_carrying_children() {
    let child = view(Vec::new()).into_element();
    let layer: Element = PrivateLayer(vec![child]).into_element();

    match layer {
        Element::External {
            type_id,
            children,
            ..
        } => {
            assert_eq!(
                type_id,
                std::any::TypeId::of::<PrivateLayerProps>(),
                "PrivateLayer must dispatch to the PrivateLayerProps handler"
            );
            assert_eq!(
                children.len(),
                1,
                "the layer's children must ride the External so the backend \
                 can parent them into the capture-excluded window root"
            );
        }
        _ => panic!("PrivateLayer must lower to Element::External"),
    }
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
