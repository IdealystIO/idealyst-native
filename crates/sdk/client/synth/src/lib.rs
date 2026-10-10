//! Procedural audio **synthesis** that plugs into the audio/media pipeline.
//!
//! Build sound effects out of nothing — tones, pitch sweeps, noise bursts,
//! ADSR envelopes, filters, bit-crushing — then hand them to the rest of the
//! framework: play them through [`audio`], or feed them into a live
//! [`media_stream::AudioStream`] that `media-writer` records and the web
//! bridge turns into a real `MediaStreamTrack`.
//!
//! ```ignore
//! use synth::{sweep, Envelope, Sweep, Waveform, DEFAULT_FORMAT};
//!
//! # async fn demo() -> Result<(), audio::AudioError> {
//! // A classic "pew": a square wave diving from 1.2 kHz to 200 Hz.
//! let laser = sweep(Waveform::Square, 1200.0, 200.0, 0.25, Sweep::Exponential, DEFAULT_FORMAT)
//!     .apply_envelope(Envelope::new(0.005, 0.05, 0.4, 0.1))
//!     .gain(0.4);
//!
//! // Encode to WAV and load it as an `audio::Sound` — replay it as often
//! // as you like.
//! let sound = laser.load().await?;
//! let _playback = sound.play();
//! # Ok(())
//! # }
//! ```
//!
//! # Ahead of time, not per sample
//!
//! Every generator renders a whole sound up front into an owned [`Pcm`]
//! buffer: interleaved `f32` samples in `[-1, 1]` plus an
//! [`AudioFormat`]. The functions are pure and deterministic (noise takes an
//! explicit seed), so a sound is the same on every platform and every run,
//! and it can be unit-tested sample by sample. There is no real-time
//! per-sample callback engine — a game's SFX are short and known in
//! advance, and rendering them once is cheaper and simpler than running a
//! DSP graph on an audio thread.
//!
//! # Two ways out into the pipeline
//!
//! - **Playback** — [`Pcm::to_wav`] encodes 16-bit PCM WAV (decodable by
//!   every platform player `audio` drives); [`Pcm::to_source`] /
//!   [`Pcm::load`] wrap that in an [`audio::AudioSource`] / load an
//!   [`audio::Sound`].
//! - **Streams** — [`Pcm::write_to`] pushes a buffer into an
//!   [`AudioWriter`](media_stream::AudioWriter) in one go (for offline
//!   consumers), and the [`Mixer`] is a *live*, real-time-paced
//!   [`AudioStream`](media_stream::AudioStream) producer: it emits continuous
//!   audio (silence when idle) and mixes each [`Mixer::play`]ed buffer in at
//!   the playhead. Record a game's audio with `media-writer`, or route it into
//!   a `MediaStreamTrack` on web.
//!
//! Captured audio comes back the other way: [`Pcm::from_frame`] /
//! [`Pcm::append_frame`] turn a microphone's
//! [`AudioFrame`](media_stream::AudioFrame)s into a buffer you can mix, trim,
//! encode and play.
//!
//! # Permissions
//!
//! None. Synthesis is pure computation; playback and recording permissions
//! (if any) belong to `audio` / `media-writer`.

#![deny(missing_docs)]

mod dsp;
mod envelope;
mod error;
mod generate;
mod mixer;
mod pcm;
pub mod presets;
mod wav;

// Compile-checked usage recipes (catalog feature only).
#[doc(hidden)]
pub mod recipes;

pub use envelope::Envelope;
pub use error::SynthError;
pub use generate::{noise, silence, sweep, tone, Noise, Sweep, Waveform};
pub use media_stream::AudioFormat;
pub use mixer::{Mixer, Voice, MAX_PUMP_MICROS, PUMP_LEAD_MICROS, PUMP_TICK_MS};
pub use pcm::Pcm;

/// The format the [`presets`] render at, and a sensible default for game
/// SFX: 44.1 kHz mono. Mono keeps buffers small; every consumer (the
/// [`Mixer`], the platform players) up-mixes as needed.
pub const DEFAULT_FORMAT: AudioFormat = AudioFormat {
    sample_rate: 44_100,
    channels: 1,
};
