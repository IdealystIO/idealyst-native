//! Windows capture backend — SKELETON (returns `Unsupported`).
//!
//! Real impl: Windows.Graphics.Capture. `GraphicsCaptureItem` from an
//! HWND (`ThisApp`/`Window`) or HMONITOR (`FullScreen`), a
//! `Direct3D11CaptureFramePool` (FreeThreaded), and a `FrameArrived`
//! handler that maps the `ID3D11Texture2D` to a CPU `VideoFrame` (BGRA).
//! The private layer needs no recorder coordination here — its sibling
//! HWND just sets `SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)`
//! and WGC renders it blank. Drive via the `windows` crate.

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
