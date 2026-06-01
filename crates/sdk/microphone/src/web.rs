//! Web capture via `getUserMedia` + the Web Audio API.
//!
//! `getUserMedia({audio:true})` yields a `MediaStream` (and triggers the
//! browser's permission prompt). We feed it through a Web Audio graph —
//! `MediaStreamAudioSourceNode` → `ScriptProcessorNode` → destination —
//! and copy each `onaudioprocess` block out as normalized f32 frames.
//!
//! `ScriptProcessorNode` is deprecated in favour of `AudioWorklet`, but it
//! needs no separate worklet module to load, which keeps this SDK a single
//! self-contained crate. It's supported in every current browser. Moving
//! to an `AudioWorklet` is a transparent swap behind this same API if the
//! deprecation ever bites.

use js_sys::{Array, Reflect};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    AudioContext, AudioContextOptions, AudioProcessingEvent, MediaStream, MediaStreamConstraints,
    MediaStreamTrack, ScriptProcessorNode,
};

use crate::{AudioBuffer, AudioStreamConfig, BoxedCallback, MicError};

/// Number of frames per `onaudioprocess` block. 4096 ≈ 85 ms at 48 kHz —
/// a balance between callback overhead and latency. Must be a power of two
/// in `[256, 16384]` per the Web Audio spec.
const SCRIPT_PROCESSOR_BUFFER: u32 = 4096;

/// Keeps the capture graph and its `onaudioprocess` closure alive. Drop
/// tears the graph down and stops the underlying media tracks.
pub(crate) struct StreamHandle {
    context: AudioContext,
    source: web_sys::MediaStreamAudioSourceNode,
    processor: ScriptProcessorNode,
    stream: MediaStream,
    // Owns the JS callback for the node's lifetime; dropped last.
    _on_audio: Closure<dyn FnMut(AudioProcessingEvent)>,
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        // Detach the node graph and stop every track so the browser's
        // recording indicator clears and the mic is released.
        let _ = self.processor.disconnect();
        let _ = self.source.disconnect();
        self.processor.set_onaudioprocess(None);
        if let Ok(tracks) = stream_tracks(&self.stream) {
            for track in tracks.iter() {
                if let Ok(track) = track.dyn_into::<MediaStreamTrack>() {
                    track.stop();
                }
            }
        }
        // `close()` returns a Promise; we don't need to await teardown.
        let _ = self.context.close();
    }
}

pub(crate) async fn request_permission() -> Result<(), MicError> {
    // Acquire a stream purely to surface the prompt, then immediately stop
    // its tracks. A granted prompt is cached by the browser, so the later
    // `open()` won't prompt again.
    let stream = get_user_media(&AudioStreamConfig::default()).await?;
    if let Ok(tracks) = stream_tracks(&stream) {
        for track in tracks.iter() {
            if let Ok(track) = track.dyn_into::<MediaStreamTrack>() {
                track.stop();
            }
        }
    }
    Ok(())
}

