//! Fallback backend for targets without a camera implementation yet
//! (desktop Linux/Windows). Every entry point reports
//! [`CameraError::Unsupported`] rather than silently doing nothing, so a
//! caller on an unsupported host gets a clear, handleable error instead of
//! a stream that never fires.
//!
//! Adding real desktop capture here (V4L2 on Linux, Media Foundation on
//! Windows) is a self-contained future addition — it slots in behind the
//! same `request_permission` / `open` / `StreamHandle` contract the other
//! backends satisfy.

use crate::{CameraConfig, CameraError, NativeSource};
use media_stream::FrameWriter;

/// No resources are held; the type exists only to satisfy the `imp`
/// contract. It is never constructed (every `open` errors first).
pub(crate) struct StreamHandle {
    _never: std::convert::Infallible,
}

pub(crate) async fn request_permission() -> Result<(), CameraError> {
    Err(CameraError::Unsupported)
}

pub(crate) async fn open(
    _config: CameraConfig,
    _writer: FrameWriter,
) -> Result<(StreamHandle, Option<NativeSource>), CameraError> {
    Err(CameraError::Unsupported)
}
