//! Native capture via [`cpal`], which wraps the platform audio stack:
//! CoreAudio on macOS/iOS, WASAPI on Windows, ALSA on Linux. One code
//! path covers all four; the only per-platform divergence is the iOS
//! `AVAudioSession` activation, isolated in [`ios_session`] below.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};

use crate::{AudioBuffer, AudioStreamConfig, BoxedCallback, MicDevice, MicError};

/// Keeps the capture alive. Dropping it stops the stream (cpal tears the
/// platform stream down in `Stream`'s `Drop`).
pub(crate) struct StreamHandle {
    // Field order matters for drop order on iOS: the cpal stream must be
    // torn down before we deactivate the audio session.
    _stream: cpal::Stream,
    #[cfg(target_os = "ios")]
    _session: ios_session::SessionGuard,
}

impl StreamHandle {
    /// cpal exposes no zero-copy native audio source; PCM flows through the
    /// CPU tap. A future `AVAudioEngine`/`AudioTrack` playback path would
    /// publish a handle here.
    pub(crate) fn native_source(&self) -> Option<crate::NativeSource> {
        None
    }
}

/// Request the microphone grant, delegated to the shared `permissions` SDK
/// (which owns the Apple `AVCaptureDevice requestAccessForMediaType:` audio
/// grant + every other platform's request flow). On desktop (Windows/Linux)
/// `permissions` reports `Unsupported` — the OS grants cpal implicitly — which
/// maps to `Ok(())`. iOS's `AVAudioSession` *activation* still happens in
/// [`open`] (it's a capture requirement, not a grant); the record grant itself
/// rides this delegated request.
pub(crate) async fn request_permission() -> Result<(), MicError> {
    map_permission(permissions::request(permissions::Permission::Microphone).await)
}

/// Passive permission query — no prompt, no capture. Delegated to the shared
/// `permissions` SDK (macOS reads `AVCaptureDevice` authorization status; iOS
/// reads its AVCaptureDevice audio status). Windows/Linux have no status API
/// (cpal grants implicitly), surfacing as `Unsupported`, which maps to
/// [`MicPermission::Unknown`].
pub(crate) async fn permission_status() -> crate::MicPermission {
    map_status(permissions::status(permissions::Permission::Microphone).await)
}

/// Map the shared [`permissions::PermissionStatus`] onto microphone's error.
/// `is_usable()` (Granted, or Unsupported where the platform needs no grant)
/// passes; anything else is a denial.
fn map_permission(status: permissions::PermissionStatus) -> Result<(), MicError> {
    if status.is_usable() {
        Ok(())
    } else {
        Err(MicError::PermissionDenied)
    }
}

/// Map the shared [`permissions::PermissionStatus`] onto microphone's own
/// [`MicPermission`]. `Unsupported` (no passive query on this platform — e.g.
/// desktop cpal) becomes [`MicPermission::Unknown`], the existing contract.
fn map_status(status: permissions::PermissionStatus) -> crate::MicPermission {
    match status {
        permissions::PermissionStatus::Granted => crate::MicPermission::Granted,
        permissions::PermissionStatus::Denied | permissions::PermissionStatus::Restricted => {
            crate::MicPermission::Denied
        }
        permissions::PermissionStatus::Undetermined => crate::MicPermission::Undetermined,
        permissions::PermissionStatus::Unsupported => crate::MicPermission::Unknown,
    }
}

/// Every cpal input on the default host, after the permission grant (which
/// is what lets macOS and iOS name their inputs). Ids come from
/// [`crate::devices::ids_for_names`].
pub(crate) async fn devices() -> Result<Vec<MicDevice>, MicError> {
    request_permission().await?;
    Ok(named_inputs(&cpal::default_host())?
        .into_iter()
        .map(|(id, name, _)| MicDevice { id, label: name })
        .collect())
}

/// `(id, name, device)` for every input that reports a name, in cpal's order.
/// An input that can't report its name can't be told apart, so it is skipped.
fn named_inputs(host: &cpal::Host) -> Result<Vec<(String, String, cpal::Device)>, MicError> {
    let inputs: Vec<(String, cpal::Device)> = host
        .input_devices()
        .map_err(|e| MicError::Backend(format!("input_devices: {e}")))?
        .filter_map(|d| d.name().ok().map(|n| (n, d)))
        .collect();
    let names: Vec<String> = inputs.iter().map(|(n, _)| n.clone()).collect();
    let ids = crate::devices::ids_for_names(&names);
    Ok(ids.into_iter().zip(inputs).map(|(id, (name, d))| (id, name, d)).collect())
}

