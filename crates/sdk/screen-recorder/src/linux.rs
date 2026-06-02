//! Linux capture backend — SKELETON (returns `Unsupported`).
//!
//! Real impl: `org.freedesktop.portal.ScreenCast` via `ashpd` (the user
//! picks a monitor/window in the portal dialog), then a PipeWire stream;
//! map dmabuf/shm buffers → `VideoFrame`. No programmatic window
//! targeting and **no layer exclusion** is available — the private layer
//! renders inline here and `start` should `log` that exclusion is
//! unavailable on Linux rather than silently dropping it.

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
