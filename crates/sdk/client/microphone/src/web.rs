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
//!
//! Every browser call goes through web-glue, the framework-owned JS boundary
//! (docs/proposals/own-web-bindings.md). The capture stream is published as
//! the `AudioStream`'s `native_source` as a `web_glue::dom::MediaStream` —
//! THE capture stream, not a copy (see [`StreamHandle::native_source`]).

use std::rc::Rc;

use web_glue::dom::{MediaStream, MediaStreamTrack};
use web_glue::js::Object;
use web_glue::{Closure, JsCast, JsError, JsFuture, JsValue};

use crate::{AudioBuffer, AudioStreamConfig, BoxedCallback, MicError};

/// Number of frames per `onaudioprocess` block. 4096 ≈ 85 ms at 48 kHz —
/// a balance between callback overhead and latency. Must be a power of two
/// in `[256, 16384]` per the Web Audio spec.
const SCRIPT_PROCESSOR_BUFFER: u32 = 4096;

/// Bit layout of [`js_in_shape`]: channels above, frames below.
const SHAPE_CHANNEL_SHIFT: f64 = 1_048_576.0;

web_glue::import! {
    // `navigator.mediaDevices`, 0 when absent (insecure context, old engine).
    fn js_media_devices() -> u32 =
        "() => { const n = typeof navigator === 'undefined' ? null : navigator; \
           const m = n == null ? null : n.mediaDevices; return m == null ? 0 : G.add(m); }";
    // `mediaDevices.getUserMedia(constraints)` → its Promise.
    #[catch]
    fn js_get_user_media(md: u32, c: u32) -> u32 = "(m, c) => G.add(G.get(m).getUserMedia(G.get(c)))";
    // `navigator.permissions.query({ name: 'microphone' })` → its Promise, 0
    // when the Permissions API is absent.
    #[catch]
    fn js_query_mic_permission() -> u32 =
        "() => { const p = typeof navigator === 'undefined' ? null : navigator.permissions; \
           return p == null ? 0 : G.add(p.query({ name: 'microphone' })); }";
    // `new AudioContext()`, or at `sampleRate` when `rate > 0`.
    #[catch]
    fn js_ctx_new(rate: f64) -> u32 =
        "(r) => G.add(r > 0 ? new AudioContext({ sampleRate: r }) : new AudioContext())";
    fn js_ctx_rate(c: u32) -> f64 = "(c) => G.get(c).sampleRate";
    // The capture graph `MediaStreamSource(s) → ScriptProcessor → destination`
    // as `{ source, processor }`, `onaudioprocess = f`. A ScriptProcessorNode
    // only fires while connected to the destination, even though we don't
    // want to hear the input; the output channels we never write stay
    // silent, so nothing is played back.
    #[catch]
    fn js_graph(c: u32, s: u32, buf: u32, ch: u32, f: u32) -> u32 =
        "(c, s, buf, ch, f) => { const ctx = G.get(c); \
           const source = ctx.createMediaStreamSource(G.get(s)); \
           const processor = ctx.createScriptProcessor(buf, ch, ch); \
           processor.onaudioprocess = G.get(f); \
           source.connect(processor); processor.connect(ctx.destination); \
           return G.add({ source, processor }); }";
    // Teardown: the handler first (no late call into a dropped closure),
    // then the nodes, then the context (`close()` is fire-and-forget).
    fn js_teardown(g: u32, c: u32) =
        "(g, c) => { const x = G.get(g); x.processor.onaudioprocess = null; \
           try { x.processor.disconnect(); } catch (_) {} \
           try { x.source.disconnect(); } catch (_) {} \
           const p = G.get(c).close(); if (p) p.catch(() => {}); }";
    // An `AudioProcessingEvent`'s input buffer shape: channels * 2^20 +
    // frames (frames < 2^20, channels ≤ 32 per the Web Audio spec), 0 when
    // unreadable.
    fn js_in_shape(e: u32) -> f64 =
        "(e) => { const b = G.get(e).inputBuffer; return b == null ? 0 : \
           b.numberOfChannels * 1048576 + b.length; }";
    // Interleave the input channels ([L0,R0,L1,R1,…]; mono is a straight
    // copy) into the channels*frames f32s at `out` (4-byte aligned).
    fn js_in_interleave(e: u32, out: usize, ch: u32, n: u32) =
        "(e, o, ch, n) => { const b = G.get(e).inputBuffer; \
           const f = new Float32Array(G.u8().buffer, o >>> 0, ch * n); \
           if (ch === 1) { f.set(b.getChannelData(0)); return; } \
           for (let c = 0; c < ch; c++) { const d = b.getChannelData(c); \
             for (let i = 0; i < n; i++) f[i * ch + c] = d[i]; } }";
}

