//! Cross-platform video-**file** decoding into live streams + a transport.
//!
//! Where [`camera`](https://docs.rs/camera) and `screen-recorder` open a live
//! *capture* source, this SDK opens a *clip* (a file path / URL), decodes it on
//! the platform's own media stack, and produces the SAME currency the capture
//! SDKs do — a [`MediaStream`] of tightly-packed `RGBA8` frames and an optional
//! [`AudioStream`] of interleaved-`f32` PCM — plus a [`Transport`] handle to
//! play / pause / seek / mute and to read position + duration.
//!
//! ```no_run
//! use video_decode::{VideoDecoder, DecodeConfig, DecodeSource};
//! # async fn demo() -> Result<(), video_decode::VideoDecodeError> {
//! let dec = VideoDecoder::new();
//! let clip = dec.open(DecodeSource::url("file:///clip.mp4"), DecodeConfig::default()).await?;
//!
//! // Composite the frames into a GPU canvas (poll `latest()` on a tick, or
//! // `subscribe`), and hand `clip.audio()` to `media-writer` for the mux.
//! clip.transport().play();
//! let _pos = clip.transport().position();   // seconds, for a scrubber
//! # let _ = clip.frames();
//! # Ok(())
//! # }
//! ```
//!
//! # Why this exists
//! The native `video` SDK renders a clip in an OS player **overlay view**,
//! outside any GPU canvas scene — so it can't be drawn over and a canvas
//! surface-capture recorder never sees it. This SDK instead hands the decoded
//! pixels back as a [`MediaStream`], so a canvas can composite the clip *into*
//! its scene (ink draws over it; the recorder captures it) and the [`AudioStream`]
//! feeds the recording's audio mux.
//!
//! # Architecture
//! The platform-agnostic surface ([`VideoDecoder`], [`DecodeConfig`],
//! [`DecodedVideo`], [`Transport`], [`VideoDecodeError`]) lives here; the
//! stream surface ([`MediaStream`] / [`AudioStream`]) is re-exported from
//! `media-stream`. Exactly one cfg-gated backend module compiles per target and
//! supplies an `imp` with `open(source, config, FrameWriter, AudioWriter) ->
//! (StreamHandle, Rc<dyn TransportControl>)`; the `StreamHandle`'s `Drop` tears
//! decode down.
//!
//! - **web (wasm32)** — a hidden `<video>` + offscreen `<canvas>` frame pump;
//!   WebAudio `MediaElementSource → ScriptProcessor` PCM tap.
//! - **iOS / macOS** — `AVPlayer` + `AVPlayerItemVideoOutput` (frames) + an
//!   `MTAudioProcessingTap` on the item's audio mix (PCM).
//! - **Android** — `MediaExtractor` + `MediaCodec` via a Kotlin shim.
//! - **other (desktop Linux/Windows)** — not implemented; returns
//!   [`VideoDecodeError::Unsupported`].

#![deny(missing_docs)]

use std::rc::Rc;

pub use media_stream::{AudioStream, MediaStream};

mod error;
pub use error::VideoDecodeError;

// ---------------------------------------------------------------------------
// Backend selector. Exactly one compiles per target; each supplies an `imp`
// module with `open(...)` and a `StreamHandle` whose `Drop` stops decode.
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[path = "web.rs"]
mod imp;

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
#[path = "android.rs"]
mod imp;

#[cfg(all(
    any(target_os = "ios", target_os = "macos"),
    not(target_arch = "wasm32")
))]
#[path = "apple.rs"]
mod imp;

/// TEST/DEBUG hook: reproduce the macOS frame-pump path on the host (see
/// [`imp::debug_pull_first_frame`]). Not part of the public API.
#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
#[doc(hidden)]
pub use imp::debug_pull_first_frame;

#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    target_os = "ios",
    target_os = "macos"
)))]
#[path = "stub.rs"]
mod imp;

// ---------------------------------------------------------------------------
// Source + config.
// ---------------------------------------------------------------------------

/// What to decode. A URL the platform media stack fetches/opens — a `file://`
/// path, an `http(s)` URL, or a `data:` URI. (A raw-bytes variant can be added
/// later behind `#[non_exhaustive]` without breaking callers.)
#[non_exhaustive]
#[derive(Clone, Debug)]
pub enum DecodeSource {
    /// A URL the platform player resolves: `file://…`, `https://…`, `data:…`.
    Url(String),
    /// Raw clip bytes (e.g. a file the picker handed back with no filesystem
    /// path — the web case). On the web this becomes an in-memory `Blob` object
    /// URL; on native it's written to a temp file and opened as a `file://` URL.
    Bytes(Vec<u8>),
}

impl DecodeSource {
    /// Build a [`DecodeSource::Url`] from anything string-like.
    pub fn url(s: impl Into<String>) -> Self {
        DecodeSource::Url(s.into())
    }
    /// Build a [`DecodeSource::Bytes`] from owned clip bytes.
    pub fn bytes(data: Vec<u8>) -> Self {
        DecodeSource::Bytes(data)
    }
}