pub(crate) async fn open(
    config: AudioStreamConfig,
    callback: BoxedCallback,
) -> Result<StreamHandle, MicError> {
    // iOS needs a granted permission and an active record session before
    // the input AudioUnit will produce anything but silence. The grant is
    // delegated to the shared `permissions` SDK; the AVAudioSession
    // *activation* (a capture requirement) stays local.
    #[cfg(target_os = "ios")]
    let session = {
        request_permission().await?;
        ios_session::activate()?
    };

    let host = cpal::default_host();
    let device = match &config.device {
        Some(id) => named_inputs(&host)?
            .into_iter()
            .find(|(d_id, _, _)| d_id == id)
            .map(|(_, _, d)| d)
            .ok_or(MicError::NoInputDevice)?,
        None => host.default_input_device().ok_or(MicError::NoInputDevice)?,
    };

    let supported = device
        .default_input_config()
        .map_err(|e| MicError::Backend(format!("default_input_config: {e}")))?;
    let sample_format = supported.sample_format();

    // The device runs at its own format; a requested rate or channel count
    // is produced by converting what it captures (see `crate::convert` for
    // why the device is never asked for it).
    let stream_config: cpal::StreamConfig = supported.config();
    let plan = plan(stream_config.sample_rate.0, stream_config.channels, &config)?;

    // Dispatch on the device's native sample format, converting each
    // format to normalized f32 in the per-type helper. Only one arm runs,
    // so each may move `callback`.
    let stream = match sample_format {
        SampleFormat::F32 => build::<f32>(&device, &stream_config, &plan, callback),
        SampleFormat::I16 => build::<i16>(&device, &stream_config, &plan, callback),
        SampleFormat::U16 => build::<u16>(&device, &stream_config, &plan, callback),
        SampleFormat::I32 => build::<i32>(&device, &stream_config, &plan, callback),
        SampleFormat::I8 => build::<i8>(&device, &stream_config, &plan, callback),
        SampleFormat::U8 => build::<u8>(&device, &stream_config, &plan, callback),
        other => {
            return Err(MicError::UnsupportedConfig(format!(
                "device sample format {other:?} not handled"
            )))
        }
    }?;

    stream
        .play()
        .map_err(|e| MicError::Backend(format!("stream play: {e}")))?;

    Ok(StreamHandle {
        _stream: stream,
        #[cfg(target_os = "ios")]
        _session: session,
    })
}

/// The device's format and what the caller gets: the request where it
/// names a value, the device's own where it does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Plan {
    device_rate: u32,
    device_channels: u16,
    rate: u32,
    channels: u16,
}

fn plan(device_rate: u32, device_channels: u16, config: &AudioStreamConfig) -> Result<Plan, MicError> {
    let rate = config.sample_rate.unwrap_or(device_rate);
    let channels = config.channels.unwrap_or(device_channels);
    if rate == 0 || channels == 0 {
        return Err(MicError::UnsupportedConfig(format!("{rate} Hz, {channels} channel(s)")));
    }
    Ok(Plan { device_rate, device_channels, rate, channels })
}

/// Build a cpal input stream for native sample type `T`, converting every
/// sample to normalized f32 — and, when the plan asks for another rate or
/// channel count, to that — before handing the chunk to `callback`.
/// Reusable buffers hold the converted frames so steady-state capture
/// doesn't allocate per chunk.
fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    plan: &Plan,
    mut callback: BoxedCallback,
) -> Result<cpal::Stream, MicError>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let mut scratch: Vec<f32> = Vec::new();
    let mut converter =
        crate::convert::Converter::new(plan.device_rate, plan.device_channels, plan.rate, plan.channels);
    let (sample_rate, channels) = (plan.rate, plan.channels);
    device
        .build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                scratch.clear();
                scratch.extend(data.iter().map(|&s| f32::from_sample(s)));
                let samples = match &mut converter {
                    Some(c) => c.process(&scratch),
                    None => &scratch,
                };
                // The resampler's first chunk can be all filter priming.
                if samples.is_empty() {
                    return;
                }
                callback(&AudioBuffer { samples, sample_rate, channels });
            },
            |err| {
                // A stream error (device unplugged, xrun) isn't fatal to
                // the process; surface it for diagnostics and let the
                // stream wind down.
                eprintln!("microphone: input stream error: {err}");
            },
            None,
        )
        .map_err(|e| match e {
            cpal::BuildStreamError::StreamConfigNotSupported
            | cpal::BuildStreamError::InvalidArgument => {
                MicError::UnsupportedConfig(format!("{e}"))
            }
            cpal::BuildStreamError::DeviceNotAvailable => MicError::NoInputDevice,
            other => MicError::Backend(format!("build_input_stream: {other}")),
        })
}

