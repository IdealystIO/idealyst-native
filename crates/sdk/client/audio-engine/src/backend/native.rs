//! Native output via [`cpal`] — CoreAudio on macOS / iOS, ALSA on Linux,
//! WASAPI on Windows. One code path for all four; the only per-platform
//! piece is the iOS `AVAudioSession` setup in [`ios_session`].
//!
//! The cpal callback owns the [`Mixer`] and renders straight into the
//! device buffer (through an `f32` scratch for non-float devices).

use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, FromSample, SampleFormat, SizedSample, SupportedBufferSize};

use crate::mixer::{self, CommandQueue, Mixer, MixerEngine};
use crate::EngineError;

use super::{EngineBackend, OutputDevice};

/// The device buffer we ask for, in milliseconds of audio (rounded up to a
/// power of two in frames: 256 at 44.1 / 48 kHz). Small enough that a
/// gunshot leaves the speaker within a few ms of `play`; large enough that
/// a 128-voice mix fits comfortably in one callback on a phone. Requested
/// with cpal `BufferSize::Fixed`, clamped to the device's supported range;
/// hosts that refuse a fixed size (cpal's iOS host) get the default, with
/// iOS's own preferred IO buffer duration set to the same target instead.
pub const TARGET_BUFFER_MS: u32 = 5;

/// `f32` scratch pre-reserved for non-float devices, in samples. Covers a
/// 4096-frame stereo callback without growing; a larger callback grows it
/// once (the only allocation the callback can make).
const SCRATCH_SAMPLES: usize = 8192;

struct CpalDevice {
    stream: cpal::Stream,
    /// Latest measured latency in µs (0 = not measured yet).
    measured_us: Arc<AtomicU64>,
    /// The fixed buffer we got, for an estimate before the first callback.
    fixed_frames: Option<u32>,
    sample_rate: u32,
}

impl OutputDevice for CpalDevice {
    fn output_latency(&self) -> Option<Duration> {
        match self.measured_us.load(Ordering::Relaxed) {
            0 => self
                .fixed_frames
                .map(|f| Duration::from_secs_f64(f as f64 / self.sample_rate as f64)),
            us => Some(Duration::from_micros(us)),
        }
    }

    fn suspend(&self) {
        if let Err(e) = self.stream.pause() {
            eprintln!("audio-engine: pause failed: {e}");
        }
    }

    fn resume(&self) {
        if let Err(e) = self.stream.play() {
            eprintln!("audio-engine: resume failed: {e}");
        }
    }
}

pub(crate) async fn start() -> Result<Rc<dyn EngineBackend>, EngineError> {
    #[cfg(target_os = "ios")]
    ios_session::configure();

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or(EngineError::NoOutputDevice)?;
    let supported = device.default_output_config().map_err(|e| match e {
        cpal::DefaultStreamConfigError::DeviceNotAvailable => EngineError::NoOutputDevice,
        other => EngineError::Backend(format!("default_output_config: {other}")),
    })?;
    let sample_format = supported.sample_format();
    let mut config: cpal::StreamConfig = supported.config();
    let sample_rate = config.sample_rate.0;

    let queue = mixer::new_queue();
    let measured_us = Arc::new(AtomicU64::new(0));
    let fixed = fixed_buffer_frames(supported.buffer_size(), sample_rate);

    let mut stream = None;
    let mut fixed_frames = None;
    if let Some(frames) = fixed {
        config.buffer_size = BufferSize::Fixed(frames);
        if let Ok(s) = build_any(sample_format, &device, &config, &queue, &measured_us) {
            stream = Some(s);
            fixed_frames = Some(frames);
        }
    }
    let stream = match stream {
        Some(s) => s,
        None => {
            config.buffer_size = BufferSize::Default;
            build_any(sample_format, &device, &config, &queue, &measured_us)?
        }
    };
    stream
        .play()
        .map_err(|e| EngineError::Backend(format!("stream play: {e}")))?;

    let device = CpalDevice {
        stream,
        measured_us,
        fixed_frames,
        sample_rate,
    };
    Ok(Rc::new(MixerEngine::new(
        queue,
        sample_rate,
        Box::new(device),
    )))
}

/// `TARGET_BUFFER_MS` in frames (next power of two), clamped to the
/// device's range; `None` when the host reports no range.
fn fixed_buffer_frames(range: &SupportedBufferSize, sample_rate: u32) -> Option<u32> {
    let target = (sample_rate as u64 * TARGET_BUFFER_MS as u64 / 1000).max(1) as u32;
    let target = target.next_power_of_two();
    match *range {
        SupportedBufferSize::Range { min, max } if max > 0 && min <= max => {
            Some(target.clamp(min.max(1), max))
        }
        _ => None,
    }
}

