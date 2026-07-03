//! Record live media streams to a file.
//!
//! `media-writer` is the *consumer* end of the `media-stream` vocabulary: it
//! takes a video [`MediaStream`] (produced by `camera` / `screen-recorder`)
//! and/or an audio [`AudioStream`] (produced by `microphone`) and muxes them
//! to a playable file, lip-syncing the two by the shared-clock `pts_micros`
//! every producer stamps onto its frames.
//!
//! ```no_run
//! use media_writer::{MediaWriter, MediaInputs, RecordConfig};
//! # async fn demo(cam: &media_writer::MediaStream, mic: &media_writer::AudioStream)
//! #     -> Result<(), Box<dyn std::error::Error>> {
//! let store = files::app_files("recordings")?;
//! let writer = MediaWriter::new();
//!
//! // Start recording camera video + mic audio into recordings/clip.mp4.
//! let recording = writer
//!     .record(MediaInputs::av(cam, mic), RecordConfig::new(store, "clip.mp4"))
//!     .await?;
//!
//! // ... later: finalize the file and get its path back.
//! let path = recording.stop().await?;
//! # let _ = path;
//! # Ok(())
//! # }
//! ```
//!
//! # Architecture
//!
//! The platform-agnostic surface ([`MediaWriter`], [`MediaInputs`],
//! [`Recording`], [`RecordConfig`], [`Container`], [`MediaWriterError`]) lives
//! here; the stream types are re-exported from `media-stream`. Exactly one
//! cfg-gated backend module compiles per target, each driving the platform's
//! native encoder/muxer:
//!
//! - **iOS / macOS** — `AVAssetWriter` (H.264 + AAC) over AVFoundation.
//! - **Android** — `MediaCodec` + `MediaMuxer` via a Kotlin shim.
//! - **web (wasm32)** — `MediaRecorder` over the streams' native
//!   `web_sys::MediaStream`; the recorded `Blob` is written through the
//!   `files` store.
//! - **other** — returns [`MediaWriterError::Unsupported`].
//!
//! Recording requires no permission of its own: the `camera` / `microphone` /
//! `screen-recorder` SDKs already gate capture, and this SDK only consumes the
//! streams they hand out and writes to the app's own files.

#![deny(missing_docs)]

mod config;
mod error;

pub use config::{Container, RecordConfig, DEFAULT_FPS};
pub use error::MediaWriterError;

// The streams this SDK consumes are the shared `media-stream` vocabulary;
// re-export so a user has everything from `media_writer::`.
pub use media_stream::{AudioStream, MediaStream};

// ---------------------------------------------------------------------------
// Backend selector. Exactly one compiles per target; each supplies an `imp`
// module with `start(inputs, config)` and a `RecordingHandle` whose `stop()`
// finalizes the file.
// ---------------------------------------------------------------------------

#[cfg(all(any(target_os = "ios", target_os = "macos"), not(target_arch = "wasm32")))]
#[path = "apple.rs"]
mod imp;

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
#[path = "android.rs"]
mod imp;

#[cfg(target_arch = "wasm32")]
#[path = "web.rs"]
mod imp;

#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    target_os = "ios",
    target_os = "macos"
)))]
#[path = "stub.rs"]
mod imp;

// Waker-based single-shot signal used by the web backend's `stop()`. Compiled
// on every target (not just wasm32) so its regression tests — which pin that it
// parks instead of spin-looping the executor — run under a plain host
// `cargo test`. Unused outside the wasm backend + tests, hence the gated allow.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod oneshot;

// ---------------------------------------------------------------------------
// Public API.
// ---------------------------------------------------------------------------

/// The set of live sources to record. At least one of [`video`](Self::video) /
/// [`audio`](Self::audio) must be present; with both, the writer muxes them
/// into one file aligned by their shared capture timestamps.
///
/// The references are only borrowed for the duration of
/// [`record`](MediaWriter::record): the backend subscribes to each stream
/// (each subscription independently keeps the capture alive), so the inputs
/// need not outlive the call.
pub struct MediaInputs<'a> {
    /// The video source to record, if any.
    pub video: Option<&'a MediaStream>,
    /// The audio source to record, if any.
    pub audio: Option<&'a AudioStream>,
}

impl<'a> MediaInputs<'a> {
    /// Record video only.
    pub fn video(stream: &'a MediaStream) -> Self {
        Self {
            video: Some(stream),
            audio: None,
        }
    }

    /// Record audio only.
    pub fn audio(stream: &'a AudioStream) -> Self {
        Self {
            video: None,
            audio: Some(stream),
        }
    }

    /// Record video + audio, muxed into one file.
    pub fn av(video: &'a MediaStream, audio: &'a AudioStream) -> Self {
        Self {
            video: Some(video),
            audio: Some(audio),
        }
    }
}

/// Records [`MediaInputs`] to a file. Cheap to construct and clone; holds no
/// resources until you [`record`](Self::record).
#[derive(Clone, Default)]
pub struct MediaWriter {
    _private: (),
}

impl MediaWriter {
    /// Create a writer handle.
    pub fn new() -> Self {
        Self::default()
    }

    /// Begin recording `inputs` to the destination in `config`. Returns a
    /// [`Recording`] that captures until you call [`Recording::stop`] (or drop
    /// it, which aborts without finalizing — always `stop()` to get a playable
    /// file).
    ///
    /// Errors with [`MediaWriterError::NoInput`] if `inputs` has neither video
    /// nor audio, and [`MediaWriterError::Unsupported`] on targets without an
    /// encoder backend.
    pub async fn record(
        &self,
        inputs: MediaInputs<'_>,
        config: RecordConfig,
    ) -> Result<Recording, MediaWriterError> {
        if inputs.video.is_none() && inputs.audio.is_none() {
            return Err(MediaWriterError::NoInput);
        }
        let path = config.path.clone();
        let handle = imp::start(inputs, &config).await?;
        Ok(Recording { handle, path })
    }
}

/// An in-progress recording. Capture runs until [`stop`](Self::stop). Dropping
/// it without stopping tears the encoder down and discards the partial file.
pub struct Recording {
    handle: imp::RecordingHandle,
    path: String,
}

impl Recording {
    /// Stop capturing, finalize the container, and flush the file. Resolves to
    /// the [`path`](RecordConfig) the recording was written to (relative to the
    /// store it was given), ready to read back or hand to a player.
    pub async fn stop(self) -> Result<String, MediaWriterError> {
        self.handle.stop().await?;
        Ok(self.path)
    }

    /// The path (relative to the configured store) the recording is being
    /// written to.
    pub fn path(&self) -> &str {
        &self.path
    }
}
