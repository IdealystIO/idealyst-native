//! Generators: pure, deterministic functions that render a whole sound into a
//! [`Pcm`]. Every generator writes the same signal to every channel of the
//! requested [`AudioFormat`] and renders at **full scale** (peaks at `±1`) —
//! shape loudness afterwards with [`Pcm::gain`] / [`Pcm::apply_envelope`].

use media_stream::AudioFormat;

use crate::dsp::SplitMix64;
use crate::pcm::{frames_for, Pcm};

/// An oscillator shape for [`tone`] and [`sweep`].
///
/// Every shape starts at phase `0`; [`Sine`](Waveform::Sine),
/// [`Sawtooth`](Waveform::Sawtooth) and [`Triangle`](Waveform::Triangle) start
/// at amplitude `0` (no click on the first sample).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Waveform {
    /// A pure sine — soft, "beep".
    Sine,
    /// A 50 %-duty square, `±1` — the classic chiptune voice.
    Square,
    /// A rising ramp — bright, buzzy.
    Sawtooth,
    /// A triangle — between sine and square; the NES bass voice.
    Triangle,
    /// A square with an adjustable duty cycle (`0..1`, fraction of each period
    /// spent at `+1`). `Pulse(0.5)` is [`Square`](Waveform::Square); thin
    /// duties (`0.125`, `0.25`) are the nasal NES pulse timbres.
    Pulse(f32),
}

impl Waveform {
    /// The waveform's value at `phase` (cycles, wrapped to `0..1`).
    pub fn sample(self, phase: f64) -> f32 {
        let p = phase - phase.floor();
        match self {
            Waveform::Sine => (p * std::f64::consts::TAU).sin() as f32,
            Waveform::Square => {
                if p < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            Waveform::Pulse(duty) => {
                if p < duty.clamp(0.0, 1.0) as f64 {
                    1.0
                } else {
                    -1.0
                }
            }
            // Shifted half a cycle so the ramp starts at 0, not -1.
            Waveform::Sawtooth => {
                let q = (p + 0.5) % 1.0;
                (2.0 * q - 1.0) as f32
            }
            // Shifted a quarter cycle so it starts at 0 rising, like sine.
            Waveform::Triangle => {
                let q = (p + 0.75) % 1.0;
                (1.0 - 4.0 * (q - 0.5).abs()) as f32
            }
        }
    }
}

/// How [`sweep`] moves between its start and end frequency.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sweep {
    /// Frequency changes by equal *hertz* per second. Sounds like it speeds up
    /// at the low end.
    Linear,
    /// Frequency changes by equal *ratios* per second (equal musical
    /// intervals) — the natural-sounding choice for lasers, risers and
    /// power-downs. Both endpoints must be above 0 Hz; they are clamped to
    /// `1 Hz` minimum.
    Exponential,
}

/// `secs` of silence in `format`.
pub fn silence(secs: f32, format: AudioFormat) -> Pcm {
    Pcm::silence(format, secs)
}

/// A constant-frequency tone: `waveform` at `freq_hz` for `secs`.
pub fn tone(waveform: Waveform, freq_hz: f32, secs: f32, format: AudioFormat) -> Pcm {
    let rate = format.sample_rate.max(1) as f64;
    let inc = freq_hz as f64 / rate;
    render(format, secs, |i| waveform.sample(i as f64 * inc))
}

/// A frequency sweep (chirp) from `from_hz` to `to_hz` over `secs`.
///
/// **Phase-continuous**: the oscillator phase is accumulated sample by sample
/// from the instantaneous frequency, so the waveform never jumps — no clicks
/// however steep the sweep.
pub fn sweep(
    waveform: Waveform,
    from_hz: f32,
    to_hz: f32,
    secs: f32,
    curve: Sweep,
    format: AudioFormat,
) -> Pcm {
    let rate = format.sample_rate.max(1) as f64;
    let frames = frames_for(format, secs);
    let span = (frames.max(2) - 1) as f64;
    let (f0, f1) = match curve {
        Sweep::Linear => (from_hz as f64, to_hz as f64),
        Sweep::Exponential => ((from_hz as f64).max(1.0), (to_hz as f64).max(1.0)),
    };
    let mut phase = 0.0f64;
    render(format, secs, move |i| {
        let t = i as f64 / span;
        let f = match curve {
            Sweep::Linear => f0 + (f1 - f0) * t,
            Sweep::Exponential => f0 * (f1 / f0).powf(t),
        };
        let v = waveform.sample(phase);
        phase = (phase + f / rate).fract();
        v
    })
}

/// A noise colour for [`noise`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Noise {
    /// Flat spectrum — hiss, static, the attack of a hit.
    White,
    /// −3 dB/octave — rain, wind, a softer hiss.
    Pink,
    /// −6 dB/octave (integrated white) — rumble, explosions, engines.
    Brown,
}

/// `secs` of `kind` noise, generated from `seed`.
///
/// Deterministic: the same `(kind, secs, format, seed)` always yields the same
/// samples on every platform (a SplitMix64 generator — no OS randomness), so
/// a game's sounds are reproducible and testable. Use different seeds for
/// variation between, say, two explosions.
pub fn noise(kind: Noise, secs: f32, format: AudioFormat, seed: u64) -> Pcm {
    let mut rng = SplitMix64::new(seed);
    // Pink: Paul Kellet's "refined" filter bank over white noise — a
    // well-known ±0.05 dB approximation of a −3 dB/oct slope.
    let mut b = [0.0f32; 7];
    // Brown: leaky integrator; the leak keeps it from drifting off to a DC
    // rail, the gain brings the RMS up to roughly match white's loudness.
    let mut brown = 0.0f32;
    render(format, secs, move |_| {
        let w = rng.next_bipolar();
        let v = match kind {
            Noise::White => w,
            Noise::Pink => {
                b[0] = 0.99886 * b[0] + w * 0.0555179;
                b[1] = 0.99332 * b[1] + w * 0.0750759;
                b[2] = 0.96900 * b[2] + w * 0.153_852;
                b[3] = 0.86650 * b[3] + w * 0.3104856;
                b[4] = 0.55000 * b[4] + w * 0.5329522;
                b[5] = -0.7616 * b[5] - w * 0.0168980;
                let pink = b[0] + b[1] + b[2] + b[3] + b[4] + b[5] + b[6] + w * 0.5362;
                b[6] = w * 0.115926;
                pink * 0.11
            }
            Noise::Brown => {
                brown = (brown + 0.02 * w) / 1.02;
                brown * 3.5
            }
        };
        v.clamp(-1.0, 1.0)
    })
}

/// Render `secs` of `format` by calling `f(frame_index)` once per frame and writing the value to every channel.
fn render(format: AudioFormat, secs: f32, mut f: impl FnMut(usize) -> f32) -> Pcm {
    let ch = format.channels.max(1) as usize;
    let frames = frames_for(format, secs);
    let mut samples = Vec::with_capacity(frames * ch);
    for i in 0..frames {
        let v = f(i);
        for _ in 0..ch {
            samples.push(v);
        }
    }
    Pcm::new(format, samples)
}
