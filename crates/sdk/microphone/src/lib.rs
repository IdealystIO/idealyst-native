//! Cross-platform microphone capture.
//!
//! The smallest useful abstraction over the platform's audio input: open
//! a stream, get raw PCM frames in a callback, drop the stream to stop.
//! No files, no encoding, no opinion about where the audio goes — that's
//! for higher-level SDKs to layer on top. This crate just establishes the
//! stream.
//!
//! ```no_run
//! use microphone::{Microphone, AudioStreamConfig};
//!
//! # async fn demo() -> Result<(), microphone::MicError> {
//! let mic = Microphone::new();
//! // Keep the returned stream alive for as long as you want to capture.
//! let stream = mic
//!     .open(AudioStreamConfig::default().mono(), |buf| {
//!         // Runs on the audio thread (native/Android) or the main
//!         // thread (web). Copy out what you need and return fast.
//!         let peak = buf.samples.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
//!         let _ = peak;
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
//! The platform-agnostic surface ([`Microphone`], [`MicStream`],
//! [`AudioBuffer`], [`AudioStreamConfig`], [`MicError`]) lives here.
//! Exactly one cfg-gated backend module is compiled per target and
//! supplies the `imp` submodule the public API delegates to:
//!
//! - **web (wasm32)** — `getUserMedia` + a Web Audio graph.
//! - **iOS / macOS / Windows / Linux** — `cpal` (CoreAudio / WASAPI /
//!   ALSA / AudioUnit). iOS additionally activates an `AVAudioSession`.
//! - **Android** — `AudioRecord` read on a dedicated JNI worker thread.
//!
//! # Permissions
//!
//! The app must declare the platform's microphone permission, or capture
//! is denied at runtime:
//!
//! - **iOS / macOS** — `NSMicrophoneUsageDescription` in `Info.plist`.
//! - **Android** — `<uses-permission android:name="android.permission.RECORD_AUDIO"/>`.
//! - **web** — none; the browser prompts on first `getUserMedia`.
//!
//! The runtime *grant* flow (reading the current status and surfacing the OS
//! prompt) is delegated to the shared [`permissions`] SDK
//! (`permissions::request(Permission::Microphone)`); this crate keeps only the
//! capture code (cpal / `AudioRecord` / the iOS `AVAudioSession` activation).
//! [`Microphone::request_permission`] proactively triggers that prompt
//! (and is a no-op where the OS prompts implicitly), but it's optional —
//! [`Microphone::open`] requests access itself if needed.

#![deny(missing_docs)]

mod buffer;
mod config;
mod error;

pub use buffer::AudioBuffer;
pub use config::AudioStreamConfig;
pub use error::MicError;

// The live-source surface, re-exported so a microphone user has everything
// from `microphone::`. `microphone` is to `AudioStream` what `camera` is to
// `MediaStream`: a producer. See [`Microphone::open_stream`].
pub use media_stream::{
    AudioFormat, AudioFrame, AudioFrameCallback, AudioStream, AudioSubscription, AudioWriter,
};

/// The type-erased native audio source a backend may publish on an
/// [`AudioStream`] (e.g. the web `web_sys::MediaStream`), downcast by a
/// same-platform playback layer. `None` where no native source is exposed.
pub(crate) type NativeSource = std::rc::Rc<dyn std::any::Any>;

// ---------------------------------------------------------------------------
// The callback bound.
//
// cpal (native) and the Android reader thread move the callback onto a
// separate thread, so it must be `Send` there. The web backend runs it on
// the single wasm thread inside a `ScriptProcessorNode` handler holding
// non-`Send` JS values, so `Send` is both unnecessary and unsatisfiable.
// One marker trait, cfg'd, keeps the public `open` signature identical on
// every target while enforcing the right bound underneath.
// ---------------------------------------------------------------------------

/// The bound a capture callback must satisfy. Implemented automatically
/// for any matching closure — you never write `impl AudioCallback`
/// yourself, just pass a `|buf| { .. }` closure to [`Microphone::open`].
///
/// On native and Android targets this requires `Send` (the callback runs
/// on the audio/reader thread); on web it does not (it runs on the main
/// thread). The closure is `FnMut`, so it may own and mutate state across
/// chunks.
#[cfg(not(target_arch = "wasm32"))]
pub trait AudioCallback: FnMut(&AudioBuffer) + Send + 'static {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: FnMut(&AudioBuffer) + Send + 'static> AudioCallback for T {}

/// See the non-wasm definition; on web the `Send` bound is dropped.
#[cfg(target_arch = "wasm32")]
pub trait AudioCallback: FnMut(&AudioBuffer) + 'static {}
#[cfg(target_arch = "wasm32")]
impl<T: FnMut(&AudioBuffer) + 'static> AudioCallback for T {}

/// The boxed form backends actually receive. Mirrors [`AudioCallback`]'s
/// cfg'd `Send`-ness.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) type BoxedCallback = Box<dyn FnMut(&AudioBuffer) + Send + 'static>;
#[cfg(target_arch = "wasm32")]
pub(crate) type BoxedCallback = Box<dyn FnMut(&AudioBuffer) + 'static>;

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

#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
#[path = "native.rs"]
mod imp;

// ---------------------------------------------------------------------------
// Public API.
// ---------------------------------------------------------------------------

