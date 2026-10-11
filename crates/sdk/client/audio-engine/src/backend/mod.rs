//! The backend seam.
//!
//! Two layers, so each platform task replaces exactly one module:
//!
//! - [`EngineBackend`] — what [`AudioEngine`](crate::AudioEngine) drives.
//!   The **web** backend implements it directly on a Web Audio node graph
//!   (the browser is the mixer). Every other target uses
//!   [`MixerEngine`](crate::mixer::MixerEngine), which implements it on top
//!   of the Rust mixer.
//! - [`OutputDevice`] — the device half of a mixer backend: whatever owns
//!   the platform's audio callback and calls `Mixer::render` from it (cpal on
//!   desktop / iOS, AAudio on Android, nothing for offline rendering).
//!
//! Each platform module exposes `pub(crate) async fn start() ->
//! Result<Rc<dyn EngineBackend>, EngineError>`; [`start`] picks one by cfg.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use media_stream::AudioStream;

use crate::mixer::{VoiceOp, VoiceShared};
use crate::{EngineError, SoundBuffer, VoiceParams};

#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
mod native;

#[cfg(target_arch = "wasm32")]
mod web;

#[cfg(target_os = "android")]
mod android;

// The Android backend's platform-free rules (API gating, open order, latency
// math); compiled on the host too so its unit tests run there.
#[cfg(any(target_os = "android", test))]
mod aaudio_policy;

/// Everything `AudioEngine` / `Voice` need from a backend. Values arrive
/// sanitized (`VoiceParams::sanitized`, clamped ops), so implementations
/// apply them as-is.
pub(crate) trait EngineBackend {
    /// The output sample rate.
    fn sample_rate(&self) -> u32;
    /// Best estimate of the time from `play` to sound at the speaker.
    fn output_latency(&self) -> Option<Duration>;
    /// Start a buffer voice. Returns its id and play-state flag (set `true`
    /// now; the backend clears it when the voice ends).
    fn play(&self, buf: &SoundBuffer, params: VoiceParams) -> (u64, Arc<VoiceShared>);
    /// Start a live stream voice (`params.rate` / `start_offset_secs` are
    /// ignored).
    fn play_stream(&self, stream: &AudioStream, params: VoiceParams) -> (u64, Arc<VoiceShared>);
    /// Change voice `id`. Unknown / finished ids are ignored.
    fn voice(&self, id: u64, op: VoiceOp);
    /// Ramp the master gain.
    fn set_master_gain(&self, gain: f32, ramp_secs: f32);
    /// Resume output (web: `AudioContext.resume()`).
    fn resume(&self);
    /// Pause output.
    fn suspend(&self);
    /// Voices currently playing (including ones started but not yet
    /// rendered).
    fn active_voices(&self) -> usize;
}

/// The device half of a mixer-driven backend. Dropping it closes the output
/// (and drops the `Mixer` its callback owns).
pub(crate) trait OutputDevice {
    /// Best estimate of the output latency.
    fn output_latency(&self) -> Option<Duration>;
    /// Pause the device stream.
    fn suspend(&self);
    /// Resume the device stream.
    fn resume(&self);
}

/// Open the default output on this target.
pub(crate) async fn start() -> Result<Rc<dyn EngineBackend>, EngineError> {
    #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
    {
        native::start().await
    }
    #[cfg(target_arch = "wasm32")]
    {
        web::start().await
    }
    #[cfg(target_os = "android")]
    {
        android::start().await
    }
}

/// The device of an offline engine: there is none — the
/// [`OfflineRenderer`](crate::OfflineRenderer) owns the mixer and is pulled
/// by its owner.
pub(crate) struct NoDevice;

impl OutputDevice for NoDevice {
    fn output_latency(&self) -> Option<Duration> {
        None
    }
    fn suspend(&self) {}
    fn resume(&self) {}
}
