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
