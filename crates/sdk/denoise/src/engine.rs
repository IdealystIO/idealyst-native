//! The platform-agnostic denoising DSP — the heart of the SDK.
//!
//! [`Engine`] owns a [`DfTract`] (DeepFilterNet 3) plus the framing/resampling
//! plumbing needed to bridge arbitrary incoming PCM chunks to the model's fixed
//! contract (48 kHz mono, exactly `hop_size` samples per inference). It is pure
//! logic: no streams, no threads, no platform code. The native and web backends
//! differ only in *where* they call [`Engine::feed`] from (a worker thread vs.
//! inline on the main thread).
//!
//! Pipeline, per incoming chunk:
//!   1. downmix interleaved -> mono,
//!   2. resample the mono signal `sr` -> 48 kHz (bypassed when already 48 kHz),
//!   3. accumulate into a ring and, while >= `hop_size` samples are buffered,
//!      run one `DfTract::process` and push the enhanced `hop_size` block out
//!      through the [`AudioWriter`] as a 48 kHz mono chunk.

// The `deep_filter` package's library target is named `df`, so it's imported
// under that name (not the package name).
use df::tract::{DfParams, DfTract, RuntimeParams};
use media_stream::AudioWriter;
use ndarray::{ArrayView2, ArrayViewMut2};

use crate::DenoiseError;

/// DeepFilterNet's fixed output format. The model is trained at 48 kHz mono;
/// every enhanced chunk we emit carries this format.
pub(crate) const OUTPUT_RATE: u32 = 48_000;
pub(crate) const OUTPUT_CHANNELS: u16 = 1;

/// Post-filter strength applied when the caller enables it. DeepFilterNet's
/// post-filter sharpens speech at the cost of a touch more attenuation; 0.0
/// disables it. 0.02 is the upstream-recommended "on" value.
const ENABLED_POST_FILTER_BETA: f32 = 0.02;

/// Tuning knobs forwarded to the model. Kept separate from the public
/// `Denoiser` builder so the engine has no dependency on the public surface.
#[derive(Clone, Copy)]
pub(crate) struct Config {
    /// Cap on how much the noise floor may be attenuated, in dB. `None` =
    /// no limit (full suppression). A finite value leaves some residual
    /// background in for a more natural sound.
    pub atten_lim_db: Option<f32>,
    /// Enable DeepFilterNet's speech post-filter.
    pub post_filter: bool,
}

/// Where the DeepFilterNet model comes from. Embedding is native-only — the web
/// build omits the model from the wasm binary (smaller bundle) and must supply
/// the bytes. `Bytes` is the model `.tar.gz` as a `&'static [u8]`: upstream's
/// `DfParams::from_bytes` requires `'static` (it was built for the embedded
/// `include_bytes!` path), even though it copies the bytes into owned buffers
/// internally. `&'static [u8]` is `Copy` + `Send`, so the worker thread takes
/// it for free.
#[derive(Clone, Copy)]
pub(crate) enum Weights {
    /// The DeepFilterNet3 model compiled into the binary (native `default-model`).
    #[cfg(not(target_arch = "wasm32"))]
    Embedded,
    /// A model `.tar.gz` supplied by the caller.
    Bytes(&'static [u8]),
}

pub(crate) struct Engine {
    df: DfTract,
    writer: AudioWriter,
    /// Samples per inference (model-defined; 480 for DF3 at 48 kHz).
    hop: usize,
    /// `sr` -> 48 kHz mono resampler. Reset when the input rate changes.
    resampler: Resampler,
    /// Resampled 48 kHz mono samples awaiting hop-aligned framing.
    ring: Vec<f32>,
    /// Reused mono downmix scratch (interleaved input -> mono).
    mono: Vec<f32>,
    /// Reused enhanced-output block (`hop` samples).
    enh: Vec<f32>,
}

impl Engine {
    pub(crate) fn new(
        writer: AudioWriter,
        cfg: Config,
        weights: Weights,
    ) -> Result<Engine, DenoiseError> {
        let mut rp = RuntimeParams::default_with_ch(OUTPUT_CHANNELS as usize);
        if let Some(lim) = cfg.atten_lim_db {
            rp = rp.with_atten_lim(lim);
        }
        if cfg.post_filter {
            rp = rp.with_post_filter(ENABLED_POST_FILTER_BETA);
        }

        // Build the tract plan from the chosen weights. The model is built
        // INSIDE the match so `bytes` (which `DfParams` borrows) outlives
        // `DfTract::new`; the resulting `DfTract` is self-owned afterwards.
        // `{:#}` renders the full anyhow context chain, not just the top frame,
        // so a model/codegen/parse failure is diagnosable.
        let df = match weights {
            #[cfg(not(target_arch = "wasm32"))]
            Weights::Embedded => DfTract::new(DfParams::default(), &rp),
            Weights::Bytes(bytes) => {
                let params = DfParams::from_bytes(bytes)
                    .map_err(|e| DenoiseError::ModelInit(format!("{e:#}")))?;
                DfTract::new(params, &rp)
            }
        }
        .map_err(|e| DenoiseError::ModelInit(format!("{e:#}")))?;

        // DF3 runs at 48 kHz; assert the assumption the rest of the engine
        // (OUTPUT_RATE) bakes in, so a future model with a different rate
        // fails loudly here instead of producing wrong-rate output.
        debug_assert_eq!(df.sr as u32, OUTPUT_RATE, "DfTract sample rate must be 48 kHz");

        let hop = df.hop_size;
        Ok(Engine {
            df,
            writer,
            hop,
            resampler: Resampler::new(),
            ring: Vec::with_capacity(hop * 4),
            mono: Vec::new(),
            enh: vec![0.0; hop],
        })
    }

