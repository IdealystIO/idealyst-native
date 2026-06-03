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
//! // capture — yields a `MediaStream`, the same currency `camera` produces
//! // and the `video` SDK displays:
//! let stream = ScreenRecorder::new().start(RecordingConfig::new()).await?;
//!
//! // tap raw RGBA8 frames (capture thread on native, main thread on web)…
//! let sub = stream.subscribe(|frame| { let _ = (frame.width, frame.height); });
//! // …or hand `stream` to the `video` SDK to show the live screen.
//! // Drop `sub` to stop tapping; drop `stream` to stop capture.
//! ```
#![deny(missing_docs)]

mod config;
mod error;
pub mod private_layer;

pub use config::{AudioSource, RecordingConfig, Source, WindowSelector, DEFAULT_FPS};
pub use error::RecorderError;
pub use private_layer::{register, PrivateLayer, PrivateLayerProps};

// The live-source surface is the shared `media-stream` vocabulary — the same
// currency the `camera` SDK produces and the `video` SDK consumes. Re-export
// it so a screen-recorder user has everything from `screen_recorder::`.
pub use media_stream::{FrameCallback, MediaStream, PixelFormat, Subscription, VideoFrame};

/// The type-erased zero-copy frame source a backend may publish on the stream
/// (e.g. the web `web_sys::MediaStream`), downcast by a same-platform display
/// / GPU consumer. `None` where no zero-copy source is exposed (yet).
pub(crate) type NativeSource = std::rc::Rc<dyn std::any::Any>;

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

/// Backend-agnostic entry point. Cheap to construct and clone; it holds no OS
/// resources until you [`start`](ScreenRecorder::start) a recording.
#[derive(Clone, Default)]
pub struct ScreenRecorder {
    _private: (),
}

impl ScreenRecorder {
    /// Create a recorder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Proactively trigger the platform consent flow for `source` (ReplayKit
    /// / MediaProjection / Screen Recording TCC / portal / picker) without
    /// starting capture. Optional — [`start`](ScreenRecorder::start) runs the
    /// consent flow itself. On most targets the prompt is only shown at
    /// `start` (e.g. the web `getDisplayMedia` picker, ReplayKit's capture
    /// consent), so this resolves `Ok` to defer to that call.
    pub async fn request_permission(&self, source: &Source) -> Result<(), RecorderError> {
        imp::request_permission(source).await
    }

    /// Begin capturing and return a live [`MediaStream`] — the same
    /// platform-agnostic source the `camera` SDK produces and the `video` SDK
    /// displays. Capture runs while any clone of the stream is alive; dropping
    /// the last one stops it and tears down the platform session.
    ///
    /// Tap raw RGBA8 frames with [`MediaStream::subscribe`] /
    /// [`MediaStream::latest`], or hand the stream to the `video` SDK to show
    /// the live screen. May surface the OS capture-consent prompt.
    pub async fn start(&self, config: RecordingConfig) -> Result<MediaStream, RecorderError> {
        let (stream, writer) = MediaStream::new();
        let (recording, native) = imp::start(config, writer).await?;
        if let Some(src) = native {
            stream.set_native_source(src);
        }
        // The backend `Recording` owns the capture session + the `FrameWriter`
        // and stops capture on drop. Own it in the stream's stopper so it tears
        // down when the last `MediaStream` clone drops — the same ownership
        // lifecycle as `camera`.
        stream.attach_stopper(move || drop(recording));
        Ok(stream)
    }
}
