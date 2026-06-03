//! macOS capture backend — SKELETON (returns `Unsupported`).
//!
//! Real impl: ScreenCaptureKit. `SCShareableContent` to enumerate, then
//! an `SCStream` with an `SCContentFilter`: `init(display:excludingWindows:)`
//! for `ThisApp` (exclude any registered private-layer `NSWindow`s),
//! `init(desktopIndependentWindow:)` for `Source::Window`. Frames arrive
//! on an `SCStreamOutput` as `CMSampleBuffer` → `CVPixelBuffer` (BGRA).
//! Requires the Screen Recording TCC permission. Drive via `objc2`.

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