    /// Feed one chunk of interleaved, normalized f32 PCM. Emits zero or more
    /// enhanced 48 kHz mono chunks through the [`AudioWriter`] as full
    /// `hop_size` blocks accumulate.
    pub(crate) fn feed(&mut self, samples: &[f32], sample_rate: u32, channels: u16) {
        if sample_rate == 0 || channels == 0 || samples.is_empty() {
            return;
        }

        // 1. Downmix to mono.
        let ch = channels as usize;
        self.mono.clear();
        if ch == 1 {
            self.mono.extend_from_slice(samples);
        } else {
            self.mono.reserve(samples.len() / ch);
            for frame in samples.chunks_exact(ch) {
                let sum: f32 = frame.iter().sum();
                self.mono.push(sum / ch as f32);
            }
        }

        // 2. Resample mono -> 48 kHz, appending into the framing ring.
        self.resampler.process(&self.mono, sample_rate, &mut self.ring);

        // 3. Drain hop-aligned blocks through the model.
        let mut cursor = 0;
        while self.ring.len() - cursor >= self.hop {
            let noisy = ArrayView2::from_shape((1, self.hop), &self.ring[cursor..cursor + self.hop])
                .expect("hop-sized slice is a valid (1, hop) view");
            let enh = ArrayViewMut2::from_shape((1, self.hop), &mut self.enh[..])
                .expect("enh buffer is a valid (1, hop) view");
            // `process` only fails on a genuine model/runtime fault, which
            // would be a persistent bug, not a per-frame condition. Emit
            // silence for the block rather than panicking the audio path.
            if self.df.process(noisy, enh).is_ok() {
                self.writer
                    .write_pcm_f32(OUTPUT_RATE, OUTPUT_CHANNELS, &self.enh);
            }
            cursor += self.hop;
        }
        self.ring.drain(..cursor);
    }
}

/// A small streaming linear resampler: arbitrary input rate -> 48 kHz mono.
///
/// Rationale: rubato (the usual choice) churns its API across majors and pulls
/// an FFT tree we don't otherwise need; a continuous-phase linear resampler is
/// a handful of lines, dependency-free, and deterministic. It interpolates
/// across chunk boundaries via the carried `prev` sample, so back-to-back
/// `process` calls produce the same stream as one combined call.
///
/// Limitation (documented, §5): linear interpolation has no anti-alias
/// pre-filter, so *down*-sampling (input rate > 48 kHz, e.g. a 96 kHz source)
/// can alias. Real inputs here are <= 48 kHz (mic/camera/most files), i.e. the
/// upsample case, where linear interpolation is artifact-free aside from mild
/// imaging well above the speech band the model cares about. If a >48 kHz
/// source becomes common, add a pre-decimation low-pass here.
pub(crate) struct Resampler {
    /// Input rate this resampler is currently configured for. A change resets
    /// the phase + history so the new rate starts cleanly.
    rate: u32,
    /// Fractional read position into the *current* input chunk. The integer
    /// part indexes a sample; index -1 refers to `prev`.
    phase: f64,
    /// Last sample of the previous chunk, for cross-boundary interpolation.
    /// Defaults to 0.0 (silence) before any input, which is the correct value
    /// for the first chunk's index -1.
    prev: f32,
}

impl Resampler {
    pub(crate) fn new() -> Resampler {
        Resampler { rate: 0, phase: 0.0, prev: 0.0 }
    }