/// On native targets, materialize [`DecodeSource::Bytes`] to a temp `file://`
/// URL so the platform player (which wants a URL) can open it; pass any `Url`
/// through unchanged. (On web, `Bytes` is handled in-backend as a `Blob` URL, so
/// this is a no-op there.)
#[cfg(not(target_arch = "wasm32"))]
fn materialize_source(source: DecodeSource) -> Result<DecodeSource, VideoDecodeError> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    match source {
        DecodeSource::Url(u) => Ok(DecodeSource::Url(u)),
        DecodeSource::Bytes(data) => {
            let n = SEQ.fetch_add(1, Ordering::Relaxed);
            let mut path = std::env::temp_dir();
            path.push(format!("idealyst-video-{}-{}.mp4", std::process::id(), n));
            std::fs::write(&path, &data)
                .map_err(|e| VideoDecodeError::Backend(format!("temp write failed: {e}")))?;
            Ok(DecodeSource::Url(format!("file://{}", path.display())))
        }
    }
}

/// How to decode + how playback begins. All fields default to a paused, unmuted,
/// non-looping clip decoded at its natural size.
#[derive(Clone, Debug)]
pub struct DecodeConfig {
    /// Begin playback immediately once decode is ready.
    pub autoplay: bool,
    /// Restart from the beginning at end-of-stream.
    pub loop_playback: bool,
    /// Start with the audio track silenced at the *player* (the [`AudioStream`]
    /// still carries PCM for the recorder regardless — muting is a playback-only
    /// concern). Toggle later via [`Transport::set_muted`].
    pub muted: bool,
    /// Bound the decoded frame's longest side to this many pixels (the backend
    /// downscales best-effort), so per-frame GPU uploads stay cheap when the clip
    /// is composited into a canvas. `None` decodes at natural size.
    pub max_dimension: Option<u32>,
}

