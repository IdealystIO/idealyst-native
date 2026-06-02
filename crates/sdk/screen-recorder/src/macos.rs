//! macOS capture backend — SKELETON (returns `Unsupported`).
//!
//! Real impl: ScreenCaptureKit. `SCShareableContent` to enumerate, then
//! an `SCStream` with an `SCContentFilter`: `init(display:excludingWindows:)`
//! for `ThisApp` (exclude any registered private-layer `NSWindow`s),
//! `init(desktopIndependentWindow:)` for `Source::Window`. Frames arrive
//! on an `SCStreamOutput` as `CMSampleBuffer` → `CVPixelBuffer` (BGRA).
//! Requires the Screen Recording TCC permission. Drive via `objc2`.

use crate::{RecorderError, RecordingConfig, Source};

pub(crate) async fn request_permission(_source: &Source) -> Result<(), RecorderError> {
    Err(RecorderError::Unsupported)
}

pub(crate) async fn start(
    _config: RecordingConfig,
    _on_frame: crate::BoxedFrameCallback,
) -> Result<Recording, RecorderError> {
    Err(RecorderError::Unsupported)
}

#[allow(dead_code)]
pub(crate) struct Recording;

#[allow(dead_code)]
impl Recording {
    pub(crate) fn pause(&self) -> Result<(), RecorderError> {
        Err(RecorderError::Unsupported)
    }
    pub(crate) fn resume(&self) -> Result<(), RecorderError> {
        Err(RecorderError::Unsupported)
    }
    pub(crate) async fn stop(self) -> Result<(), RecorderError> {
        Err(RecorderError::Unsupported)
    }
}
