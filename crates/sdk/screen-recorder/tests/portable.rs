//! Platform-agnostic tests for the screen-recorder skeleton. These run
//! on the host (no real capture backend) and exercise the API surface +
//! the skeleton's `Unsupported` contract.

use screen_recorder::{
    PrivateLayer, RecorderError, RecordingConfig, ScreenRecorder, Source, DEFAULT_FPS,
};

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

#[tokio::test]
async fn start_reports_unsupported_on_skeleton() {
    let recorder = ScreenRecorder::new();
    // `RecordingHandle` (the Ok variant) isn't `Debug`, so match rather
    // than `expect_err`.
    let result = recorder
        .start(RecordingConfig::new(), |_frame| {})
        .await;
    assert!(matches!(result, Err(RecorderError::Unsupported)));
}

#[tokio::test]
async fn request_permission_reports_unsupported_on_skeleton() {
    let recorder = ScreenRecorder::new();
    let result = recorder.request_permission(&Source::ThisApp).await;
    assert!(matches!(result, Err(RecorderError::Unsupported)));
}
