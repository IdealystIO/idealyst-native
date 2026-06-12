//! Neural noise suppression as an [`AudioStream`] transformer.
//!
//! Hand [`Denoiser::process`] an [`AudioStream`] and get a new `AudioStream`
//! back whose chunks have been run through **DeepFilterNet 3** (real-time
//! speech enhancement). It drops into the media stack anywhere a stream flows:
//!
//! ```no_run
//! use denoise::{Denoiser, AudioStream};
//! # fn noisy_input() -> AudioStream { AudioStream::new().0 }
//! # async fn demo() -> Result<(), denoise::DenoiseError> {
//! // `noisy` is any AudioStream — e.g. `microphone::open_stream(..)`, camera
//! // audio, a decoded file. Microphone -> denoise -> (recorder, playback, …).
//! let noisy = noisy_input();
//! let clean = Denoiser::new().process(&noisy).await?;
//! // `clean` is a 48 kHz mono AudioStream; subscribe / record / play it.
//! # let _ = clean;
//! # Ok(())
//! # }
//! ```
//!
//! On web, the model isn't bundled into the wasm binary — fetch the
//! `DeepFilterNet3_onnx.tar.gz` and use `Denoiser::with_weights(bytes)` instead
//! of `new()`. See [`Denoiser`].
//!
//! # What it is (and isn't)
//!
//! This is noise *suppression* — single-stream speech enhancement that removes
//! background noise from one signal. It is **not** acoustic echo cancellation
//! (which needs a reference/far-end signal); for that, a different SDK is the
//! right tool.
//!
//! # Output format
//!
//! Output is **always 48 kHz mono** — DeepFilterNet's native format. Input of
//! any rate/channel-count is accepted: it's downmixed to mono and resampled to
//! 48 kHz before processing. The output stream carries its own 48 kHz monotonic
//! clock (`pts_micros` from the [`AudioWriter`]), not the input's timeline — fine
//! for standalone denoised capture; note it if you're muxing against the
//! *original* audio/video for lip-sync.
//!
//! # Latency
//!
//! Algorithmic latency is roughly one hop of buffering (~10 ms) plus
//! DeepFilterNet's lookahead (~20 ms) ≈ **~30 ms** — fine for recording and
//! one-way streaming; marginal for full-duplex live monitoring.
//!
//! # Platforms
//!
//! The denoising is pure-Rust computation (inference via `tract`), so the same
//! code runs on **macOS / iOS / Android / desktop** with no per-OS native code.
//! Only the execution context differs (see the `imp` modules):
//!
//! - **native** — a background processing thread, fed by the input
//!   subscription over a channel, so inference never blocks the audio thread.
//! - **web (wasm32)** — the same engine, run inline on the main thread inside
//!   the subscribe callback (wasm has no threads). One DeepFilterNet inference
//!   per ~10 ms frame runs on the main thread; if profiling shows jank, the
//!   path forward is an AudioWorklet hosting the wasm engine.

#![deny(missing_docs)]

mod engine;

use engine::{Config, Weights};

pub use media_stream::AudioStream;

// Exactly one backend `imp` compiles per target. Each exposes
// `async start(input, writer, cfg, weights) -> Result<Handle, DenoiseError>`,
// where the returned `Handle`'s `Drop` tears down processing. The model is
// built inside `start` (on the worker thread on native, inline on web) and
// awaited so init errors surface to the caller.
#[cfg(target_arch = "wasm32")]
#[path = "web.rs"]
mod imp;

#[cfg(not(target_arch = "wasm32"))]
#[path = "native.rs"]
mod imp;

/// Why a [`Denoiser::process`] call failed. Both variants are setup-time
/// faults; once `process` returns `Ok`, streaming itself does not error (a
/// transient per-frame model fault emits silence for that block rather than
/// surfacing here).
#[derive(Debug, thiserror::Error)]
pub enum DenoiseError {
    /// The DeepFilterNet model failed to load or build its tract plan.
    #[error("failed to initialize DeepFilterNet model: {0}")]
    ModelInit(String),
    /// The background processing thread could not be spawned (native only).
    #[error("failed to spawn denoise worker: {0}")]
    Spawn(String),
}

/// A reusable, cheap handle that turns noisy [`AudioStream`]s into clean ones.
///
/// Configure it with the builder methods, then call [`process`](Self::process)
/// per input stream. One `Denoiser` can process many streams; each
/// [`process`](Self::process) call builds its own independent model instance
/// and processing pipeline.
///
/// # Where the model comes from
///
/// - On **native**, [`Denoiser::new`] uses a model embedded in the binary — no
///   setup, no files.
/// - On **web**, the model is deliberately *not* embedded (it would bloat the
///   wasm bundle), so [`new`](Self::new) is unavailable; fetch the
///   `DeepFilterNet3_onnx.tar.gz` model yourself and pass its bytes to
///   [`with_weights`](Self::with_weights). That method also works on native for
///   a custom or externally-managed model.
#[derive(Clone, Copy)]
pub struct Denoiser {
    weights: Weights,
    atten_lim_db: Option<f32>,
    post_filter: bool,
}

