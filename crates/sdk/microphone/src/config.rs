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
}
