//! Low-latency, mixable, spatial-ready sound for games.
//!
//! ```ignore
//! use audio_engine::{AudioEngine, SoundBuffer, VoiceParams};
//!
//! let engine = AudioEngine::start().await?;
//! let shot = SoundBuffer::decode(include_bytes!("shot.ogg"))?;
//!
//! // Fire-and-forget: overlapping shots layer, each with its own pan/filter.
//! engine.play(&shot, VoiceParams::new().gain(0.8).pan(-0.4));
//!
//! // A loop you steer while it plays.
//! let hum = engine.play(&engine_hum, VoiceParams::new().looping(true).lowpass(800.0));
//! hum.set_lowpass(Some(4_000.0), 0.25);   // open the filter over 250 ms
//! hum.set_rate(1.3, 0.25);                // and pitch up
//! hum.stop(0.5);                          // fade out, then free
//! ```
//!
//! # What it is
//!
//! An [`AudioEngine`] owns the default audio output and mixes up to
//! [`MAX_VOICES`] [`Voice`]s into it. A voice plays a [`SoundBuffer`]
//! (decoded once, shared by every voice that plays it) or a live
//! [`media_stream::AudioStream`] (a `synth::Mixer`, a microphone), with its
//! own gain, stereo pan, low-pass filter and playback rate — each changeable
//! while it plays, with a linear ramp. [`spatial`] turns a listener and a
//! source position into those three numbers.
//!
//! # One behavior on every platform
//!
//! On the web the browser's Web Audio graph does the mixing; everywhere else
//! a deterministic Rust mixer does, writing into the platform output (cpal on
//! macOS / iOS / Linux / Windows, AAudio on Android). Every formula — the
//! `BiquadFilterNode` low-pass, the `StereoPannerNode` equal-power law, the
//! `AudioBufferSourceNode` linear-interpolated rate, `linearRampToValueAtTime`,
//! the destination clamp — is the Web Audio spec's, so the two converge on
//! the same output. [`AudioEngine::offline`] renders the Rust mixer with no
//! device, for tests.
//!
//! # Versus `audio`
//!
//! The `audio` crate plays one file per platform player with a volume knob —
//! simple, and enough for UI sounds and music. This crate is for when you
//! need many overlapping sounds (Android's `audio` restarts instead of
//! layering), per-sound pan / filter / pitch, low latency, or a live stream.

#![deny(missing_docs)]

mod backend;
mod buffer;
#[doc(hidden)]
pub mod dsp;
mod mixer;
mod offline;
mod params;
pub mod spatial;

// Compile-checked usage recipes (catalog feature only).
#[doc(hidden)]
pub mod recipes;

use std::rc::{Rc, Weak};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use media_stream::AudioStream;

pub use buffer::SoundBuffer;
pub use mixer::{MAX_VOICES, QUANTUM, STREAM_BACKLOG_SECS};
pub use offline::OfflineRenderer;
pub use params::{VoiceParams, MAX_RATE};

use backend::EngineBackend;
use mixer::{VoiceOp, VoiceShared};
use params::{sanitize_gain, sanitize_hz, sanitize_pan, sanitize_rate, sanitize_secs};

/// The iOS `AVAudioSession` category the engine puts the app in when it
/// starts — **Ambient**: the game's sound mixes with whatever else is
/// playing (the player's own music keeps going) and obeys the ring/silent
/// switch, which is what players expect from game audio. (`Playback` would
/// stop other apps' audio and ignore the switch — right for a music player,
/// wrong for SFX.) Applied only while the session is still in the system
/// default category, so an app that chose its own (e.g. `microphone`'s
/// PlayAndRecord) keeps it.
pub const IOS_SESSION_CATEGORY: &str = "AVAudioSessionCategoryAmbient";