/// Keeps the capture graph and its `onaudioprocess` closure alive. Drop
/// tears the graph down and stops the underlying media tracks.
pub(crate) struct StreamHandle {
    /// The `AudioContext`.
    context: JsValue,
    /// `{ source, processor }` (see [`js_graph`]).
    graph: JsValue,
    stream: MediaStream,
    // Owns the JS callback for the node's lifetime; dropped after `Drop`
    // detached it.
    _on_audio: Closure,
}

impl StreamHandle {
    /// Publish the live capture `MediaStream` (audio track) as the
    /// [`AudioStream`](media_stream::AudioStream)'s native source, so a
    /// same-platform consumer — the `media-writer` `MediaRecorder` path, a
    /// future `<audio>` playback layer — binds the browser's own audio
    /// pipeline instead of reconstructing it from raw PCM.
    ///
    /// The handle clone is the SAME JS stream, so stopping the capture ends
    /// the consumer's tracks too. (The web-sys port published
    /// `Rc::new(self.stream.clone())` — web-sys's inherent `clone()` is the
    /// JS `MediaStream.clone()`, a new stream with cloned tracks that
    /// stopping the microphone left running.)
    pub(crate) fn native_source(&self) -> Option<crate::NativeSource> {
        Some(Rc::new(self.stream.clone()))
    }
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        // Detach the node graph and stop every track so the browser's
        // recording indicator clears and the mic is released.
        // SAFETY: live handles.
        unsafe { js_teardown(self.graph.raw(), self.context.raw()) };
        stop_tracks(&self.stream);
    }
}

fn stop_tracks(stream: &MediaStream) {
    for track in stream.get_tracks().iter() {
        track.unchecked_into::<MediaStreamTrack>().stop();
    }
}

pub(crate) async fn request_permission() -> Result<(), MicError> {
    // Acquire a stream purely to surface the prompt, then immediately stop
    // its tracks. A granted prompt is cached by the browser, so the later
    // `open()` won't prompt again.
    let stream = get_user_media(&AudioStreamConfig::default()).await?;
    stop_tracks(&stream);
    Ok(())
}

/// `navigator.permissions.query({name:"microphone"})` — the passive status read
/// (no `getUserMedia`, no prompt). Support is uneven (older Firefox lacks the
/// `microphone` descriptor), so any failure degrades to
/// [`MicPermission::Unknown`](crate::MicPermission::Unknown).
pub(crate) async fn permission_status() -> crate::MicPermission {
    // SAFETY: the result is 0 or a fresh Promise handle.
    let promise = match unsafe { js_query_mic_permission() } {
        Ok(0) | Err(_) => return crate::MicPermission::Unknown,
        Ok(p) => unsafe { JsValue::from_raw(p) },
    };
    let Ok(status) = JsFuture::new(&promise).await else {
        return crate::MicPermission::Unknown;
    };
    match status.get("state").ok().and_then(|s| s.as_string()).as_deref() {
        Some("granted") => crate::MicPermission::Granted,
        Some("denied") => crate::MicPermission::Denied,
        Some("prompt") => crate::MicPermission::Undetermined,
        _ => crate::MicPermission::Unknown,
    }
}

pub(crate) async fn open(
    config: AudioStreamConfig,
    callback: BoxedCallback,
) -> Result<StreamHandle, MicError> {
    let stream = get_user_media(&config).await?;

    // An AudioContext at the requested rate if any; the browser may still
    // clamp it, so the rate we read off the context is authoritative.
    let rate = config.sample_rate.map_or(0.0, |sr| sr as f64);
    // SAFETY: the result is a fresh context handle.
    let context = unsafe { js_ctx_new(rate) }
        .map(|c| unsafe { JsValue::from_raw(c) })
        .map_err(|e| MicError::Backend(format!("AudioContext: {}", err_string(&e))))?;

    let channels = config.channels.unwrap_or(1).max(1);
    // SAFETY: a live context handle.
    let sample_rate = unsafe { js_ctx_rate(context.raw()) } as u32;
    let mut callback = callback;
    let mut scratch: Vec<f32> = Vec::new();

    let on_audio = Closure::new(move |event: JsValue| {
        // SAFETY: the `AudioProcessingEvent` handed to this listener.
        let shape = unsafe { js_in_shape(event.raw()) };
        let n_channels = (shape / SHAPE_CHANNEL_SHIFT).floor() as u32;
        let frames = (shape % SHAPE_CHANNEL_SHIFT) as usize;
        if n_channels == 0 || frames == 0 {
            return;
        }
        scratch.clear();
        scratch.resize(frames * n_channels as usize, 0.0);
        // SAFETY: `scratch` holds exactly channels * frames f32s.
        unsafe { js_in_interleave(event.raw(), scratch.as_mut_ptr() as usize, n_channels, frames as u32) };
        let buffer = AudioBuffer {
            samples: &scratch,
            sample_rate,
            channels: n_channels as u16,
        };
        callback(&buffer);
    });

    // SAFETY: live handles; the result is a fresh graph handle.
    let graph = unsafe {
        js_graph(
            context.raw(),
            stream.as_js().raw(),
            SCRIPT_PROCESSOR_BUFFER,
            channels as u32,
            on_audio.as_js().raw(),
        )
    }
    .map(|g| unsafe { JsValue::from_raw(g) })
    .map_err(|e| MicError::Backend(format!("capture graph: {}", err_string(&e))))?;

    Ok(StreamHandle {
        context,
        graph,
        stream,
        _on_audio: on_audio,
    })
}