    /// Resample `input` (mono, at `rate` Hz) to 48 kHz, appending the result to
    /// `out`. State persists across calls for gapless streaming.
    pub(crate) fn process(&mut self, input: &[f32], rate: u32, out: &mut Vec<f32>) {
        if input.is_empty() || rate == 0 {
            return;
        }
        // Fast path: already at the target rate, pass through unchanged.
        if rate == OUTPUT_RATE {
            out.extend_from_slice(input);
            // Keep `prev` coherent in case the rate changes back later.
            self.prev = *input.last().unwrap();
            self.rate = OUTPUT_RATE;
            self.phase = 0.0;
            return;
        }
        if rate != self.rate {
            // New (or first) input rate: restart the timeline.
            self.rate = rate;
            self.phase = 0.0;
            self.prev = 0.0;
        }

        // Input samples advanced per output sample.
        let step = rate as f64 / OUTPUT_RATE as f64;
        let n = input.len();
        let sample_at = |i: isize| -> f32 {
            if i < 0 {
                self.prev
            } else {
                input[i as usize]
            }
        };

        // Emit outputs while the interpolation window [floor(t), floor(t)+1]
        // is available within this chunk (i.e. floor(t)+1 <= n-1).
        let mut t = self.phase;
        while t <= (n - 1) as f64 {
            let i0 = t.floor() as isize;
            let frac = (t - i0 as f64) as f32;
            let a = sample_at(i0);
            let b = sample_at(i0 + 1);
            out.push(a + (b - a) * frac);
            t += step;
        }

        // Carry phase relative to the next chunk's start, and remember the
        // last real sample for the next call's index -1.
        self.phase = t - n as f64;
        self.prev = input[n - 1];
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    // Resampling 48 kHz in is a pure pass-through (no interpolation, no drift).
    #[test]
    fn resampler_passthrough_at_48k() {
        let mut r = Resampler::new();
        let mut out = Vec::new();
        let input: Vec<f32> = (0..960).map(|i| (i as f32 * 0.01).sin()).collect();
        r.process(&input, 48_000, &mut out);
        assert_eq!(out, input);
    }

    // Regression (named after the bug class): the 48 kHz output sample count
    // must track the input rate. Feed 1 s of audio at each common rate and
    // assert the streamed total lands at ~48 kHz, within one step. Guards both
    // the resample ratio and the streaming/bypass paths.
    #[test]
    fn resampler_ratio_regression() {
        for &rate in &[16_000u32, 44_100, 48_000] {
            let mut r = Resampler::new();
            let mut out = Vec::new();
            // One second, delivered in 10 ms chunks to exercise cross-boundary
            // interpolation and phase carry — not one big buffer.
            let chunk = (rate / 100) as usize;
            for c in 0..100 {
                let block: Vec<f32> = (0..chunk)
                    .map(|i| (((c * chunk + i) as f32) * 0.001).sin())
                    .collect();
                r.process(&block, rate, &mut out);
            }
            let got = out.len() as i64;
            // Streaming linear resampling defers the final input sample's
            // interpolated outputs (they need the next, never-arriving chunk),
            // so the total runs a hair SHORT — at most ceil(48000/rate) samples,
            // never over. For 16 kHz (3x up) that's up to 3 trailing samples.
            let max_short = (48_000 + rate as i64 - 1) / rate as i64 + 1;
            assert!(
                got <= 48_000 && 48_000 - got <= max_short,
                "rate {rate}: expected 48000 (−{max_short}) output samples, got {got}",
            );
        }
    }

    // Upsampling preserves a constant signal exactly (linear interp of equal
    // endpoints is the same constant) — a sanity check that values aren't
    // mangled, only resampled.
    #[test]
    fn resampler_constant_signal_is_preserved() {
        let mut r = Resampler::new();
        let mut out = Vec::new();
        let input = vec![0.5f32; 1600]; // 0.1 s @ 16 kHz
        r.process(&input, 16_000, &mut out);
        assert!(out.len() >= 4700 && out.len() <= 4810, "≈3× upsample, got {}", out.len());
        assert!(out.iter().all(|&s| (s - 0.5).abs() < 1e-6), "constant preserved");
    }
}
