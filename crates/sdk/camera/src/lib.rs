//! Cross-platform camera capture.
//!
//! The smallest useful abstraction over the platform's camera: open a
//! stream, get raw pixel frames in a callback, drop the stream to stop.
//! No files, no encoding, no preview widget, no opinion about where the
//! frames go — that's for higher-level SDKs (or the app) to layer on top.
//! This crate just establishes the stream and hands you pixels.
//!
//! It is the sibling of the `microphone` SDK: same shape, same
//! unopinionated posture, but for video frames instead of PCM. If you want
//! the frames *on screen*, copy them into a `graphics` surface (a GPU
//! texture you own) or draw them to a canvas — this SDK deliberately does
//! not render anything itself.
//!
//! ```no_run
//! use camera::{Camera, CameraConfig};
//!
//! # async fn demo() -> Result<(), camera::CameraError> {
//! let cam = Camera::new();
//! // Keep the returned stream alive for as long as you want to capture.
//! let stream = cam
//!     .open(CameraConfig::default().back(), |frame| {
//!         // Runs on a capture thread (native/Android) or the main thread
//!         // (web). Copy out what you need and return fast.
//!         let (w, h) = (frame.width, frame.height);
//!         let _ = (w, h, frame.data);
//!     })
//!     .await?;
//!
//! // ... later ...
//! stream.stop();
//! # Ok(())
//! # }
//! ```
//!
//! # Architecture
//!
//! The platform-agnostic surface ([`Camera`], [`CameraStream`],
//! [`VideoFrame`], [`CameraConfig`], [`CameraFacing`], [`CameraError`])
//! lives here. Exactly one cfg-gated backend module is compiled per target
//! and supplies the `imp` submodule the public API delegates to:
//!
//! - **web (wasm32)** — `getUserMedia` + a `<video>`/`<canvas>` frame pump.
//! - **iOS / macOS** — `AVCaptureSession` + `AVCaptureVideoDataOutput`,
//!   frames delivered to a sample-buffer delegate.
//! - **Android** — `Camera2` + `ImageReader` read on a JNI worker thread.
//! - **other (desktop Linux/Windows)** — not yet implemented; every call
//!   returns [`CameraError::Unsupported`]. A V4L2/MSMF backend is a clean
//!   future addition, not a gap in the capture model.
//!
//! # Permissions
//!
//! The app must declare the platform's camera permission, or capture is
//! denied at runtime. This SDK declares the `camera` capability
//! (`[package.metadata.idealyst] capabilities = ["camera"]`); the build
//! CLI injects the right per-platform artifacts:
//!
//! - **iOS / macOS** — `NSCameraUsageDescription` in `Info.plist`.
//! - **Android** — `<uses-permission android:name="android.permission.CAMERA"/>`.
//! - **web** — none; the browser prompts on first `getUserMedia`.
//!
//! [`Camera::request_permission`] proactively triggers that prompt (and is
//! a no-op where the OS prompts implicitly), but it's optional —
//! [`Camera::open`] requests access itself if needed.

#![deny(missing_docs)]

mod config;
mod error;
mod frame;

pub use config::{CameraConfig, CameraFacing};
pub use error::CameraError;
pub use frame::{PixelFormat, VideoFrame};

// ---------------------------------------------------------------------------
// The callback bound.
//
// The native (AVFoundation) and Android backends deliver frames on a
// capture/reader thread, so the callback must be `Send` there. The web
// backend runs it on the single wasm thread inside a `requestVideoFrame`
// handler holding non-`Send` JS values, so `Send` is both unnecessary and
// unsatisfiable. One marker trait, cfg'd, keeps the public `open`
// signature identical on every target while enforcing the right bound
// underneath. (Mirrors `microphone::AudioCallback`.)
// ---------------------------------------------------------------------------

/// The bound a frame callback must satisfy. Implemented automatically for
/// any matching closure — you never write `impl FrameCallback` yourself,
/// just pass a `|frame| { .. }` closure to [`Camera::open`].
///
/// On native and Android targets this requires `Send` (the callback runs
/// on the capture/reader thread); on web it does not (it runs on the main
/// thread). The closure is `FnMut`, so it may own and mutate state across
/// frames.
#[cfg(not(target_arch = "wasm32"))]
pub trait FrameCallback: FnMut(&VideoFrame) + Send + 'static {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: FnMut(&VideoFrame) + Send + 'static> FrameCallback for T {}

/// See the non-wasm definition; on web the `Send` bound is dropped.
#[cfg(target_arch = "wasm32")]
pub trait FrameCallback: FnMut(&VideoFrame) + 'static {}
#[cfg(target_arch = "wasm32")]
impl<T: FnMut(&VideoFrame) + 'static> FrameCallback for T {}

/// The boxed form backends actually receive. Mirrors [`FrameCallback`]'s
/// cfg'd `Send`-ness.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) type BoxedCallback = Box<dyn FnMut(&VideoFrame) + Send + 'static>;
#[cfg(target_arch = "wasm32")]
pub(crate) type BoxedCallback = Box<dyn FnMut(&VideoFrame) + 'static>;

// ---------------------------------------------------------------------------
// Backend selector. Exactly one compiles per target; each supplies an
// `imp` module with `request_permission()`, `open()`, and a `StreamHandle`
// whose `Drop` stops capture.
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[path = "web.rs"]
mod imp;

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
#[path = "android.rs"]
mod imp;

#[cfg(all(any(target_os = "ios", target_os = "macos"), not(target_arch = "wasm32")))]
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
    /// platforms that have one (iOS, macOS, Android, web). Resolves
    /// `Ok(())` once access is granted, [`CameraError::PermissionDenied`]
    /// if refused. A no-op `Ok(())` where the platform grants implicitly or
    /// prompts only on capture start.
    ///
    /// Optional — [`open`](Camera::open) requests access on its own. Call
    /// this when you want the prompt to appear before, say, showing a
    /// capture UI.
    pub async fn request_permission(&self) -> Result<(), CameraError> {
        imp::request_permission().await
    }

    /// Open a live capture stream. `callback` fires with each captured
    /// frame (see [`VideoFrame`]) until the returned [`CameraStream`] is
    /// dropped (or [`stop`](CameraStream::stop)ped).
    ///
    /// Requests permission if it hasn't been granted yet, so this can
    /// surface the OS prompt. The callback runs on a capture thread on
    /// native/Android targets and on the main thread on web — keep it fast
    /// and non-blocking; copy pixels out rather than processing heavily in
    /// place.
    pub async fn open<C: FrameCallback>(
        &self,
        config: CameraConfig,
        callback: C,
    ) -> Result<CameraStream, CameraError> {
        let boxed: BoxedCallback = Box::new(callback);
        let handle = imp::open(config, boxed).await?;
        Ok(CameraStream { _handle: handle })
    }
}

/// A live capture stream. Capture runs for as long as this value is alive;
/// dropping it tears the stream down and stops the callback. Hold onto it
/// (e.g. in your app state) for the duration you want to capture.
///
/// Not `Send` on native targets — the underlying platform session is tied
/// to the thread that opened it. Keep it on that thread.
pub struct CameraStream {
    // The concrete type is backend-specific; its `Drop` stops capture.
    _handle: imp::StreamHandle,
}

impl CameraStream {
    /// Stop capturing and release the stream. Equivalent to dropping the
    /// value; provided for call sites where an explicit `stop()` reads
    /// clearer than a `drop(stream)`.
    pub fn stop(self) {
        // `self` drops here, running the backend's teardown.
    }
}
