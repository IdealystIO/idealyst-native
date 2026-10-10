//! Ready-made game sound effects, each a few lines over the primitives —
//! use them as-is or as starting points (read the source: that's the point).
//!
//! All render at [`DEFAULT_FORMAT`](crate::DEFAULT_FORMAT) (44.1 kHz mono),
//! peak at roughly `0.5`–`0.8`, and are deterministic.

use crate::envelope::Envelope;
use crate::generate::{noise, sweep, tone, Noise, Sweep, Waveform};
use crate::pcm::Pcm;
use crate::DEFAULT_FORMAT as F;

/// A short UI / menu blip: 60 ms of an 880 Hz square, quick decay.
pub fn blip() -> Pcm {
    tone(Waveform::Square, 880.0, 0.06, F)
        .apply_envelope(Envelope::percussive(0.002, 0.06))
        .gain(0.5)
}

/// A "pew" laser: a square diving 1.2 kHz → 180 Hz over 250 ms.
pub fn laser() -> Pcm {
    sweep(Waveform::Square, 1200.0, 180.0, 0.25, Sweep::Exponential, F)
        .apply_envelope(Envelope::new(0.003, 0.05, 0.5, 0.12))
        .gain(0.45)
}

/// An explosion: brown noise, low-passed to a rumble, with a long decay.
pub fn explosion() -> Pcm {
    noise(Noise::Brown, 0.9, F, 0xB00)
        .mix(&noise(Noise::White, 0.08, F, 0xB01).gain(0.6))
        .low_pass(500.0)
        .apply_envelope(Envelope::percussive(0.004, 0.9))
        .normalize(0.8)
}

/// A pickup / coin: two quick rising square notes (B5 → E6).
pub fn coin() -> Pcm {
    let env = Envelope::new(0.002, 0.02, 0.7, 0.03);
    tone(Waveform::Square, 987.77, 0.07, F)
        .apply_envelope(env)
        .concat(&tone(Waveform::Square, 1318.51, 0.2, F).apply_envelope(Envelope::new(
            0.002, 0.04, 0.6, 0.15,
        )))
        .gain(0.4)
}

/// A hit / punch: a white-noise crack over a falling 160 → 60 Hz thump.
pub fn hit() -> Pcm {
    let thump = sweep(Waveform::Sine, 160.0, 60.0, 0.18, Sweep::Exponential, F)
        .apply_envelope(Envelope::percussive(0.002, 0.18));
    let crack = noise(Noise::White, 0.05, F, 0x417)
        .high_pass(1500.0)
        .apply_envelope(Envelope::percussive(0.001, 0.05));
    thump.mix(&crack.gain(0.7)).normalize(0.75)
}

/// A jump: a triangle rising 220 → 660 Hz over 180 ms.
pub fn jump() -> Pcm {
    sweep(Waveform::Triangle, 220.0, 660.0, 0.18, Sweep::Exponential, F)
        .apply_envelope(Envelope::new(0.005, 0.03, 0.8, 0.06))
        .gain(0.6)
}

/// A power-down: a sawtooth sliding 600 → 60 Hz over 600 ms, bit-crushed.
pub fn power_down() -> Pcm {
    sweep(Waveform::Sawtooth, 600.0, 60.0, 0.6, Sweep::Exponential, F)
        .bitcrush(4)
        .apply_envelope(Envelope::new(0.005, 0.1, 0.7, 0.25))
        .gain(0.4)
}
