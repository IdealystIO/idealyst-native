//! iOS capture backend — SKELETON (returns `Unsupported`).
//!
//! Real impl: `RPScreenRecorder.shared().startCapture(handler:)` (in-app)
//! or a Broadcast Upload Extension + `RPSystemBroadcastPickerView`
//! (system-wide). Each `CMSampleBuffer` → `CVPixelBuffer` (lock base
//! address) → `VideoFrame { format: Bgra8, .. }`. `Source::Window` is
//! unsupported on iOS (return `UnsupportedSource`); `UserChoice` and
//! `FullScreen` collapse to the app capture. Drive via `objc2`/`block2`.

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
