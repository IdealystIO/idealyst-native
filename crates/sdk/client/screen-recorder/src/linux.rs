//! Linux capture backend — SKELETON (returns `Unsupported`).
//!
//! Real impl: `org.freedesktop.portal.ScreenCast` via `ashpd` (the user
//! picks a monitor/window in the portal dialog), then a PipeWire stream;
//! map dmabuf/shm buffers → `VideoFrame`. No programmatic window
//! targeting and **no layer exclusion** is available — the private layer
//! renders inline here and `start` should `log` that exclusion is
//! unavailable on Linux rather than silently dropping it.

use crate::{NativeSource, RecorderError, RecordingConfig, Source};
use media_stream::FrameWriter;

pub(crate) async fn request_permission(_source: &Source) -> Result<(), RecorderError> {
    Err(RecorderError::Unsupported)
}

pub(crate) async fn start(
    _config: RecordingConfig,
    _writer: FrameWriter,
) -> Result<(Recording, Option<NativeSource>), RecorderError> {
    Err(RecorderError::Unsupported)
}

#[allow(dead_code)]
pub(crate) struct Recording;

/// Install the `PrivateLayer` external handler — a no-op on this target
/// (no capture-exclusion mechanism, and these backends don't expose an
/// `ExternalRegistry` the SDK can reach generically). Generic over
/// [`Backend`](runtime_core::Backend) so author code can call
/// `screen_recorder::register(&mut backend)` unconditionally; the
/// framework's External placeholder renders if a `PrivateLayer` is
/// actually mounted, making the unbound layer obvious. macOS's
/// `SCContentFilter(excludingWindows:)` exclusion is a later addition.
pub fn register<B: runtime_core::Backend>(_backend: &mut B) {}