/// Why the engine couldn't do something.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum EngineError {
    /// No audio output device is available.
    #[error("no audio output device")]
    NoOutputDevice,
    /// No engine backend on this platform or OS version (Android before
    /// 8.1 / API 27 — AAudio is skipped on 8.0, as Oboe does).
    #[error("audio engine not supported on this platform")]
    Unsupported,
    /// The bytes couldn't be decoded as WAV / MP3 / Ogg Vorbis / FLAC.
    #[error("decode failed: {0}")]
    Decode(String),
    /// The platform audio stack failed.
    #[error("audio backend error: {0}")]
    Backend(String),
}

/// The audio output and its mixer. A cheap `Clone` handle (not `Send` —
/// use it from the UI thread); the output closes when the last clone drops.
/// [`Voice`]s don't keep it open: once it closes they go quiet and their
/// setters do nothing.
#[derive(Clone)]
pub struct AudioEngine {
    backend: Rc<dyn EngineBackend>,
}

impl AudioEngine {
    /// Open the default output.
    ///
    /// On the web, browsers start audio suspended until a user gesture: call
    /// [`resume`](Self::resume) from a click/tap handler.
    pub async fn start() -> Result<AudioEngine, EngineError> {
        Ok(AudioEngine {
            backend: backend::start().await?,
        })
    }

    /// An engine with no device: its mixer is rendered on demand through the
    /// returned [`OfflineRenderer`] (`channels`-wide interleaved output at
    /// `sample_rate`). Deterministic — the same calls always render the same
    /// samples — and identical to what a device-backed engine plays. For
    /// tests, and for rendering sound to a file.
    pub fn offline(sample_rate: u32, channels: u16) -> (AudioEngine, OfflineRenderer) {
        let (backend, renderer) = offline::new(sample_rate, channels);
        (AudioEngine { backend }, renderer)
    }

    /// The output sample rate in Hz.
    pub fn sample_rate(&self) -> u32 {
        self.backend.sample_rate()
    }

    /// Best estimate of the delay from [`play`](Self::play) to sound at the
    /// speaker (device buffer + device-reported output delay), when the
    /// platform reports one.
    pub fn output_latency(&self) -> Option<Duration> {
        self.backend.output_latency()
    }

    /// Start playing `buf`. The voice starts at the next 128-frame render
    /// quantum (≤ 2.7 ms at 48 kHz) after the output picks the command up.
    /// Dropping the returned [`Voice`] does NOT stop it — one-shots are
    /// fire-and-forget; stop loops with [`Voice::stop`].
    ///
    /// When [`MAX_VOICES`] are already sounding, the oldest non-looping
    /// buffer voice is cut to make room (looping and stream voices are never
    /// stolen; if nothing can be stolen the new voice doesn't start).
    pub fn play(&self, buf: &SoundBuffer, params: VoiceParams) -> Voice {
        let (id, shared) = self.backend.play(buf, params.sanitized());
        self.voice(id, shared)
    }

    /// Play a live stream: its PCM is converted to the output rate and
    /// layout (linear resampling) as it arrives, at most
    /// [`STREAM_BACKLOG_SECS`] is buffered (older audio is dropped), and
    /// silence plays while the stream is starved. Gain, pan and low-pass
    /// apply as for a buffer; `rate`, `looping` and `start_offset_secs` are
    /// ignored (a live stream can't be sped up or rewound). A stream voice
    /// plays until [`Voice::stop`].
    pub fn play_stream(&self, stream: &AudioStream, params: VoiceParams) -> Voice {
        let (id, shared) = self.backend.play_stream(stream, params.sanitized());
        self.voice(id, shared)
    }

    /// Ramp the master gain (applied to the mix before the final clamp).
    pub fn set_master_gain(&self, gain: f32, ramp_secs: f32) {
        self.backend
            .set_master_gain(sanitize_gain(gain), sanitize_secs(ramp_secs));
    }

    /// Resume output. On the web this is `AudioContext.resume()` and must be
    /// called from a user gesture (or after
    /// `runtime_core::user_activated()`) the first time; on native it
    /// restarts a [`suspend`](Self::suspend)ed stream.
    pub fn resume(&self) {
        self.backend.resume();
    }

