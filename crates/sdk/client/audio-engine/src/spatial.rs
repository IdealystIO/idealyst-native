//! Positional sound as plain math: a listener and a source position in, the
//! voice's gain / pan / low-pass out. No engine state — call it each frame
//! for a moving source and feed the result to
//! [`Voice::set_spatial`](crate::Voice::set_spatial) (or the individual
//! setters) with a short ramp.
//!
//! Stereo can only place a sound left/right: a source straight behind pans
//! the same as one straight ahead. Distance is carried by gain (inverse
//! distance, like Web Audio's `PannerNode` `"inverse"` model) and by
//! muffling (a low-pass falling toward
//! [`muffle_hz_at_max`](Attenuation::muffle_hz_at_max)), the cue that makes
//! far sounds read as far rather than just quiet.

/// The ear: where it is and which way it faces. `forward` and `up` need not
/// be normalized, but must not be parallel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Listener {
    /// World position.
    pub position: [f32; 3],
    /// Facing direction.
    pub forward: [f32; 3],
    /// Head-up direction.
    pub up: [f32; 3],
}

impl Default for Listener {
    /// At the origin, facing −Z with +Y up (the OpenGL / Web Audio
    /// convention), so +X is to the right.
    fn default() -> Self {
        Listener {
            position: [0.0; 3],
            forward: [0.0, 0.0, -1.0],
            up: [0.0, 1.0, 0.0],
        }
    }
}

/// How sound falls off with distance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Attenuation {
    /// Distance at (and inside) which the sound is at full gain, unfiltered.
    pub ref_distance: f32,
    /// Distance past which nothing changes any more (gain and muffling stop
    /// falling — OpenAL's "inverse distance clamped").
    pub max_distance: f32,
    /// How fast gain falls: `gain = ref / (ref + rolloff·(d − ref))`.
    /// `0` disables distance gain.
    pub rolloff: f32,
    /// Low-pass cutoff at `max_distance`. Between `ref_distance` and
    /// `max_distance` the cutoff slides geometrically from
    /// [`UNMUFFLED_HZ`] down to this. `≤ 0` or `≥ UNMUFFLED_HZ` disables
    /// muffling.
    pub muffle_hz_at_max: f32,
}

impl Default for Attenuation {
    /// ref 1, max 50, rolloff 1, muffled to 1.5 kHz at max distance.
    fn default() -> Self {
        Attenuation {
            ref_distance: 1.0,
            max_distance: 50.0,
            rolloff: 1.0,
            muffle_hz_at_max: 1_500.0,
        }
    }
}

/// The cutoff muffling starts from at `ref_distance` (the top of human
/// hearing — effectively unfiltered).
pub const UNMUFFLED_HZ: f32 = 20_000.0;

/// `(gain, pan, lowpass)` for a sound at `source` heard by `listener`.
///
/// - **pan** = the component of the (normalized) listener→source direction
///   along the listener's right vector (`forward × up`): `1` hard right,
///   `-1` hard left, `0` dead ahead or behind.
/// - **gain** = inverse distance, clamped to `[ref_distance, max_distance]`.
/// - **lowpass** = `None` within `ref_distance`, else a cutoff falling
///   geometrically from [`UNMUFFLED_HZ`] to `muffle_hz_at_max` at
///   `max_distance`.
pub fn params_for(
    listener: &Listener,
    source: [f32; 3],
    att: &Attenuation,
) -> (f32, f32, Option<f32>) {
    let d = sub(source, listener.position);
    let dist = len(d);
    let ref_d = att.ref_distance.max(f32::EPSILON);
    let max_d = att.max_distance.max(ref_d);

    let pan = if dist <= f32::EPSILON {
        0.0
    } else {
        let right = normalize(cross(listener.forward, listener.up));
        let dir = scale(d, 1.0 / dist);
        dot(dir, right).clamp(-1.0, 1.0)
    };

    let clamped = dist.clamp(ref_d, max_d);
    let rolloff = att.rolloff.max(0.0);
    let gain = ref_d / (ref_d + rolloff * (clamped - ref_d));

    let muffle = att.muffle_hz_at_max;
    let lowpass = if dist <= ref_d || muffle <= 0.0 || muffle >= UNMUFFLED_HZ || max_d <= ref_d {
        None
    } else {
        let t = (clamped - ref_d) / (max_d - ref_d);
        Some(UNMUFFLED_HZ * (muffle / UNMUFFLED_HZ).powf(t))
    };

    (gain, pan, lowpass)
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn len(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}
fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn normalize(a: [f32; 3]) -> [f32; 3] {
    let l = len(a);
    if l <= f32::EPSILON {
        [0.0; 3]
    } else {
        scale(a, 1.0 / l)
    }
}
