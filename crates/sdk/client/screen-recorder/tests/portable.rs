//! Platform-agnostic tests for the screen-recorder skeleton. These run
//! on the host (no real capture backend) and exercise the API surface +
//! the skeleton's `Unsupported` contract.

use screen_recorder::{
    PrivateLayer, PrivateLayerProps, RecorderError, RecordingConfig, ScreenRecorder, Source,
    DEFAULT_FPS,
};
use runtime_core::{view, Element, IntoElement};

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
    // It builds an Element::External; constructing it must not panic and
    // must accept a children vec.
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

#[tokio::test]
async fn start_reports_unsupported_on_skeleton() {
    let recorder = ScreenRecorder::new();
    // `MediaStream` (the Ok variant) isn't `Debug`, so match rather than
    // `expect_err`.
    let result = recorder.start(RecordingConfig::new()).await;
    assert!(matches!(result, Err(RecorderError::Unsupported)));
}

#[tokio::test]
async fn request_permission_reports_unsupported_on_skeleton() {
    let recorder = ScreenRecorder::new();
    let result = recorder.request_permission(&Source::ThisApp).await;
    assert!(matches!(result, Err(RecorderError::Unsupported)));
}