// ---------------------------------------------------------------------------
// iOS: AVAudioSession activation + record-permission request.
//
// cpal opens the input AudioUnit but does not touch AVAudioSession. iOS
// requires an audio session in a record-capable category, made active,
// with a granted record permission — otherwise the unit yields silence.
// ---------------------------------------------------------------------------

#[cfg(target_os = "ios")]
mod ios_session {
    use crate::MicError;
    use objc2::runtime::{AnyObject, Bool};
    use objc2::{class, msg_send};
    use objc2_foundation::NSString;
    use std::ptr;

    /// Held by `StreamHandle`; on drop it deactivates the audio session
    /// so we don't keep the app's session pinned in a record category
    /// after capture ends.
    pub(crate) struct SessionGuard;

    impl Drop for SessionGuard {
        fn drop(&mut self) {
            unsafe {
                let session = shared_instance();
                // setActive:NO error:nil — best effort; ignore failure.
                let _: Bool = msg_send![
                    session,
                    setActive: Bool::NO,
                    error: ptr::null_mut::<*mut AnyObject>(),
                ];
            }
        }
    }

    /// `[AVAudioSession sharedInstance]`. A process-wide singleton that
    /// lives forever, so we message the raw pointer without retaining.
    unsafe fn shared_instance() -> *mut AnyObject {
        let cls = class!(AVAudioSession);
        msg_send![cls, sharedInstance]
    }

    /// Set the record category and activate the session.
    pub(crate) fn activate() -> Result<SessionGuard, MicError> {
        unsafe {
            let session = shared_instance();
            // The category constant's string value equals its name, so we
            // construct it directly rather than linking the extern symbol
            // (`AVAudioSessionCategoryPlayAndRecord`). Play-and-record so
            // an app that also plays audio isn't forced to reconfigure.
            let category = NSString::from_str("AVAudioSessionCategoryPlayAndRecord");
            let ok: Bool = msg_send![
                session,
                setCategory: &*category,
                error: ptr::null_mut::<*mut AnyObject>(),
            ];
            if !ok.as_bool() {
                return Err(MicError::Backend(
                    "AVAudioSession setCategory failed".into(),
                ));
            }
            let ok: Bool = msg_send![
                session,
                setActive: Bool::YES,
                error: ptr::null_mut::<*mut AnyObject>(),
            ];
            if !ok.as_bool() {
                return Err(MicError::Backend("AVAudioSession setActive failed".into()));
            }
            Ok(SessionGuard)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: `open` passed a requested rate to cpal, which asked the
    /// device for it. On a 1st-gen iPad Pro a 16 kHz request deadlocked
    /// `AudioUnitStart` against the RemoteIO thread (`0x8BADF00D`). The
    /// device is now always opened at its own format and the SDK converts.
    /// The deadlock itself needs the hardware's timing to reproduce, so
    /// this pins the decision that avoids it.
    #[test]
    fn regression_a_requested_rate_is_converted_never_asked_of_the_device() {
        let config = AudioStreamConfig::default().with_sample_rate(16_000).mono();
        let p = plan(48_000, 2, &config).unwrap();
        assert_eq!((p.device_rate, p.device_channels), (48_000, 2), "the device keeps its own format");
        assert_eq!((p.rate, p.channels), (16_000, 1), "the caller gets what it asked for");
    }

    #[test]
    fn an_unset_request_is_the_device_format() {
        let p = plan(44_100, 1, &AudioStreamConfig::default()).unwrap();
        assert_eq!((p.rate, p.channels), (44_100, 1));
    }

    #[test]
    fn a_zero_rate_or_channel_count_is_unsupported() {
        let zero_rate = AudioStreamConfig::default().with_sample_rate(0);
        assert!(matches!(plan(48_000, 1, &zero_rate), Err(MicError::UnsupportedConfig(_))));
        let zero_channels = AudioStreamConfig::default().with_channels(0);
        assert!(matches!(plan(48_000, 1, &zero_channels), Err(MicError::UnsupportedConfig(_))));
    }
}