/// Current microphone permission, queryable WITHOUT prompting or opening the
/// input — unlike [`request_permission`](Microphone::request_permission), which
/// may surface the OS prompt, or [`open`](Microphone::open), which activates the
/// mic. Lets a host seed UI state (e.g. default the mic muted unless
/// [`Granted`](MicPermission::Granted)) without touching the microphone.
///
/// `Unknown` is returned where the platform exposes no passive query — desktop
/// Windows/Linux (cpal grants implicitly, with no status API), or a web browser
/// whose Permissions API lacks a `microphone` descriptor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MicPermission {
    /// Access granted — capture will succeed without a prompt.
    Granted,
    /// Access denied (or restricted by policy) — capture fails until the user
    /// changes it in system settings.
    Denied,
    /// Not yet decided — the next [`open`](Microphone::open) /
    /// [`request_permission`](Microphone::request_permission) will prompt.
    Undetermined,
    /// The platform offers no passive status query.
    Unknown,
}

/// A handle to the device's microphone. Cheap to construct and clone; it
/// holds no OS resources until you [`open`](Microphone::open) a stream.
#[derive(Clone, Default)]
pub struct Microphone {
    _private: (),
}

impl Microphone {
    /// Create a microphone handle.
    pub fn new() -> Self {
        Self::default()
    }

    /// Proactively request microphone permission, triggering the OS
    /// prompt on platforms that have one (iOS, Android, web). Resolves
    /// `Ok(())` once access is granted, [`MicError::PermissionDenied`] if
    /// refused. A no-op `Ok(())` where the platform grants implicitly
    /// (Windows, Linux) or prompts only on capture start.
    ///
    /// Optional — [`open`](Microphone::open) requests access on its own.
    /// Call this when you want the prompt to appear before, say, showing
    /// a recording UI.
    pub async fn request_permission(&self) -> Result<(), MicError> {
        imp::request_permission().await
    }

    /// Query the current microphone permission WITHOUT prompting or activating
    /// the mic. Use it to seed UI state — e.g. start the mic muted unless the
    /// status is [`MicPermission::Granted`] — so the OS prompt only appears later,
    /// on a deliberate capture or [`request_permission`](Self::request_permission).
    ///
    /// Returns [`MicPermission::Unknown`] where the platform has no passive query
    /// (desktop Windows/Linux; some browsers). Delegated to the shared
    /// [`permissions`] SDK — `AVCaptureDevice` authorization status on
    /// iOS/macOS, `navigator.permissions.query` on web, and
    /// `checkSelfPermission` on Android — and mapped onto [`MicPermission`].
    pub async fn permission_status(&self) -> MicPermission {
        imp::permission_status().await
    }

    /// Open a live capture stream. `callback` fires with each chunk of
    /// PCM frames (see [`AudioBuffer`]) until the returned [`MicStream`]
    /// is dropped (or [`stop`](MicStream::stop)ped).
    ///
    /// Requests permission if it hasn't been granted yet, so this can
    /// surface the OS prompt. The callback runs on the audio thread on
    /// native/Android targets and on the main thread on web — keep it
    /// fast and non-blocking; copy samples out rather than processing
    /// heavily in place.
    pub async fn open<C: AudioCallback>(
        &self,
        config: AudioStreamConfig,
        callback: C,
    ) -> Result<MicStream, MicError> {
        let boxed: BoxedCallback = Box::new(callback);
        let handle = imp::open(config, boxed).await?;
        Ok(MicStream { _handle: handle })
    }

    /// Open a live capture stream as an [`AudioStream`] — the platform-agnostic
    /// audio peer of [`MediaStream`](media_stream::MediaStream), the same
    /// currency `camera` produces for video.
    ///
    /// Use this (instead of the raw-callback [`open`](Self::open)) when you
    /// want to hand the microphone to a *consumer* that speaks `AudioStream`:
    /// the `media-writer` SDK (record mic audio to a file, optionally muxed
    /// with a camera `MediaStream`), or a future audio-playback layer that
    /// binds the platform's native pipeline. Tap PCM with
    /// [`AudioStream::subscribe`] / [`AudioStream::latest`]; on web the
    /// underlying `web_sys::MediaStream` is published as the stream's
    /// [`native_source`](AudioStream::native_source) for zero-reconstruction
    /// playback / recording.
    ///
    /// Capture runs while any clone of the returned stream is alive; dropping
    /// the last one stops it. Requests permission if needed.
    pub async fn open_stream(&self, config: AudioStreamConfig) -> Result<AudioStream, MicError> {
        let (stream, writer) = AudioStream::new();
        // Bridge the raw-callback backends into the AudioStream producer side:
        // every captured chunk is pushed through the `Send` `AudioWriter`,
        // stamped with the shared capture clock so a muxer can align it with
        // video from another source.
        let boxed: BoxedCallback = Box::new(move |buf: &AudioBuffer| {
            writer.write_pcm_f32(buf.sample_rate, buf.channels, buf.samples);
        });
        let handle = imp::open(config, boxed).await?;
        if let Some(src) = handle.native_source() {
            stream.set_native_source(src);
        }
        // The backend `StreamHandle` stops capture on drop; own it in the
        // stopper so it tears down when the last `AudioStream` clone drops.
        stream.attach_stopper(move || drop(handle));
        Ok(stream)
    }
}

/// A live capture stream. Capture runs for as long as this value is
/// alive; dropping it tears the stream down and stops the callback. Hold
/// onto it (e.g. in your app state) for the duration you want to record.
///
/// Not `Send` on native targets — the underlying platform stream is tied
/// to the thread that opened it. Keep it on that thread.
pub struct MicStream {
    // The concrete type is backend-specific; its `Drop` stops capture.
    _handle: imp::StreamHandle,
}

impl MicStream {
    /// Stop capturing and release the stream. Equivalent to dropping the
    /// value; provided for call sites where an explicit `stop()` reads
    /// clearer than a `drop(stream)`.
    pub fn stop(self) {
        // `self` drops here, running the backend's teardown.
    }
}
