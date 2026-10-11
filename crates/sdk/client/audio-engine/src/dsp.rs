//! The DSP building blocks, each a transcription of the **Web Audio spec**
//! formula for the node the web backend uses. That is the convergence
//! contract (rule 7): the browser runs `BiquadFilterNode` /
//! `StereoPannerNode` / `AudioParam.linearRampToValueAtTime`, every other
//! target runs these, and the two produce the same samples.
//!
//! Public (doc-hidden) so the tests — and the web backend's spec-convergence
//! test (`backend/web/tests.rs`) — can check them against an independent computation.

use std::f64::consts::PI;

/// The Web Audio `BiquadFilterNode.Q` default. For the `"lowpass"` type the
/// spec interprets Q in **dB** (`α_Q = sin ω0 / (2·10^(Q/20))`); 1 dB gives a
/// gentle 12 dB/octave slope with a tiny (~1 dB) bump at the cutoff.
pub const LOWPASS_Q_DB: f64 = 1.0;

/// Normalized biquad coefficients `[b0, b1, b2, a1, a2]` (each divided by
/// `a0`) for the Web Audio `"lowpass"` filter at `f0` Hz, sample rate `fs`,
/// resonance `q_db` (dB).
///
/// The spec's formula (Web Audio API §1.13, "Filters characteristics"):
///
/// ```text
/// ω0 = 2π·f0/Fs            α_Q = sin ω0 / (2·10^(Q/20))
/// b0 = (1 − cos ω0)/2      b1 = 1 − cos ω0      b2 = (1 − cos ω0)/2
/// a0 = 1 + α_Q             a1 = −2 cos ω0       a2 = 1 − α_Q
/// ```
///
/// `f0` is clamped to the spec's nominal range `[0, Fs/2]`. The two ends are
/// special-cased exactly as browsers implement them (Chromium's
/// `Biquad::SetLowpassParams`): at Nyquist the transfer function is `1`
/// (pass everything — the formula gives a pole/zero pair at z = −1 that only
/// cancels in exact arithmetic), at 0 Hz it is `0`.
pub fn lowpass_coefficients(fs: f64, f0: f64, q_db: f64) -> [f64; 5] {
    let nyquist = fs / 2.0;
    let f0 = if f0.is_nan() {
        nyquist
    } else {
        f0.clamp(0.0, nyquist)
    };
    let normalized = f0 / nyquist;
    if normalized >= 1.0 {
        return [1.0, 0.0, 0.0, 0.0, 0.0];
    }
    if normalized <= 0.0 {
        return [0.0, 0.0, 0.0, 0.0, 0.0];
    }
    let w0 = 2.0 * PI * f0 / fs;
    let (sin, cos) = w0.sin_cos();
    let alpha = sin / (2.0 * 10f64.powf(q_db / 20.0));
    let a0 = 1.0 + alpha;
    let b0 = (1.0 - cos) / 2.0;
    let b1 = 1.0 - cos;
    let b2 = (1.0 - cos) / 2.0;
    let a1 = -2.0 * cos;
    let a2 = 1.0 - alpha;
    [b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0]
}

