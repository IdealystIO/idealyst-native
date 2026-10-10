//! The ADSR amplitude [`Envelope`].

/// An ADSR (attack / decay / sustain / release) amplitude envelope, applied to
/// a buffer with [`Pcm::apply_envelope`](crate::Pcm::apply_envelope).
///
/// # Fixed-length semantics
///
/// A synthesized sound already has a length, so the envelope is laid over
/// that length rather than driven by a "note off" event:
///
/// ```text
///  1.0 ┤  /\
///      │ /  \______________            sustain level
///      │/                  \
///  0.0 ┼────┬───┬──────────┬───\──
///      0    A  A+D        T-R   T     (T = buffer length)
/// ```
///
/// - **attack** — linear ramp `0 → 1` over `attack` seconds from the start.
/// - **decay** — linear ramp `1 → sustain` over the next `decay` seconds.
/// - **sustain** — the *level* (`0..=1`, not a time) held until the release.
/// - **release** — linear ramp `sustain → 0` over the final `release`
///   seconds, ending exactly at the end of the buffer.
///
/// If `attack + decay + release` is longer than the buffer, the three
/// segments are scaled down proportionally so they fit (the sustain hold
/// shrinks to zero) — a short buffer still gets the envelope's *shape*, it is
/// never truncated mid-ramp.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Envelope {
    /// Attack time in seconds (`0 → 1`).
    pub attack: f32,
    /// Decay time in seconds (`1 → sustain`).
    pub decay: f32,
    /// Sustain level, `0..=1`.
    pub sustain: f32,
    /// Release time in seconds (`sustain → 0`, ending at the buffer's end).
    pub release: f32,
}

impl Envelope {
    /// An envelope from its four parameters. Negative times are treated as
    /// `0`; `sustain` is clamped to `0..=1`.
    pub fn new(attack: f32, decay: f32, sustain: f32, release: f32) -> Self {
        Envelope {
            attack: attack.max(0.0),
            decay: decay.max(0.0),
            sustain: sustain.clamp(0.0, 1.0),
            release: release.max(0.0),
        }
    }

    /// A percussive envelope for a sound `length` seconds long: an `attack`
    /// ramp, then a straight fall to silence over the rest of the length
    /// (sustain `0`, no hold). The usual shape for hits, blips and
    /// explosions.
    pub fn percussive(attack: f32, length: f32) -> Self {
        let attack = attack.max(0.0);
        Envelope::new(attack, (length - attack).max(0.0), 0.0, 0.0)
    }

    /// The gain at time `t` seconds into a buffer `total` seconds long, per
    /// the fixed-length semantics above. `0` outside `0..=total`.
    pub fn gain_at(&self, t: f32, total: f32) -> f32 {
        if !(0.0..=total).contains(&t) || total <= 0.0 {
            return 0.0;
        }
        let (a, d, r) = self.fitted(total);
        let s = self.sustain;
        if t < a {
            return t / a;
        }
        if t < a + d {
            return 1.0 + (s - 1.0) * ((t - a) / d);
        }
        let release_start = total - r;
        if t < release_start {
            return s;
        }
        if r <= 0.0 {
            return s;
        }
        s * ((total - t) / r).clamp(0.0, 1.0)
    }

    /// Attack / decay / release scaled to fit `total` seconds.
    fn fitted(&self, total: f32) -> (f32, f32, f32) {
        let (a, d, r) = (self.attack as f64, self.decay as f64, self.release as f64);
        let sum = a + d + r;
        if sum <= total as f64 || sum <= 0.0 {
            return (self.attack, self.decay, self.release);
        }
        let k = total as f64 / sum;
        ((a * k) as f32, (d * k) as f32, (r * k) as f32)
    }
}
