//! [`VoiceParams`] — how a voice starts — and the value sanitizing every
//! backend shares, so the web graph and the Rust mixer clamp identically.

/// The fastest playback rate a voice accepts (4 octaves up). Web Audio has no
/// hard cap, but past this a linear-interpolated buffer is all aliasing.
pub const MAX_RATE: f32 = 16.0;

/// How a voice starts. Every field can be changed later through
/// [`Voice`](crate::Voice) setters (with a ramp), except
/// [`start_offset_secs`](Self::start_offset_secs).
///
/// ```
/// use audio_engine::VoiceParams;
/// let p = VoiceParams::new().gain(0.5).pan(-0.25).rate(1.5).lowpass(2_000.0).looping(true);
/// assert_eq!(p.lowpass_hz, Some(2_000.0));
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoiceParams {
    /// Linear gain (`1` = unchanged). Negative / NaN become `0`.
    pub gain: f32,
    /// Stereo position, `-1` (left) … `1` (right), `StereoPannerNode`
    /// equal-power law. Clamped.
    pub pan: f32,
    /// Playback rate: `2` = an octave up and half as long. Clamped to
    /// `[0, MAX_RATE]` (`0` holds the playhead). Ignored for stream voices.
    pub rate: f32,
    /// Low-pass cutoff in Hz (`BiquadFilterNode` `"lowpass"`, Q 1 dB), or
    /// `None` for no filter. Clamped to `[0, output Nyquist]`.
    pub lowpass_hz: Option<f32>,
    /// Loop the whole buffer until [`Voice::stop`](crate::Voice::stop) or
    /// [`Voice::set_looping(false)`](crate::Voice::set_looping).
    pub looping: bool,
    /// Where in the buffer to start, in seconds. Clamped to the buffer; for a
    /// looping voice it wraps. Ignored for stream voices.
    pub start_offset_secs: f32,
}

impl Default for VoiceParams {
    fn default() -> Self {
        VoiceParams {
            gain: 1.0,
            pan: 0.0,
            rate: 1.0,
            lowpass_hz: None,
            looping: false,
            start_offset_secs: 0.0,
        }
    }
}

impl VoiceParams {
    /// The defaults: gain 1, centered, rate 1, no filter, one-shot, from 0.
    pub fn new() -> Self {
        Self::default()
    }
    /// Set [`gain`](Self::gain).
    pub fn gain(mut self, gain: f32) -> Self {
        self.gain = gain;
        self
    }
    /// Set [`pan`](Self::pan).
    pub fn pan(mut self, pan: f32) -> Self {
        self.pan = pan;
        self
    }
    /// Set [`rate`](Self::rate).
    pub fn rate(mut self, rate: f32) -> Self {
        self.rate = rate;
        self
    }
    /// Set a low-pass cutoff ([`lowpass_hz`](Self::lowpass_hz)).
    pub fn lowpass(mut self, hz: f32) -> Self {
        self.lowpass_hz = Some(hz);
        self
    }
    /// Set [`looping`](Self::looping).
    pub fn looping(mut self, looping: bool) -> Self {
        self.looping = looping;
        self
    }
    /// Set [`start_offset_secs`](Self::start_offset_secs).
    pub fn start_offset(mut self, secs: f32) -> Self {
        self.start_offset_secs = secs;
        self
    }

    /// Gain, pan and low-pass for a sound at `source` heard by `listener`
    /// (see [`spatial::params_for`](crate::spatial::params_for)); everything
    /// else default. Chain `.gain(..)` AFTER this only to replace the
    /// distance gain — to scale it, multiply: `p.gain(p.gain * 0.5)`.
    pub fn spatial(
        listener: &crate::spatial::Listener,
        source: [f32; 3],
        attenuation: &crate::spatial::Attenuation,
    ) -> Self {
        let (gain, pan, lowpass_hz) = crate::spatial::params_for(listener, source, attenuation);
        VoiceParams {
            gain,
            pan,
            lowpass_hz,
            ..Self::default()
        }
    }

    /// The values a backend actually uses (see each field's doc for the
    /// rule). Public so the web backend applies the exact same clamps.
    #[doc(hidden)]
    pub fn sanitized(self) -> Self {
        VoiceParams {
            gain: sanitize_gain(self.gain),
            pan: sanitize_pan(self.pan),
            rate: sanitize_rate(self.rate),
            lowpass_hz: self.lowpass_hz.map(sanitize_hz),
            looping: self.looping,
            start_offset_secs: sanitize_secs(self.start_offset_secs),
        }
    }
}

#[doc(hidden)]
pub fn sanitize_gain(g: f32) -> f32 {
    if g.is_nan() {
        0.0
    } else {
        g.clamp(0.0, f32::MAX)
    }
}

#[doc(hidden)]
pub fn sanitize_pan(p: f32) -> f32 {
    if p.is_nan() {
        0.0
    } else {
        p.clamp(-1.0, 1.0)
    }
}

#[doc(hidden)]
pub fn sanitize_rate(r: f32) -> f32 {
    if r.is_nan() {
        1.0
    } else {
        r.clamp(0.0, MAX_RATE)
    }
}

/// Non-negative; the upper clamp (Nyquist) needs the output rate, so it
/// happens where that is known.
#[doc(hidden)]
pub fn sanitize_hz(hz: f32) -> f32 {
    if hz.is_nan() {
        f32::MAX
    } else {
        hz.max(0.0)
    }
}

/// Ramp / fade / offset durations: negative or NaN → 0.
#[doc(hidden)]
pub fn sanitize_secs(s: f32) -> f32 {
    if s.is_nan() || s < 0.0 {
        0.0
    } else {
        s
    }
}
