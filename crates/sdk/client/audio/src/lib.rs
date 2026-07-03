//! Cross-platform sound **playback**.
//!
//! Load a sound (from bytes, a file path, or a URL), play it, and get back
//! a thin handle you control. This is the playback peer of the *capture*
//! SDKs (`microphone` / `camera`): where those turn the device's mic /
//! camera into an `AudioStream` / `MediaStream`, this one takes prepared
//! audio and pushes it back out the speakers. No mixing, effects, or
//! background playback — just foreground playback of loaded sounds.
//!
//! ```ignore
//! use audio::{load, AudioSource};
//!
//! # async fn demo() -> Result<(), audio::AudioError> {
//! // Decode / prepare once…
//! let sound = load(AudioSource::path("assets/ding.wav")).await?;
//!
//! // …then play. `play()` returns a `Playback` handle.
//! let playback = sound.play();
//! playback.set_volume(0.5);
//! playback.set_looping(true);
//!
//! // Dropping the handle stops the sound; `stop()` is the explicit form.
//! playback.stop();
//! # Ok(())
//! # }
//! ```
//!
//! # Loading vs. playing
//!
//! [`load`] does the (possibly slow) decode/prepare and is `async`. The
//! resulting [`Sound`] is a cheap, replayable source: call
//! [`Sound::play`] as many times as you like. Each call starts an
//! independent [`Playback`] where the platform supports concurrent
//! playback (web, Apple); see [`Sound::play`] for the one platform caveat.
//!
//! # Lifetime — a playing sound is owned by its handle
//!
//! [`Playback`] is RAII: while it's alive the sound keeps playing (or
//! looping); when it drops, playback stops and the platform player is torn
//! down. Hold onto it (e.g. in your app state) for as long as you want the
//! sound to keep going. [`Playback::stop`] is the explicit equivalent of
//! `drop(playback)` for call sites where that reads clearer.
//!
//! # Per-platform mechanism
//!
//! One author API; each target uses its native player and the *observable
//! behavior* is the same:
//!
//! - **web (wasm32)** — `HTMLAudioElement` (`new Audio()` + a blob/URL
//!   `src`). Runnable on web.
//! - **iOS / macOS / tvOS** — `AVAudioPlayer` (objc2). Compile-checked
//!   only — not device-verified from this repo.
//! - **Android** — `MediaPlayer` (JNI). Compile-checked only.
//! - **Windows / Linux / other native** — no pure-Rust player is pulled in
//!   (kept dependency-light on purpose), so [`load`] returns
//!   [`AudioError::NotSupported`]. Honest fallback rather than a silent
//!   no-op.
//!
//! # Permissions
//!
//! None. Foreground playback needs no OS permission on any platform.
//! *Background* audio (continuing while the app is backgrounded) is a
//! different capability that needs app config — iOS `UIBackgroundModes`
//! `audio`, an Android foreground service — and is out of scope here.

#![deny(missing_docs)]

use std::path::PathBuf;

// Backend selector. Exactly one `imp` compiles per target; each supplies a
// `PreparedSound` (held by `Sound`) and a `PlaybackHandle` (held by
// `Playback`) whose `Drop` stops the sound. The desktop catch-all uses the
// `unsupported` stub, which makes `load` return `NotSupported`.
#[cfg(target_arch = "wasm32")]
#[path = "web.rs"]
mod imp;

#[cfg(all(
    not(target_arch = "wasm32"),
    any(target_os = "ios", target_os = "macos", target_os = "tvos")
))]
#[path = "apple.rs"]
mod imp;

#[cfg(all(not(target_arch = "wasm32"), target_os = "android"))]
#[path = "android.rs"]
mod imp;

#[cfg(all(
    not(target_arch = "wasm32"),
    not(any(target_os = "ios", target_os = "macos", target_os = "tvos")),
    not(target_os = "android")
))]
#[path = "unsupported.rs"]
mod imp;

// Compile-checked usage recipes (catalog feature only).
#[doc(hidden)]
pub mod recipes;

// ---------------------------------------------------------------------------
// Public API.
// ---------------------------------------------------------------------------

/// Where a sound's encoded audio comes from. Pass one to [`load`].
///
/// Use the constructors ([`AudioSource::bytes`], [`AudioSource::path`],
/// [`AudioSource::url`]) for ergonomic call sites.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioSource {
    /// Encoded audio already in memory (e.g. an asset bundled with
    /// `include_bytes!`). The bytes are an encoded container (wav / mp3 /
    /// aac / …), not raw PCM — the platform decoder handles the format.
    Bytes(Vec<u8>),
    /// A local file path the platform player reads directly.
    Path(PathBuf),
    /// A remote URL the platform fetches and plays.
    Url(String),
}

impl AudioSource {
    /// A source from encoded audio bytes already in memory.
    pub fn bytes(bytes: impl Into<Vec<u8>>) -> Self {
        AudioSource::Bytes(bytes.into())
    }

    /// A source from a local file path.
    pub fn path(path: impl Into<PathBuf>) -> Self {
        AudioSource::Path(path.into())
    }

    /// A source from a remote URL.
    pub fn url(url: impl Into<String>) -> Self {
        AudioSource::Url(url.into())
    }
}

/// A playback failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioError {
    /// The underlying platform player failed (I/O, player init, no audio
    /// device, …). The string carries the platform's detail.
    Backend(String),
    /// The audio couldn't be decoded — unrecognized/unsupported format, or
    /// corrupt data.
    Decode(String),
    /// Playback isn't available on this platform (the desktop fallback) or
    /// for this source on this platform.
    NotSupported,
}

