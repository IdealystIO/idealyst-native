//! What to ask the device for. Everything is optional — `None` means
//! "let the platform pick its native default", which is what most
//! callers want (the device's preferred rate is the cheapest path, no
//! resampling).

/// Requested capture parameters. A `None` field defers to the device's
/// preferred value. Construct with [`AudioStreamConfig::default`] (device
/// defaults) or the small builders below.
///
/// These are *requests*. A backend that can't honour an explicit value
/// returns [`MicError::UnsupportedConfig`](crate::MicError::UnsupportedConfig)
/// rather than silently substituting — so the actual rate/channels you
/// observe on each [`AudioBuffer`](crate::AudioBuffer) are authoritative.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AudioStreamConfig {
    /// Desired sample rate in Hz (e.g. `16_000`, `44_100`, `48_000`).
    /// `None` = the device's default rate.
    pub sample_rate: Option<u32>,

    /// Desired channel count (`1` = mono, `2` = stereo). `None` = the
    /// device default, which for a microphone is usually mono. Samples
    /// in the callback are interleaved when this is > 1.
    pub channels: Option<u16>,

    /// **Web only.** Whether the browser applies its built-in **noise
    /// suppression** to the captured signal. `None` = the browser default,
    /// which is `true` in Chrome/Firefox/Safari. Set `Some(false)` to get the
    /// unprocessed mic — e.g. when the app runs its own denoiser and doesn't
    /// want the browser suppressing (or double-processing) first. **No effect
    /// on native / Android**, where capture is already raw (`cpal` /
    /// `AudioRecord`) and the browser's DSP simply doesn't exist.
    pub noise_suppression: Option<bool>,

    /// **Web only.** Browser **echo cancellation** (AEC). `None` = the browser
    /// default (`true`). Set `Some(false)` for the unprocessed signal. Native /
    /// Android no-op — see [`noise_suppression`](Self::noise_suppression).
    pub echo_cancellation: Option<bool>,

    /// **Web only.** Browser **auto gain control** (AGC). `None` = the browser
    /// default (`true`). Turning it off (`Some(false)`) yields a truer, more
    /// faithful level but a *much quieter* web mic (getUserMedia AGC is what
    /// normally lifts web input to a usable level). Native / Android no-op —
    /// see [`noise_suppression`](Self::noise_suppression).
    pub auto_gain_control: Option<bool>,
}

impl AudioStreamConfig {
    /// Device defaults for everything — the recommended starting point.
    pub fn new() -> Self {
        Self::default()
    }

    /// Request a specific sample rate, leaving channels at the default.
    pub fn with_sample_rate(mut self, hz: u32) -> Self {
        self.sample_rate = Some(hz);
        self
    }

    /// Request a specific channel count.
    pub fn with_channels(mut self, channels: u16) -> Self {
        self.channels = Some(channels);
        self
    }

    /// Request a single (mono) channel — the common case for voice.
    pub fn mono(self) -> Self {
        self.with_channels(1)
    }

    /// **Web only.** Enable/disable the browser's built-in noise suppression
    /// (see [`noise_suppression`](Self::noise_suppression)). No effect on
    /// native / Android.
    pub fn with_noise_suppression(mut self, on: bool) -> Self {
        self.noise_suppression = Some(on);
        self
    }

    /// **Web only.** Enable/disable the browser's echo cancellation
    /// (see [`echo_cancellation`](Self::echo_cancellation)).
    pub fn with_echo_cancellation(mut self, on: bool) -> Self {
        self.echo_cancellation = Some(on);
        self
    }

    /// **Web only.** Enable/disable the browser's auto gain control
    /// (see [`auto_gain_control`](Self::auto_gain_control)).
    pub fn with_auto_gain_control(mut self, on: bool) -> Self {
        self.auto_gain_control = Some(on);
        self
    }

    /// **Web only.** Request the browser's **unprocessed** signal: disables
    /// noise suppression, echo cancellation, *and* auto gain control in one
    /// call, for an app that owns its own audio processing. Note AGC-off makes
    /// the web mic much quieter (see
    /// [`auto_gain_control`](Self::auto_gain_control)); prefer the individual
    /// setters if you want to keep AGC. Native / Android no-op.
    pub fn unprocessed(mut self) -> Self {
        self.noise_suppression = Some(false);
        self.echo_cancellation = Some(false);
        self.auto_gain_control = Some(false);
        self
    }
}