pub(crate) async fn open(
    config: AudioStreamConfig,
    callback: BoxedCallback,
) -> Result<StreamHandle, MicError> {
    let stream = get_user_media(&config).await?;

    // An AudioContext at the requested rate if any; the browser may still
    // clamp it, so the rate we read off the context is authoritative.
    let context = match config.sample_rate {
        Some(sr) => {
            let opts = AudioContextOptions::new();
            opts.set_sample_rate(sr as f32);
            AudioContext::new_with_context_options(&opts)
        }
        None => AudioContext::new(),
    }
    .map_err(|e| MicError::Backend(format!("AudioContext: {}", err_string(&e))))?;

    let source = context
        .create_media_stream_source(&stream)
        .map_err(|e| MicError::Backend(format!("create_media_stream_source: {}", err_string(&e))))?;

    let channels = config.channels.unwrap_or(1).max(1);
    let processor = context
        .create_script_processor_with_buffer_size_and_number_of_input_channels_and_number_of_output_channels(
            SCRIPT_PROCESSOR_BUFFER,
            channels as u32,
            channels as u32,
        )
        .map_err(|e| MicError::Backend(format!("create_script_processor: {}", err_string(&e))))?;

    let sample_rate = context.sample_rate() as u32;
    let mut callback = callback;
    let mut scratch: Vec<f32> = Vec::new();

    let on_audio = Closure::wrap(Box::new(move |event: AudioProcessingEvent| {
        let in_buf = match event.input_buffer() {
            Ok(b) => b,
            Err(_) => return,
        };
        let n_channels = in_buf.number_of_channels();
        let frames = in_buf.length() as usize;
        if n_channels == 0 || frames == 0 {
            return;
        }

        if n_channels == 1 {
            // Mono fast path: hand the channel data straight through.
            if let Ok(data) = in_buf.get_channel_data(0) {
                let buffer = AudioBuffer {
                    samples: &data,
                    sample_rate,
                    channels: 1,
                };
                callback(&buffer);
            }
            return;
        }

        // Interleave planar channels into the scratch buffer.
        scratch.clear();
        scratch.resize(frames * n_channels as usize, 0.0);
        for ch in 0..n_channels {
            if let Ok(data) = in_buf.get_channel_data(ch) {
                for (frame, &sample) in data.iter().enumerate() {
                    scratch[frame * n_channels as usize + ch as usize] = sample;
                }
            }
        }
        let buffer = AudioBuffer {
            samples: &scratch,
            sample_rate,
            channels: n_channels as u16,
        };
        callback(&buffer);
    }) as Box<dyn FnMut(AudioProcessingEvent)>);

    processor.set_onaudioprocess(Some(on_audio.as_ref().unchecked_ref()));

    // A ScriptProcessorNode only fires while connected to the destination,
    // even though we don't want to hear the input. The output channels we
    // never write stay silent, so nothing is played back.
    source
        .connect_with_audio_node(&processor)
        .map_err(|e| MicError::Backend(format!("connect source: {}", err_string(&e))))?;
    processor
        .connect_with_audio_node(&context.destination())
        .map_err(|e| MicError::Backend(format!("connect dest: {}", err_string(&e))))?;

    Ok(StreamHandle {
        context,
        source,
        processor,
        stream,
        _on_audio: on_audio,
    })
}

/// Run `getUserMedia({ audio: <constraints> })` and await the resulting
/// `MediaStream`. Maps a rejected promise to the closest [`MicError`].
async fn get_user_media(config: &AudioStreamConfig) -> Result<MediaStream, MicError> {
    let window = web_sys::window().ok_or(MicError::Unsupported)?;
    let devices = window
        .navigator()
        .media_devices()
        .map_err(|_| MicError::Unsupported)?;

    let constraints = MediaStreamConstraints::new();
    constraints.set_audio(&audio_constraint(config));

    let promise = devices
        .get_user_media_with_constraints(&constraints)
        .map_err(|e| MicError::Backend(format!("getUserMedia: {}", err_string(&e))))?;

    let value = JsFuture::from(promise).await.map_err(map_gum_error)?;
    value
        .dyn_into::<MediaStream>()
        .map_err(|_| MicError::Backend("getUserMedia did not return a MediaStream".into()))
}

/// Build the `audio` member of the constraints. `true` for device
/// defaults, or an object carrying explicit `sampleRate` / `channelCount`
/// the browser treats as preferences.
fn audio_constraint(config: &AudioStreamConfig) -> JsValue {
    if config.sample_rate.is_none() && config.channels.is_none() {
        return JsValue::TRUE;
    }
    let obj = js_sys::Object::new();
    if let Some(sr) = config.sample_rate {
        let _ = Reflect::set(&obj, &"sampleRate".into(), &JsValue::from_f64(sr as f64));
    }
    if let Some(ch) = config.channels {
        let _ = Reflect::set(&obj, &"channelCount".into(), &JsValue::from_f64(ch as f64));
    }
    obj.into()
}

fn stream_tracks(stream: &MediaStream) -> Result<Array, MicError> {
    Ok(stream.get_tracks())
}

/// Map a rejected `getUserMedia` to a [`MicError`]. The DOMException name
/// distinguishes a user/policy denial from no device / device busy.
fn map_gum_error(err: JsValue) -> MicError {
    let name = Reflect::get(&err, &"name".into())
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default();
    match name.as_str() {
        "NotAllowedError" | "SecurityError" | "PermissionDeniedError" => MicError::PermissionDenied,
        "NotFoundError" | "OverconstrainedError" => MicError::NoInputDevice,
        _ => MicError::Backend(format!("getUserMedia rejected: {}", err_string(&err))),
    }
}

fn err_string(value: &JsValue) -> String {
    value
        .as_string()
        .or_else(|| {
            Reflect::get(value, &"message".into())
                .ok()
                .and_then(|v| v.as_string())
        })
        .unwrap_or_else(|| format!("{value:?}"))
}