impl Default for DecodeConfig {
    fn default() -> Self {
        Self {
            autoplay: false,
            loop_playback: false,
            muted: false,
            max_dimension: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Transport — imperative control + polled state, backed per-platform.
// ---------------------------------------------------------------------------

/// Per-platform playback control + state, implemented by each backend over its
/// native player (`AVPlayer`, the web `<video>`, Android `MediaCodec` clock).
/// Main-thread only (these touch native UI/media objects), matching
/// [`MediaStream`]'s `!Send` posture. All methods are defaulted so a backend
/// that hasn't wired one degrades to a no-op / zero rather than panicking.
pub trait TransportControl {
    /// Start (or resume) playback.
    fn play(&self) {}
    /// Pause, leaving the position intact.
    fn pause(&self) {}
    /// Seek to `seconds` from the start (clamped to the clip by the backend).
    /// EXACT — decodes to the precise frame. Use on a scrub's final landing.
    fn seek(&self, _seconds: f32) {}
    /// Approximate, fast seek for live scrubbing — backends may jump to the
    /// nearest decodable point (e.g. web `fastSeek`) so frames flow quickly while
    /// dragging. Defaults to an exact [`seek`](Self::seek) where there's no fast
    /// path. Follow up with [`seek`](Self::seek) when the drag ends for precision.
    fn seek_preview(&self, seconds: f32) {
        self.seek(seconds);
    }
    /// Mute/unmute the *player's* audio output (does not affect the recorder's
    /// [`AudioStream`] tap).
    fn set_muted(&self, _muted: bool) {}
    /// Set the playback rate (`1.0` = normal). `0.0` is equivalent to pause.
    fn set_rate(&self, _rate: f32) {}
    /// Current playback position in seconds, or `0.0` if unknown.
    fn position(&self) -> f32 {
        0.0
    }
    /// Total duration in seconds, or `0.0` if not yet known / indeterminate
    /// (live). Poll after `open` — duration often resolves a beat after load.
    fn duration(&self) -> f32 {
        0.0
    }
    /// Whether the clip is currently playing (rate > 0 and not ended).
    fn is_playing(&self) -> bool {
        false
    }
    /// Whether the player's audio output is muted.
    fn is_muted(&self) -> bool {
        false
    }
}

/// A cloneable handle to a decoded clip's playback. Cheap to clone (an `Rc`).
/// Obtained from [`DecodedVideo::transport`].
#[derive(Clone)]
pub struct Transport(Rc<dyn TransportControl>);

impl Transport {
    /// Wrap a backend control impl.
    fn new(ctrl: Rc<dyn TransportControl>) -> Self {
        Self(ctrl)
    }
    /// Start (or resume) playback.
    pub fn play(&self) {
        self.0.play();
    }
    /// Pause, leaving the position intact.
    pub fn pause(&self) {
        self.0.pause();
    }
    /// Seek to `seconds` from the start (exact — for a scrub's final landing).
    pub fn seek(&self, seconds: f32) {
        self.0.seek(seconds);
    }
    /// Approximate, fast seek for live scrubbing (see
    /// [`TransportControl::seek_preview`]).
    pub fn seek_preview(&self, seconds: f32) {
        self.0.seek_preview(seconds);
    }
    /// Mute/unmute the player's audio output.
    pub fn set_muted(&self, muted: bool) {
        self.0.set_muted(muted);
    }
    /// Set the playback rate (`1.0` = normal).
    pub fn set_rate(&self, rate: f32) {
        self.0.set_rate(rate);
    }
    /// Current playback position in seconds.
    pub fn position(&self) -> f32 {
        self.0.position()
    }
    /// Total duration in seconds (`0.0` if unknown).
    pub fn duration(&self) -> f32 {
        self.0.duration()
    }
    /// Whether the clip is currently playing.
    pub fn is_playing(&self) -> bool {
        self.0.is_playing()
    }
    /// Whether the player's audio output is muted.
    pub fn is_muted(&self) -> bool {
        self.0.is_muted()
    }
}

// ---------------------------------------------------------------------------
// DecodedVideo — the result bundle.
// ---------------------------------------------------------------------------

/// A decoded, playable clip: its video-frame [`MediaStream`], its optional audio
/// [`AudioStream`] (`None` when the clip has no audio track), and a [`Transport`].
///
/// Decode runs while a clone of [`frames`](Self::frames) (or this struct) is
/// alive; dropping the last one tears the decoder down.
pub struct DecodedVideo {
    frames: MediaStream,
    audio: Option<AudioStream>,
    transport: Transport,
    /// The clip's natural pixel size, if the backend reported it at open.
    natural_size: Option<(u32, u32)>,
}

impl DecodedVideo {
    /// The decoded video frames (`RGBA8`). Poll [`MediaStream::latest`] +
    /// [`MediaStream::generation`] on a render tick to blit the newest frame, or
    /// [`MediaStream::subscribe`] for push delivery.
    pub fn frames(&self) -> &MediaStream {
        &self.frames
    }
    /// The decoded audio PCM, or `None` if the clip has no audio track. Hand to
    /// `media-writer` to mux the clip's sound into a recording.
    pub fn audio(&self) -> Option<&AudioStream> {
        self.audio.as_ref()
    }
    /// Playback control + state.
    pub fn transport(&self) -> &Transport {
        &self.transport
    }
    /// The clip's natural `(width, height)` in pixels, if known at open.
    pub fn natural_size(&self) -> Option<(u32, u32)> {
        self.natural_size
    }
}

/// What a backend's `imp::open` returns: an opaque, `Drop`-ful decoder handle
/// (held to keep decode alive, dropped to tear it down), the transport control,
/// whether the clip carries an audio track, and its natural size if known.
pub(crate) struct Opened {
    /// The backend's `StreamHandle` (or equivalent), type-erased — we only ever
    /// hold then drop it, so `Any` suffices and its `Drop` stops decode.
    pub handle: Box<dyn std::any::Any>,
    pub control: Rc<dyn TransportControl>,
    pub has_audio: bool,
    pub natural_size: Option<(u32, u32)>,
}

// ---------------------------------------------------------------------------
// Public API.
// ---------------------------------------------------------------------------

/// A handle that opens video clips for decode. Cheap to construct and clone; it
/// holds no resources until you [`open`](VideoDecoder::open) a clip.
#[derive(Clone, Default)]
pub struct VideoDecoder {
    _private: (),
}

impl VideoDecoder {
    /// Create a decoder handle.
    pub fn new() -> Self {
        Self::default()
    }

    /// Open and begin decoding `source`, returning a [`DecodedVideo`]. Decode
    /// runs while the returned value (or its frames stream) is alive; dropping it
    /// stops decode and releases the player.
    pub async fn open(
        &self,
        source: DecodeSource,
        config: DecodeConfig,
    ) -> Result<DecodedVideo, VideoDecodeError> {
        // Native: a `Bytes` source becomes a temp `file://` URL here, so the
        // per-platform backend only ever sees a `Url`. Web handles `Bytes`
        // directly (a `Blob` object URL), so it's passed through unchanged there.
        #[cfg(not(target_arch = "wasm32"))]
        let source = materialize_source(source)?;

        let (frames, frame_writer) = MediaStream::new();
        let (audio, audio_writer) = AudioStream::new();
        let Opened {
            handle,
            control,
            has_audio,
            natural_size,
        } = imp::open(source, config, frame_writer, audio_writer).await?;
        // One teardown owns the decoder; run it when the last frames-stream clone
        // drops (the audio stream rides the same lifetime).
        let handle = Rc::new(std::cell::RefCell::new(Some(handle)));
        let h2 = handle.clone();
        frames.attach_stopper(move || {
            h2.borrow_mut().take();
        });
        let audio = if has_audio {
            // Keep the decoder alive as long as the audio stream is held too.
            let h3 = handle.clone();
            audio.attach_stopper(move || {
                h3.borrow_mut().take();
            });
            Some(audio)
        } else {
            None
        };
        Ok(DecodedVideo {
            frames,
            audio,
            transport: Transport::new(control),
            natural_size,
        })
    }
}