fn build_any(
    format: SampleFormat,
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    queue: &Arc<CommandQueue>,
    measured: &Arc<AtomicU64>,
) -> Result<cpal::Stream, EngineError> {
    match format {
        SampleFormat::F32 => build::<f32>(device, config, queue, measured),
        SampleFormat::I16 => build::<i16>(device, config, queue, measured),
        SampleFormat::U16 => build::<u16>(device, config, queue, measured),
        SampleFormat::I32 => build::<i32>(device, config, queue, measured),
        SampleFormat::I8 => build::<i8>(device, config, queue, measured),
        SampleFormat::U8 => build::<u8>(device, config, queue, measured),
        SampleFormat::F64 => build::<f64>(device, config, queue, measured),
        other => Err(EngineError::Backend(format!(
            "device sample format {other:?} not handled"
        ))),
    }
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    queue: &Arc<CommandQueue>,
    measured: &Arc<AtomicU64>,
) -> Result<cpal::Stream, EngineError>
where
    T: SizedSample + FromSample<f32>,
{
    let rate = config.sample_rate.0;
    let channels = config.channels;
    let mut mixer = Mixer::new(queue.clone(), rate);
    let mut scratch: Vec<f32> = Vec::with_capacity(SCRATCH_SAMPLES);
    let measured = measured.clone();
    device
        .build_output_stream(
            config,
            move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
                // Latency = device-side delay of this buffer (playback
                // instant − callback instant) + the buffer itself.
                let ts = info.timestamp();
                let frames = data.len() / channels.max(1) as usize;
                if let Some(d) = ts.playback.duration_since(&ts.callback) {
                    let buffer = Duration::from_secs_f64(frames as f64 / rate as f64);
                    measured.store((d + buffer).as_micros().max(1) as u64, Ordering::Relaxed);
                }
                if scratch.len() != data.len() {
                    scratch.resize(data.len(), 0.0);
                }
                mixer.render(&mut scratch, channels, rate);
                for (d, s) in data.iter_mut().zip(scratch.iter()) {
                    *d = T::from_sample(*s);
                }
            },
            |err| eprintln!("audio-engine: output stream error: {err}"),
            None,
        )
        .map_err(|e| match e {
            cpal::BuildStreamError::DeviceNotAvailable => EngineError::NoOutputDevice,
            other => EngineError::Backend(format!("build_output_stream: {other}")),
        })
}

// ---------------------------------------------------------------------------
// iOS: AVAudioSession.
//
// cpal opens the output AudioUnit but never touches AVAudioSession. The
// default category (SoloAmbient) silences every other app's audio and
// ignores nothing; games want `crate::IOS_SESSION_CATEGORY` (Ambient).
// ---------------------------------------------------------------------------

#[cfg(target_os = "ios")]
mod ios_session {
    use objc2::runtime::{AnyObject, Bool};
    use objc2::{class, msg_send};
    use objc2_foundation::NSString;
    use std::ptr;

    use super::TARGET_BUFFER_MS;

    // AVAudioSession is resolved by name at runtime; link the framework so
    // the class exists (declared in Cargo.toml metadata for the Xcode link).
    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" {}

    /// The category a fresh app session starts in.
    const DEFAULT_CATEGORY: &str = "AVAudioSessionCategorySoloAmbient";

    /// Put the session in [`crate::IOS_SESSION_CATEGORY`] — only if the app
    /// hasn't chosen a category itself (still the default). An app that
    /// records (`microphone` sets PlayAndRecord) or set Playback keeps its
    /// choice: switching a recording session to Ambient would kill capture.
    /// Then ask for a small IO buffer and activate. Failures are logged, not
    /// fatal: output still works in whatever category the session is in.
    pub(super) fn configure() {
        unsafe {
            let session: *mut AnyObject = msg_send![class!(AVAudioSession), sharedInstance];
            let current: *mut NSString = msg_send![session, category];
            let is_default = current.is_null() || (*current).to_string() == DEFAULT_CATEGORY;
            if is_default {
                let category = NSString::from_str(crate::IOS_SESSION_CATEGORY);
                let ok: Bool = msg_send![
                    session,
                    setCategory: &*category,
                    error: ptr::null_mut::<*mut AnyObject>(),
                ];
                if !ok.as_bool() {
                    eprintln!("audio-engine: AVAudioSession setCategory failed");
                }
            }
            let secs = TARGET_BUFFER_MS as f64 / 1000.0;
            let _: Bool = msg_send![
                session,
                setPreferredIOBufferDuration: secs,
                error: ptr::null_mut::<*mut AnyObject>(),
            ];
            let ok: Bool = msg_send![
                session,
                setActive: Bool::YES,
                error: ptr::null_mut::<*mut AnyObject>(),
            ];
            if !ok.as_bool() {
                eprintln!("audio-engine: AVAudioSession setActive failed");
            }
        }
    }
}
