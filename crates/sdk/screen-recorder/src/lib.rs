//! Cross-platform screen / window recording for the idealyst framework.
//!
//! Two independent pieces:
//!
//! 1. **The capture capability** — [`ScreenRecorder`]. A backend-agnostic
//!    object (no `Element`, no `Backend` method) that establishes a live
//!    frame stream and fires a raw [`VideoFrame`] callback per frame.
//!    Same shape as the `microphone` SDK. **It never writes a file** —
//!    encoding/persistence is a separate higher-level crate.
//!
//! 2. **The private layer** — [`PrivateLayer`] + [`register`]. An
//!    `Element::External` overlay subtree that recordings don't capture,
//!    via the framework's existing third-party-extension mechanism. Zero
//!    framework-core changes. See [`private_layer`] for the design.
//!
//! ```ignore
//! // bootstrap (only needed for the private layer):
//! screen_recorder::register(&mut backend);
//!
//! // capture:
//! let recorder = ScreenRecorder::new();
//! recorder.request_permission(&Source::ThisApp).await?;
//! let handle = recorder
//!     .start(RecordingConfig::new(), |frame| {
//!         // runs on the capture thread (native) / main thread (web);
//!         // copy out what you need and return — encode/preview/upload
//!         // downstream, off this thread.
//!         let _ = (frame.width, frame.height);
//!     })
//!     .await?;
//! // ... later ...
//! handle.stop().await?;
//! ```
#![deny(missing_docs)]

mod config;
mod error;
mod frame;
pub mod private_layer;

pub use config::{AudioSource, RecordingConfig, Source, WindowSelector, DEFAULT_FPS};
pub use error::RecorderError;
pub use frame::{PixelFormat, VideoFrame};
pub use private_layer::{register, PrivateLayer, PrivateLayerProps};

// ---------------------------------------------------------------------------
// The frame-sink callback bound.
//
// Native capture APIs (ReplayKit, ScreenCaptureKit, MediaProjection,
// Windows.Graphics.Capture, PipeWire) deliver frames on a background
// thread, so the sink must be `Send` there. The web backend runs it on
// the single wasm thread holding non-`Send` JS values, where `Send` is
// both unnecessary and unsatisfiable. One cfg'd marker trait keeps
// `start`'s signature identical on every target — mirrors `microphone`'s
// `AudioCallback`. The frame is passed **by reference** so the backend
// can keep ownership of the mapped buffer for the call's duration
// (zero-copy) and unlock/recycle it on return.
// ---------------------------------------------------------------------------

/// The bound a per-frame sink must satisfy. Implemented automatically for
/// any matching closure — pass a `|frame| { .. }` to
/// [`ScreenRecorder::start`], never write `impl FrameCallback` yourself.
///
/// On native targets this requires `Send` (the sink runs on the capture
/// thread); on web it does not (it runs on the main thread). `FnMut`, so
/// the sink may own and mutate state across frames. Keep it fast — copy
/// out what you need and return; heavy work belongs off this thread.
#[cfg(not(target_arch = "wasm32"))]
pub trait FrameCallback: FnMut(&VideoFrame) + Send + 'static {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: FnMut(&VideoFrame) + Send + 'static> FrameCallback for T {}

/// See the non-wasm definition; on web the `Send` bound is dropped.
#[cfg(target_arch = "wasm32")]
pub trait FrameCallback: FnMut(&VideoFrame) + 'static {}
#[cfg(target_arch = "wasm32")]
impl<T: FnMut(&VideoFrame) + 'static> FrameCallback for T {}

/// The boxed form the `imp` backends actually receive. Mirrors
/// [`FrameCallback`]'s cfg'd `Send`-ness.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) type BoxedFrameCallback = Box<dyn FnMut(&VideoFrame) + Send + 'static>;
#[cfg(target_arch = "wasm32")]
pub(crate) type BoxedFrameCallback = Box<dyn FnMut(&VideoFrame) + 'static>;

// One capture backend is compiled per target. Each `imp` module supplies
// `request_permission`, `start`, and the `Recording` handle type. The
// skeleton's modules all return `RecorderError::Unsupported`.
#[cfg_attr(target_arch = "wasm32", path = "web.rs")]
#[cfg_attr(all(not(target_arch = "wasm32"), target_os = "ios"), path = "ios.rs")]
#[cfg_attr(all(not(target_arch = "wasm32"), target_os = "macos"), path = "macos.rs")]
#[cfg_attr(all(not(target_arch = "wasm32"), target_os = "android"), path = "android.rs")]
#[cfg_attr(all(not(target_arch = "wasm32"), target_os = "windows"), path = "windows.rs")]
#[cfg_attr(all(not(target_arch = "wasm32"), target_os = "linux"), path = "linux.rs")]
#[cfg_attr(
    all(
        not(target_arch = "wasm32"),
        not(target_os = "ios"),
        not(target_os = "macos"),
        not(target_os = "android"),
        not(target_os = "windows"),
        not(target_os = "linux")
    ),
    path = "unsupported.rs"
)]
mod imp;

/// Backend-agnostic entry point. Construct once; reuse across recordings.
pub struct ScreenRecorder {
    _private: (),
}

impl Default for ScreenRecorder {
    fn default() -> Self {
        Self::new()
    }
}

impl ScreenRecorder {
    /// Create a recorder. Cheap; does no platform work until
    /// [`ScreenRecorder::start`].
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Trigger the platform consent flow for `source` (ReplayKit /
    /// MediaProjection / Screen Recording TCC / portal / picker). No
    /// target permits silent capture except a Windows own-window grab,
    /// so this is always modeled as async and may reject.
    pub async fn request_permission(&self, source: &Source) -> Result<(), RecorderError> {
        imp::request_permission(source).await
    }

    /// Begin recording. `on_frame` fires once per captured frame (on the
    /// capture thread on native targets, the main thread on web) until
    /// the returned [`RecordingHandle`] is stopped or dropped. The frame
    /// is borrowed — copy out what you need and return promptly.
    pub async fn start<C: FrameCallback>(
        &self,
        config: RecordingConfig,
        on_frame: C,
    ) -> Result<RecordingHandle, RecorderError> {
        let boxed: BoxedFrameCallback = Box::new(on_frame);
        let inner = imp::start(config, boxed).await?;
        Ok(RecordingHandle { inner })
    }
}

/// A live recording. Drop or call [`RecordingHandle::stop`] to end it.
pub struct RecordingHandle {
    inner: imp::Recording,
}

impl RecordingHandle {
    /// Pause capture without tearing down the session.
    pub fn pause(&self) -> Result<(), RecorderError> {
        self.inner.pause()
    }

    /// Resume a paused capture.
    pub fn resume(&self) -> Result<(), RecorderError> {
        self.inner.resume()
    }

    /// Stop capture and release the platform session. Resolves once the
    /// backend has flushed and torn down.
    pub async fn stop(self) -> Result<(), RecorderError> {
        self.inner.stop().await
    }
}
