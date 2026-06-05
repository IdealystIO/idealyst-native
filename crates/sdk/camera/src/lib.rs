//! Cross-platform camera capture.
//!
//! The smallest useful abstraction over the platform's camera: open it, get
//! a [`MediaStream`] — a platform-agnostic live video source — and drop the
//! stream to stop. No files, no encoding, no preview widget. Tap raw frames
//! with [`MediaStream::subscribe`], or hand the stream to a display /
//! compositing layer; the per-platform transport stays hidden.
//!
//! It is the sibling of the `microphone` SDK: same unopinionated posture,
//! but for video. The [`MediaStream`] is the common currency it shares with
//! `screen-recorder` (another producer) and the `video` display layer (a
//! consumer) — see the `media-stream` crate.
//!
//! ```no_run
//! use camera::{Camera, CameraConfig};
//!
//! # async fn demo() -> Result<(), camera::CameraError> {
//! let cam = Camera::new();
//! // Keep the stream alive for as long as you want to capture.
//! let stream = cam.open(CameraConfig::default().back()).await?;
//!
//! // Tap raw RGBA8 frames (runs on a capture thread / the web main thread):
//! let sub = stream.subscribe(|frame| {
//!     let (w, h) = (frame.width, frame.height);
//!     let _ = (w, h, frame.data);
//! });
//!
//! // ... later: drop `sub` to stop tapping, drop `stream` to stop capture.
//! # let _ = sub;
//! # Ok(())
//! # }
//! ```
//!
//! # Architecture
//!
//! The platform-agnostic surface ([`Camera`], [`CameraConfig`],
//! [`CameraFacing`], [`CameraError`]) lives here; the live-source surface
//! ([`MediaStream`], [`VideoFrame`], …) is re-exported from `media-stream`.
//! Exactly one cfg-gated backend module is compiled per target; each pushes
//! frames into a [`FrameWriter`](media_stream::FrameWriter):
//!
//! - **web (wasm32)** — `getUserMedia` + a `<video>`/`<canvas>` frame pump.
//!   Also publishes the `web_sys::MediaStream` as the stream's
//!   [`native_source`](MediaStream::native_source) for a future zero-copy
//!   display / GPU consumer.
//! - **iOS / macOS** — `AVCaptureSession` + `AVCaptureVideoDataOutput`.
//! - **Android** — `Camera2` + `ImageReader` via a Kotlin shim.
//! - **other (desktop Linux/Windows)** — not yet implemented; returns
//!   [`CameraError::Unsupported`].
//!
//! # Permissions
//!
//! The app must declare the platform's camera permission. This SDK declares
//! the `camera` capability (`[package.metadata.idealyst] capabilities =
//! ["camera"]`); the build CLI injects `NSCameraUsageDescription` (iOS/macOS)
//! / the `CAMERA` permission (Android). Web prompts on first `getUserMedia`.
//!
//! [`Camera::request_permission`] proactively triggers that prompt; it's
//! optional — [`Camera::open`] requests access itself if needed.

#![deny(missing_docs)]

mod config;
mod error;

pub use config::{CameraConfig, CameraFacing};
pub use error::CameraError;

// The live-source surface is the shared `media-stream` vocabulary. Re-export
// it so a camera user has everything from `camera::`.
pub use media_stream::{FrameCallback, MediaStream, PixelFormat, Subscription, VideoFrame};

/// The type-erased zero-copy frame source a backend publishes on the stream
/// (e.g. the web `web_sys::MediaStream`), downcast by a same-platform
/// consumer. `None` where no zero-copy source is exposed (yet).
pub(crate) type NativeSource = std::rc::Rc<dyn std::any::Any>;

// ---------------------------------------------------------------------------
// Backend selector. Exactly one compiles per target; each supplies an `imp`
// module with `request_permission()`, `open(config, FrameWriter)`, and a
// `StreamHandle` whose `Drop` stops capture.
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[path = "web.rs"]
mod imp;

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
#[path = "android.rs"]
mod imp;

// iOS SIMULATOR: synthetic camera (no AVCaptureSession hardware exists there).
// Routed ahead of `apple.rs` and excluded from it below, so the simulator gets a
// fake stream while iOS DEVICES + macOS keep the real AVFoundation backend.
#[cfg(all(target_os = "ios", target_abi = "sim", not(target_arch = "wasm32")))]
#[path = "sim_camera.rs"]
mod imp;

#[cfg(all(
    any(target_os = "ios", target_os = "macos"),
    not(target_arch = "wasm32"),
    not(all(target_os = "ios", target_abi = "sim"))
))]
#[path = "apple.rs"]
mod imp;

#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    target_os = "ios",
    target_os = "macos"
)))]
#[path = "stub.rs"]
mod imp;

// ---------------------------------------------------------------------------
// Public API.
// ---------------------------------------------------------------------------

/// A handle to the device's camera. Cheap to construct and clone; it holds
/// no OS resources until you [`open`](Camera::open) a stream.
#[derive(Clone, Default)]
pub struct Camera {
    _private: (),
}

impl Camera {
    /// Create a camera handle.
    pub fn new() -> Self {
        Self::default()
    }

    /// Proactively request camera permission, triggering the OS prompt on
    /// platforms that have one (iOS, macOS, Android, web). Resolves `Ok(())`
    /// once access is granted, [`CameraError::PermissionDenied`] if refused.
    ///
    /// Optional — [`open`](Camera::open) requests access on its own.
    pub async fn request_permission(&self) -> Result<(), CameraError> {
        imp::request_permission().await
    }

    /// Open the camera and return a live [`MediaStream`]. Capture runs while
    /// any clone of the stream is alive; dropping the last one stops it.
    ///
    /// Requests permission if needed (can surface the OS prompt). Tap frames
    /// with [`MediaStream::subscribe`] / [`MediaStream::latest`], or hand the
    /// stream to a display / compositing consumer.
    pub async fn open(&self, config: CameraConfig) -> Result<MediaStream, CameraError> {
        let (stream, writer) = MediaStream::new();
        let (handle, native) = imp::open(config, writer).await?;
        if let Some(src) = native {
            stream.set_native_source(src);
        }
        // The backend `StreamHandle` (which owns the capture session + the
        // `FrameWriter`) stops capture on drop. Own it in the stopper so it
        // tears down when the last `MediaStream` clone drops.
        stream.attach_stopper(move || drop(handle));
        Ok(stream)
    }
}