/// One channel of a Direct-Form-I biquad (the form the spec's difference
/// equation is written in). `f64` state: at cutoffs near Nyquist the poles
/// sit close to the unit circle and `f32` rounding is audible as drift.
#[derive(Clone, Copy, Default, Debug)]
pub struct BiquadState {
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl BiquadState {
    /// Filter one sample with coefficients `c` (`[b0, b1, b2, a1, a2]`).
    #[inline]
    pub fn process(&mut self, c: &[f64; 5], x: f32) -> f32 {
        let x = x as f64;
        let y = c[0] * x + c[1] * self.x1 + c[2] * self.x2 - c[3] * self.y1 - c[4] * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y as f32
    }

    /// Pass one sample through untouched while keeping the history
    /// consistent with an identity filter (`y = x`). Re-enabling the filter
    /// at Nyquist (where it IS the identity) then continues without a
    /// discontinuity — the click-free `None → Some` switch.
    #[inline]
    pub fn bypass(&mut self, x: f32) -> f32 {
        let x64 = x as f64;
        self.x2 = self.x1;
        self.x1 = x64;
        self.y2 = self.y1;
        self.y1 = x64;
        x
    }
}

/// `StereoPannerNode` gains for a **mono** input (spec §1.31 "StereoPanner
/// algorithm"): `x = (pan + 1)/2`, `L = cos(x·π/2)`, `R = sin(x·π/2)`.
/// `pan` is clamped to `[-1, 1]` (the spec's nominal range).
#[inline]
pub fn pan_mono(pan: f32, input: f32) -> (f32, f32) {
    let pan = clamp_pan(pan) as f64;
    let x = (pan + 1.0) / 2.0;
    let (gl, gr) = ((x * PI / 2.0).cos(), (x * PI / 2.0).sin());
    ((input as f64 * gl) as f32, (input as f64 * gr) as f32)
}

/// `StereoPannerNode` for a **stereo** input (same spec section): for
/// `pan ≤ 0`, `x = pan + 1`, `L = inL + inR·cos(x·π/2)`, `R = inR·sin(x·π/2)`;
/// for `pan > 0`, `x = pan`, `L = inL·cos(x·π/2)`, `R = inR + inL·sin(x·π/2)`.
#[inline]
pub fn pan_stereo(pan: f32, l: f32, r: f32) -> (f32, f32) {
    let pan = clamp_pan(pan) as f64;
    let (l, r) = (l as f64, r as f64);
    if pan <= 0.0 {
        let x = pan + 1.0;
        let (gl, gr) = ((x * PI / 2.0).cos(), (x * PI / 2.0).sin());
        ((l + r * gl) as f32, (r * gr) as f32)
    } else {
        let x = pan;
        let (gl, gr) = ((x * PI / 2.0).cos(), (x * PI / 2.0).sin());
        ((l * gl) as f32, (r + l * gr) as f32)
    }
}

fn clamp_pan(pan: f32) -> f32 {
    if pan.is_nan() {
        0.0
    } else {
        pan.clamp(-1.0, 1.0)
    }
}

/// A rampable parameter with `AudioParam.linearRampToValueAtTime` semantics:
/// a ramp issued at frame `n0` with value `V0` toward `V1` over `N` frames
/// takes the value `V0 + (V1 − V0)·k/N` at frame `n0 + k`, reaching exactly
/// `V1` at `k = N` and holding it after. A new ramp starts from the value the
/// parameter has at that moment (the web backend issues
/// `setValueAtTime(current, now)` before each ramp to get the same anchor).
#[derive(Clone, Copy, Debug)]
pub struct Param {
    value: f32,
    start: f32,
    target: f32,
    total: u32,
    elapsed: u32,
}

impl Param {
    /// A parameter resting at `value`.
    pub fn new(value: f32) -> Param {
        Param {
            value,
            start: value,
            target: value,
            total: 0,
            elapsed: 0,
        }
    }

    /// Ramp to `target` over `frames` frames (`0` = jump now).
    pub fn ramp_to(&mut self, target: f32, frames: u32) {
        if frames == 0 {
            *self = Param::new(target);
        } else {
            self.start = self.value;
            self.target = target;
            self.total = frames;
            self.elapsed = 0;
        }
    }

    /// The value at the current frame, then advance one frame.
    #[inline]
    pub fn tick(&mut self) -> f32 {
        let v = self.value;
        self.advance(1);
        v
    }

    /// The value at the current frame (no advance).
    #[inline]
    pub fn current(&self) -> f32 {
        self.value
    }

    /// The value the parameter is heading to (its current value when idle).
    pub fn target(&self) -> f32 {
        self.target
    }

    /// `true` while a ramp is in progress.
    #[inline]
    pub fn is_ramping(&self) -> bool {
        self.elapsed < self.total
    }

    /// Advance `frames` frames (the k-rate path: one value per quantum).
    #[inline]
    pub fn advance(&mut self, frames: u32) {
        if self.elapsed < self.total {
            self.elapsed = self.elapsed.saturating_add(frames).min(self.total);
            self.value = if self.elapsed >= self.total {
                self.target
            } else {
                // Exact per-frame formula, not an accumulated step: no drift,
                // and the target is hit bit-exactly at k = N.
                self.start + (self.target - self.start) * (self.elapsed as f32 / self.total as f32)
            };
        }
    }
}

/// Seconds → whole frames at `rate` (rounded; negative / NaN → 0).
pub fn secs_to_frames(secs: f32, rate: u32) -> u32 {
    if secs.is_nan() || secs <= 0.0 {
        return 0;
    }
    (secs as f64 * rate as f64).round().min(u32::MAX as f64) as u32
}