/// Run `getUserMedia({ audio: <constraints> })` and await the resulting
/// `MediaStream`. Maps a rejected promise to the closest [`MicError`].
async fn get_user_media(config: &AudioStreamConfig) -> Result<MediaStream, MicError> {
    // SAFETY: the result is 0 or a fresh handle.
    let devices = match unsafe { js_media_devices() } {
        0 => return Err(MicError::Unsupported),
        h => unsafe { JsValue::from_raw(h) },
    };

    let constraints = Object::new();
    let _ = constraints.set("audio", &audio_constraint(config));

    // SAFETY: live handles; the result is a fresh Promise handle.
    let promise = unsafe { js_get_user_media(devices.raw(), constraints.as_js().raw()) }
        .map(|p| unsafe { JsValue::from_raw(p) })
        .map_err(|e| MicError::Backend(format!("getUserMedia: {}", err_string(&e))))?;

    let value = JsFuture::new(&promise).await.map_err(map_gum_error)?;
    value
        .dyn_into::<MediaStream>()
        .map_err(|_| MicError::Backend("getUserMedia did not return a MediaStream".into()))
}

/// Build the `audio` member of the constraints. `true` for device defaults,
/// or an object carrying explicit `sampleRate` / `channelCount` preferences and
/// the browser audio-processing flags (`noiseSuppression`, `echoCancellation`,
/// `autoGainControl`) — each emitted only when the caller set it, so an unset
/// field leaves the browser default (all three default to `true`).
fn audio_constraint(config: &AudioStreamConfig) -> JsValue {
    if config.sample_rate.is_none()
        && config.channels.is_none()
        && config.noise_suppression.is_none()
        && config.echo_cancellation.is_none()
        && config.auto_gain_control.is_none()
    {
        return JsValue::from_bool(true);
    }
    let obj = Object::new();
    if let Some(sr) = config.sample_rate {
        let _ = obj.set("sampleRate", &JsValue::from_f64(sr as f64));
    }
    if let Some(ch) = config.channels {
        let _ = obj.set("channelCount", &JsValue::from_f64(ch as f64));
    }
    if let Some(on) = config.noise_suppression {
        let _ = obj.set("noiseSuppression", &JsValue::from_bool(on));
    }
    if let Some(on) = config.echo_cancellation {
        let _ = obj.set("echoCancellation", &JsValue::from_bool(on));
    }
    if let Some(on) = config.auto_gain_control {
        let _ = obj.set("autoGainControl", &JsValue::from_bool(on));
    }
    obj.into()
}

/// Map a rejected `getUserMedia` to a [`MicError`]. The DOMException name
/// distinguishes a user/policy denial from no device / device busy.
fn map_gum_error(err: JsError) -> MicError {
    let name = err.get("name").ok().and_then(|v| v.as_string()).unwrap_or_default();
    match name.as_str() {
        "NotAllowedError" | "SecurityError" | "PermissionDeniedError" => MicError::PermissionDenied,
        "NotFoundError" | "OverconstrainedError" => MicError::NoInputDevice,
        _ => MicError::Backend(format!("getUserMedia rejected: {}", err_string(&err))),
    }
}

fn err_string(value: &JsValue) -> String {
    value
        .as_string()
        .or_else(|| value.get("message").ok().and_then(|v| v.as_string()))
        .unwrap_or_else(|| format!("{value:?}"))
}
