//! Native capture via [`cpal`], which wraps the platform audio stack:
//! CoreAudio on macOS/iOS, WASAPI on Windows, ALSA on Linux. One code
//! path covers all four; the only per-platform divergence is the iOS
//! `AVAudioSession` activation, isolated in [`ios_session`] below.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};

use crate::{AudioBuffer, AudioStreamConfig, BoxedCallback, MicError};

/// Keeps the capture alive. Dropping it stops the stream (cpal tears the
/// platform stream down in `Stream`'s `Drop`).
pub(crate) struct StreamHandle {
    // Field order matters for drop order on iOS: the cpal stream must be
    // torn down before we deactivate the audio session.
    _stream: cpal::Stream,
    #[cfg(target_os = "ios")]
    _session: ios_session::SessionGuard,
}

pub(crate) async fn request_permission() -> Result<(), MicError> {
    // Desktop (macOS/Windows/Linux): the OS either grants implicitly or
    // surfaces its own prompt the first time the input stream starts —
    // there's no portable pre-prompt API, so this is a no-op success.
    #[cfg(target_os = "ios")]
    {
        ios_session::request_record_permission().await
    }
    #[cfg(not(target_os = "ios"))]
    {
        Ok(())
    }
}

pub(crate) async fn open(
    config: AudioStreamConfig,
    callback: BoxedCallback,
) -> Result<StreamHandle, MicError> {
    // iOS needs a granted permission and an active record session before
    // the input AudioUnit will produce anything but silence.
    #[cfg(target_os = "ios")]
    let session = {
        ios_session::request_record_permission().await?;
        ios_session::activate()?
    };

    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or(MicError::NoInputDevice)?;

    let supported = device
        .default_input_config()
        .map_err(|e| MicError::Backend(format!("default_input_config: {e}")))?;
    let sample_format = supported.sample_format();

    // Start from the device default, then apply explicit requests. cpal
    // validates the result when it builds the stream — an unsupported
    // rate/channel surfaces as a build error we map to UnsupportedConfig.
    let mut stream_config: cpal::StreamConfig = supported.config();
    if let Some(sr) = config.sample_rate {
        stream_config.sample_rate = cpal::SampleRate(sr);
    }
    if let Some(ch) = config.channels {
        stream_config.channels = ch;
    }

    let sample_rate = stream_config.sample_rate.0;
    let channels = stream_config.channels;

    // Dispatch on the device's native sample format, converting each
    // format to normalized f32 in the per-type helper. Only one arm runs,
    // so each may move `callback`.
    let stream = match sample_format {
        SampleFormat::F32 => build::<f32>(&device, &stream_config, sample_rate, channels, callback),
        SampleFormat::I16 => build::<i16>(&device, &stream_config, sample_rate, channels, callback),
        SampleFormat::U16 => build::<u16>(&device, &stream_config, sample_rate, channels, callback),
        SampleFormat::I32 => build::<i32>(&device, &stream_config, sample_rate, channels, callback),
        SampleFormat::I8 => build::<i8>(&device, &stream_config, sample_rate, channels, callback),
        SampleFormat::U8 => build::<u8>(&device, &stream_config, sample_rate, channels, callback),
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

/// Build a cpal input stream for native sample type `T`, converting every
/// sample to normalized f32 before handing the chunk to `callback`. A
/// reusable scratch `Vec` holds the converted frames so steady-state
/// capture doesn't allocate per chunk.
fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_rate: u32,
    channels: u16,
    mut callback: BoxedCallback,
) -> Result<cpal::Stream, MicError>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let mut scratch: Vec<f32> = Vec::new();
    device
        .build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                scratch.clear();
                scratch.extend(data.iter().map(|&s| f32::from_sample(s)));
                let buffer = AudioBuffer {
                    samples: &scratch,
                    sample_rate,
                    channels,
                };
                callback(&buffer);
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
    use block2::RcBlock;
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

    /// Bridge `requestRecordPermission:`'s completion block to async via a
    /// oneshot channel.
    pub(crate) async fn request_record_permission() -> Result<(), MicError> {
        let (tx, rx) = futures_channel::oneshot::channel::<bool>();
        // The block is invoked once, on an arbitrary queue. Move the
        // sender in; `RcBlock` keeps it alive until the block fires.
        let tx = std::cell::Cell::new(Some(tx));
        let block = RcBlock::new(move |granted: Bool| {
            if let Some(tx) = tx.take() {
                let _ = tx.send(granted.as_bool());
            }
        });
        unsafe {
            let session = shared_instance();
            let _: () = msg_send![session, requestRecordPermission: &*block];
        }
        match rx.await {
            Ok(true) => Ok(()),
            Ok(false) => Err(MicError::PermissionDenied),
            // Sender dropped without firing — treat as denial.
            Err(_) => Err(MicError::PermissionDenied),
        }
    }
}