impl Denoiser {
    /// A denoiser using the **embedded** DeepFilterNet 3 model, with sensible
    /// defaults (full suppression, post-filter off). Native only — on web,
    /// use [`with_weights`](Self::with_weights) (the model isn't bundled into
    /// the wasm binary).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new() -> Self {
        Denoiser {
            weights: Weights::Embedded,
            atten_lim_db: None,
            post_filter: false,
        }
    }

    /// A denoiser using a caller-supplied model. `weights` is the DeepFilterNet
    /// model `.tar.gz` (e.g. `DeepFilterNet3_onnx.tar.gz`) — on web, fetch it
    /// separately (keeping it out of the wasm bundle) and pass the bytes here.
    ///
    /// The slice must be `'static` because upstream's `DfParams::from_bytes`
    /// requires it. A denoising model is loaded once and lives for the app's
    /// lifetime, so this is natural: stash the fetched bytes in a `static`
    /// (`OnceLock<Vec<u8>>` → `.as_slice()`) or `Box::leak` a one-time buffer.
    /// The SDK copies the bytes into the model and never leaks them itself.
    pub fn with_weights(weights: &'static [u8]) -> Self {
        Denoiser {
            weights: Weights::Bytes(weights),
            atten_lim_db: None,
            post_filter: false,
        }
    }

    /// Cap how much the background may be attenuated, in dB. Leaving some
    /// residual noise in (e.g. `12.0`) sounds more natural than total removal.
    /// Omit (the default) for unlimited suppression.
    pub fn attenuation_limit_db(mut self, db: f32) -> Self {
        self.atten_lim_db = Some(db);
        self
    }

    /// Enable DeepFilterNet's speech post-filter, which sharpens voice at the
    /// cost of slightly more attenuation. Off by default.
    pub fn post_filter(mut self, on: bool) -> Self {
        self.post_filter = on;
        self
    }

    /// Start denoising `input`, returning a new **48 kHz mono** [`AudioStream`]
    /// of the enhanced signal.
    ///
    /// `async` because building the model (the expensive step) happens here:
    /// on native it runs on the processing thread and this awaits its
    /// completion without blocking the caller; on web it builds inline. Once it
    /// resolves `Ok`, the per-chunk work runs off the audio thread (native) /
    /// inline (web). Processing continues for as long as the returned stream
    /// (or a clone) is held and the input keeps producing; dropping the last
    /// clone tears down the pipeline. Fails with [`DenoiseError`] if the model
    /// can't load/build.
    pub async fn process(&self, input: &AudioStream) -> Result<AudioStream, DenoiseError> {
        let cfg = Config { atten_lim_db: self.atten_lim_db, post_filter: self.post_filter };
        let (out, writer) = AudioStream::new();
        let handle = imp::start(input, writer, cfg, self.weights).await?;
        // Keep the pipeline alive exactly as long as the output stream: the
        // last drop runs this stopper, tearing down the worker/subscription.
        out.attach_stopper(move || drop(handle));
        Ok(out)
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use media_stream::{AudioFrame, AudioFormat};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    // End-to-end through the real native pipeline (subscription -> channel ->
    // worker thread -> DfTract -> output stream): push noisy 16 kHz stereo into
    // an input stream, run `process`, and assert enhanced 48 kHz MONO chunks
    // come out the other side. Exercises downmix + resample + framing + the
    // thread driver, not just the engine in isolation.
    #[test]
    fn process_end_to_end_emits_48k_mono() {
        let (input, in_writer) = AudioStream::new();
        let out = pollster::block_on(Denoiser::new().process(&input))
            .expect("model init + worker spawn");

        let formats = Arc::new(Mutex::new(Vec::<AudioFormat>::new()));
        let total = Arc::new(Mutex::new(0usize));
        let _sub = {
            let formats = formats.clone();
            let total = total.clone();
            out.subscribe(move |f: &AudioFrame| {
                formats.lock().unwrap().push(f.format());
                *total.lock().unwrap() += f.frame_count();
            })
        };

        // ~0.5 s of 16 kHz stereo white-ish noise, delivered in 10 ms chunks.
        let mut seed = 0x1234_5678u32;
        let mut rng = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 9) as f32 / (1u32 << 23) as f32 * 2.0 - 1.0
        };
        for _ in 0..50 {
            let chunk: Vec<f32> = (0..160 * 2).map(|_| rng() * 0.3).collect();
            in_writer.write_pcm_f32(16_000, 2, &chunk);
        }

        // The worker processes asynchronously; wait until output arrives.
        let deadline = Instant::now() + Duration::from_secs(20);
        while *total.lock().unwrap() == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }

        let formats = formats.lock().unwrap();
        assert!(!formats.is_empty(), "denoised chunks were emitted");
        for fmt in formats.iter() {
            assert_eq!(
                *fmt,
                AudioFormat { sample_rate: 48_000, channels: 1 },
                "output is canonical 48 kHz mono",
            );
        }
    }

    // Silence in -> (near) silence out: DeepFilterNet must not invent energy
    // from a zero signal. Also a deterministic smoke test of the full path.
    #[test]
    fn process_silence_stays_quiet() {
        let (input, in_writer) = AudioStream::new();
        let out = pollster::block_on(Denoiser::new().process(&input)).expect("init");

        let peak = Arc::new(Mutex::new(0.0f32));
        let _sub = {
            let peak = peak.clone();
            out.subscribe(move |f: &AudioFrame| {
                let local = f.samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
                let mut p = peak.lock().unwrap();
                if local > *p {
                    *p = local;
                }
            })
        };

        let silence = vec![0.0f32; 480]; // 10 ms @ 48 kHz mono
        for _ in 0..50 {
            in_writer.write_pcm_f32(48_000, 1, &silence);
        }

        let deadline = Instant::now() + Duration::from_secs(20);
        // Give the worker time to drain.
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            if *peak.lock().unwrap() > 0.0 {
                break;
            }
        }
        assert!(*peak.lock().unwrap() < 1e-3, "silence in stays quiet out");
    }
}
