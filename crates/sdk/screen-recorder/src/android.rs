//! Android capture backend — SKELETON (returns `Unsupported`).
//!
//! Real impl: `MediaProjectionManager.createScreenCaptureIntent()` for
//! consent (re-prompts each session on 14+), then a `VirtualDisplay`
//! rendering into an `ImageReader` `Surface`; pull `Image` planes →
//! `VideoFrame` (RGBA/NV12). Requires a foreground service with
//! `mediaProjection` type. For the private layer, mirror recordable
//! content into a `Presentation` on a *second* captured `VirtualDisplay`
//! and keep the overlay on the default display. Glue lives in a
//! `runtime_kotlin` `ScreenRecorder.kt`; bridge via JNI.

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