    /// Pause output (e.g. when the app goes to the background). Voices hold
    /// their position; [`resume`](Self::resume) continues them.
    pub fn suspend(&self) {
        self.backend.suspend();
    }

    /// Voices currently sounding (including ones just started).
    pub fn active_voices(&self) -> usize {
        self.backend.active_voices()
    }

    fn voice(&self, id: u64, shared: Arc<VoiceShared>) -> Voice {
        Voice {
            id,
            shared,
            engine: Rc::downgrade(&self.backend),
        }
    }
}

/// A playing sound. A cheap `Clone` handle; dropping it does **not** stop
/// the sound. All setters ramp linearly over `ramp_secs` (`0` = now) from
/// wherever the value currently is; calls on a finished voice do nothing.
#[derive(Clone)]
pub struct Voice {
    id: u64,
    shared: Arc<VoiceShared>,
    engine: Weak<dyn EngineBackend>,
}

impl Voice {
    fn op(&self, op: VoiceOp) {
        if let Some(engine) = self.engine.upgrade() {
            engine.voice(self.id, op);
        }
    }

    /// Fade the gain to 0 over `fade_secs`, then free the voice (`0` = cut
    /// now). Later [`set_gain`](Self::set_gain) calls are ignored — the fade
    /// owns the gain.
    pub fn stop(&self, fade_secs: f32) {
        self.op(VoiceOp::Stop {
            fade_secs: sanitize_secs(fade_secs),
        });
    }

    /// Ramp the gain (linear; `1` = unchanged).
    pub fn set_gain(&self, gain: f32, ramp_secs: f32) {
        self.op(VoiceOp::Gain {
            gain: sanitize_gain(gain),
            ramp_secs: sanitize_secs(ramp_secs),
        });
    }

    /// Ramp the stereo position (`-1` left … `1` right).
    pub fn set_pan(&self, pan: f32, ramp_secs: f32) {
        self.op(VoiceOp::Pan {
            pan: sanitize_pan(pan),
            ramp_secs: sanitize_secs(ramp_secs),
        });
    }

    /// Ramp the playback rate (= pitch; `2` is an octave up). Clamped to
    /// `[0, MAX_RATE]`. No effect on stream voices.
    pub fn set_rate(&self, rate: f32, ramp_secs: f32) {
        self.op(VoiceOp::Rate {
            rate: sanitize_rate(rate),
            ramp_secs: sanitize_secs(ramp_secs),
        });
    }

    /// Ramp the low-pass cutoff. `Some(hz)` on an unfiltered voice fades the
    /// filter in from the top of the spectrum; `None` ramps the cutoff up to
    /// Nyquist (where the filter is transparent) and then removes it — no
    /// click either way.
    pub fn set_lowpass(&self, hz: Option<f32>, ramp_secs: f32) {
        self.op(VoiceOp::Lowpass {
            hz: hz.map(sanitize_hz),
            ramp_secs: sanitize_secs(ramp_secs),
        });
    }

    /// Turn looping on or off. Off lets the current pass play to the end.
    pub fn set_looping(&self, looping: bool) {
        self.op(VoiceOp::Looping(looping));
    }

    /// Gain + pan + low-pass for a source at `source` (see
    /// [`spatial::params_for`]), all ramped over `ramp_secs` — call it each
    /// frame for a moving sound with a ramp about one frame long.
    pub fn set_spatial(
        &self,
        listener: &spatial::Listener,
        source: [f32; 3],
        attenuation: &spatial::Attenuation,
        ramp_secs: f32,
    ) {
        let (gain, pan, lowpass) = spatial::params_for(listener, source, attenuation);
        self.set_gain(gain, ramp_secs);
        self.set_pan(pan, ramp_secs);
        self.set_lowpass(lowpass, ramp_secs);
    }

    /// `true` until the voice ends (played out, stopped, stolen, or the
    /// engine closed).
    pub fn is_playing(&self) -> bool {
        self.shared.playing.load(Ordering::Acquire)
    }
}