impl std::fmt::Display for AudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AudioError::Backend(msg) => write!(f, "audio backend error: {msg}"),
            AudioError::Decode(msg) => write!(f, "audio decode error: {msg}"),
            AudioError::NotSupported => {
                write!(f, "audio playback is not supported on this platform")
            }
        }
    }
}

impl std::error::Error for AudioError {}

/// Decode / prepare a sound from `source`, ready to [`play`](Sound::play).
///
/// `async` because preparation can be slow (decoding, or fetching a URL).
/// The returned [`Sound`] is reusable — play it as many times as you want.
///
/// Returns [`AudioError::Decode`] if the audio can't be decoded,
/// [`AudioError::Backend`] for a player/IO failure, and
/// [`AudioError::NotSupported`] on platforms without a player (desktop
/// Windows / Linux).
pub async fn load(source: AudioSource) -> Result<Sound, AudioError> {
    let prepared = imp::prepare(source).await?;
    Ok(Sound { inner: prepared })
}

/// A loaded, ready-to-play sound. Cheap to keep around and replay; the
/// expensive decode/prepare happened once in [`load`].
///
/// Call [`play`](Sound::play) to start it — each call returns its own
/// [`Playback`] handle. Keep the `Sound` alive as long as you might replay
/// it; dropping it doesn't stop already-started [`Playback`]s (those own
/// their own player), it just means you can't start new ones.
pub struct Sound {
    inner: imp::PreparedSound,
}

impl Sound {
    /// Start playing this sound, returning a [`Playback`] handle that
    /// controls (and, by its `Drop`, owns) the running sound.
    ///
    /// Playback begins immediately. Where the platform supports it (web,
    /// Apple), calling `play` again while an earlier `Playback` is still
    /// alive starts an *independent*, overlapping voice — fine for layering
    /// short sound effects. On **Android** a single backing player is reused
    /// per call, so a second `play()` restarts this sound rather than
    /// layering a second copy; mix short overlapping SFX with a higher
    /// layer (a `SoundPool`-style pool) if you need true polyphony there.
    pub fn play(&self) -> Playback {
        Playback {
            inner: self.inner.play(),
        }
    }
}

/// A handle to one running playback. **RAII**: while this value is alive
/// the sound plays (looping if you asked it to); dropping it — or calling
/// [`stop`](Playback::stop) — stops the sound and releases the platform
/// player.
///
/// A thin control surface: pause / resume / stop, volume, and looping.
/// Anything richer (seeking, fades, effects, position callbacks) is a
/// higher layer's job.
pub struct Playback {
    inner: imp::PlaybackHandle,
}

impl Playback {
    /// Pause playback, keeping position so [`resume`](Playback::resume)
    /// continues where it left off. A no-op if already paused or stopped.
    pub fn pause(&self) {
        self.inner.pause();
    }

    /// Resume after a [`pause`](Playback::pause). A no-op if already
    /// playing or if the sound has finished.
    pub fn resume(&self) {
        self.inner.resume();
    }

    /// Stop playback and release the player now. Equivalent to dropping the
    /// handle; provided for call sites where an explicit `stop()` reads
    /// clearer. After this, the handle's other methods are no-ops.
    pub fn stop(self) {
        // `self` drops here, running the backend teardown.
    }

    /// Set the playback volume in `[0.0, 1.0]` (clamped). `1.0` is the
    /// sound's natural level; `0.0` is silent.
    pub fn set_volume(&self, volume: f32) {
        self.inner.set_volume(volume.clamp(0.0, 1.0));
    }

    /// Loop the sound (`true`) or play it once (`false`). Takes effect from
    /// the next loop boundary; the currently-playing pass isn't interrupted.
    pub fn set_looping(&self, looping: bool) {
        self.inner.set_looping(looping);
    }

    /// Whether the sound is currently producing audio — `true` while
    /// playing, `false` once paused, stopped, or finished (and not
    /// looping).
    pub fn is_playing(&self) -> bool {
        self.inner.is_playing()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn audio_source_constructors() {
        assert_eq!(
            AudioSource::bytes(vec![1u8, 2, 3]),
            AudioSource::Bytes(vec![1, 2, 3])
        );
        assert_eq!(
            AudioSource::path("a/b.wav"),
            AudioSource::Path(PathBuf::from("a/b.wav"))
        );
        assert_eq!(
            AudioSource::url("https://x/y.mp3"),
            AudioSource::Url("https://x/y.mp3".to_string())
        );
    }

    #[test]
    fn error_display_is_distinct() {
        let backend = AudioError::Backend("boom".into()).to_string();
        let decode = AudioError::Decode("bad".into()).to_string();
        let unsupported = AudioError::NotSupported.to_string();
        assert!(backend.contains("boom"));
        assert!(decode.contains("bad"));
        assert!(unsupported.contains("not supported"));
        // Each variant renders distinctly.
        assert_ne!(backend, decode);
        assert_ne!(decode, unsupported);
    }

    #[test]
    fn error_is_std_error() {
        fn assert_error<E: std::error::Error>(_: &E) {}
        assert_error(&AudioError::NotSupported);
    }

    /// On the hosts that run these tests (macOS / Linux / Windows), `load`
    /// is reachable and returns a typed result — exercising the public
    /// async surface end-to-end without a real audio device. On macOS this
    /// hits the real `AVAudioPlayer` decode path; on Linux/Windows it's the
    /// honest `NotSupported` fallback. Either way `load` must not panic.
    #[tokio::test]
    async fn load_bad_bytes_is_typed_error_not_panic() {
        // Not valid encoded audio — every backend should reject it as a
        // typed error (Decode/Backend) or report NotSupported, never panic.
        let result = load(AudioSource::bytes(vec![0u8; 8])).await;
        assert!(
            result.is_err(),
            "garbage bytes must not decode into a playable Sound"
        );
    }
}
