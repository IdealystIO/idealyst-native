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

// ===========================================================================
// Private layer — borderless overlay window above the app window.
// ===========================================================================

use backend_macos::MacosBackend;

/// Install the `PrivateLayer` external handler against a `MacosBackend`.
///
/// The handler asks the backend to build a separate, borderless `NSWindow`
/// (see `MacosBackend::create_private_layer_window`) and returns its content
/// view. The framework's External walker then parents the layer's children
/// (toolbar, recording preview) into that content view; the backend's
/// `insert` / `clear_children` skip reparenting it into the main tree because
/// the content view is registered as a detached window root.
///
/// The overlay is added as a CHILD window above the app window so it tracks
/// the app's moves + Spaces and composites on top, and its passthrough
/// `hitTest:` lets clicks fall through to the app everywhere except over a
/// real control — so the toolbar is interactive while the canvas beneath stays
/// drawable. Mirrors `ios::register`.
///
/// Capture EXCLUSION (omitting the overlay from a ScreenCaptureKit recording
/// via `SCContentFilter(excludingWindows:)`) is a separate, later task; the
/// backend already registers every overlay window so that task can enumerate
/// them via `MacosBackend::private_layer_windows`.
pub fn register(backend: &mut MacosBackend) {
    backend.register_external::<crate::PrivateLayerProps, _>(|_props, b| {
        b.create_private_layer_window()
    });
}
