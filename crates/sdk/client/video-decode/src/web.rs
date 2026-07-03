//! Web (wasm32) file-decode backend — a hidden `<video>` + offscreen `<canvas>`
//! frame pump and a WebAudio PCM tap.
//!
//! The design mirrors the two outputs the SDK produces, the same split as the
//! Apple backend:
//!
//! - **Video frames** — a hidden `HtmlVideoElement` drives decode + the clock.
//!   Each animation frame (a [`runtime_core::scheduling::raf_loop`], the same
//!   pump the Apple backend uses) we `drawImage` the video's *current* frame
//!   into a reused offscreen `HtmlCanvasElement`, sized to the (optionally
//!   `max_dimension`-downscaled, aspect-preserving) target, then `getImageData`
//!   hands us straight (non-premultiplied) tightly-packed `RGBA8` — exactly the
//!   SDK's frame format — which we push through the [`FrameWriter`]. This is the
//!   same `<video>`+`<canvas>` readback the `camera` web backend uses for a live
//!   feed; here the source is a clip URL instead of a `getUserMedia` stream.
//!   The element is appended to the document offscreen + invisible (not removed,
//!   so the browser keeps decoding it) rather than shown in an overlay — its
//!   pixels go to the canvas scene, not a player view.
//! - **Audio PCM** — an `AudioContext` `createMediaElementSource(video)` routes
//!   the element's audio through a `ScriptProcessorNode` whose `onaudioprocess`
//!   interleaves the input channels into one `f32` buffer and pushes it through
//!   the [`AudioWriter`] for the recorder's mux. Routing audio through WebAudio
//!   *replaces* the element's normal output, so the processor is connected on to
//!   the context destination — otherwise playback would go silent.
//!
//! Web is single-threaded; [`FrameWriter`] / [`AudioWriter`] are `!Send` on
//! wasm (the crate handles that). The rAF loop handle, the WebAudio nodes, the
//! `onaudioprocess` [`Closure`], and the `<video>` element all live in the
//! [`StreamHandle`] so nothing is dropped early; its `Drop` pauses the video,
//! disconnects the nodes, removes the element, and stops the pump.

use std::cell::Cell;
use std::rc::Rc;

use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{
    AudioContext, AudioProcessingEvent, CanvasRenderingContext2d, HtmlCanvasElement,
    HtmlVideoElement, MediaElementAudioSourceNode, ScriptProcessorNode,
};

use media_stream::{AudioWriter, FrameWriter};

use crate::{DecodeConfig, DecodeSource, Opened, TransportControl, VideoDecodeError};

/// ScriptProcessor buffer size — 4096 frames is the common, low-overhead choice
/// for a non-latency-critical recording tap.
const SCRIPT_PROCESSOR_BUFFER: u32 = 4096;

/// Whether to route the clip's audio through WebAudio to tap PCM for the
/// recording mux. OFF for now: `createMediaElementSource` + a suspended
/// `AudioContext` stalls `<video>` playback. With it off the clip plays natively
/// (audible); only capturing that audio into a recording is deferred. Mirrors
/// the macOS `apple.rs` gate.
const ENABLE_AUDIO_TAP: bool = false;

// ===========================================================================
// Transport — drives the <video> element.
// ===========================================================================

/// Per-platform playback control over the hidden `<video>`. The `muted` cell
/// shadows the element so [`is_muted`](TransportControl::is_muted) is a cheap
/// read (the element's muted state would otherwise need a JS round-trip and is
/// the player's concern, distinct from the recorder's PCM tap).
struct WebTransport {
    video: HtmlVideoElement,
    muted: Cell<bool>,
    /// Latest-wins, one-in-flight scrub coordination (shared with the `seeked`
    /// handler).
    seek_state: Rc<SeekState>,
}

/// Scrub coordination: a fast drag records `target` every tick (cheap, keeps the
/// slider responsive), but only ONE `set_current_time` is outstanding at a time.
/// When it completes (`seeked`), the newest pending target — if any — is issued
/// and the intermediate ones are dropped, so decodes never backlog on a large
/// clip and the picture catches up as fast as it can.
struct SeekState {
    /// `(seconds, exact)` — `exact=false` is a live-scrub preview (use `fastSeek`
    /// where available); `exact=true` decodes the precise frame (drag landing).
    target: Cell<Option<(f64, bool)>>,
    seeking: Cell<bool>,
    /// Latched once `fastSeek` is found unsupported (Chrome) — fall back to exact
    /// `currentTime` thereafter.
    no_fast_seek: Cell<bool>,
}

/// Issue the pending target iff no seek is in flight (latest-wins). Skips a
/// target that's already ~current (it would produce no `seeked` and wedge the
/// in-flight flag). A non-exact target prefers `fastSeek` (fast, approximate) for
/// smooth scrubbing; exact targets — and any browser without `fastSeek` — decode
/// the precise frame via `currentTime`.
fn pump_seek(video: &HtmlVideoElement, state: &SeekState) {
    if state.seeking.get() {
        return;
    }
    let Some((t, exact)) = state.target.take() else { return };
    if (t - video.current_time()).abs() < 0.01 {
        return;
    }
    state.seeking.set(true);
    if exact || state.no_fast_seek.get() {
        video.set_current_time(t);
    } else if video.fast_seek(t).is_err() {
        // fastSeek unsupported here (e.g. Chrome) — latch + use exact from now on.
        state.no_fast_seek.set(true);
        video.set_current_time(t);
    }
}

impl TransportControl for WebTransport {
    fn play(&self) {
        // play() returns a Promise we don't await; browsers may reject autoplay
        // without a user gesture, but our calls originate from a click.
        let _ = self.video.play();
    }
    fn pause(&self) {
        let _ = self.video.pause();
    }
    fn seek(&self, seconds: f32) {
        // Exact landing (drag end): decode the precise frame.
        self.seek_state.target.set(Some((seconds.max(0.0) as f64, true)));
        pump_seek(&self.video, &self.seek_state);
    }
    fn seek_preview(&self, seconds: f32) {
        // Live scrub: record the target (cheap) + issue only if nothing's in
        // flight; the `seeked` handler issues the newest pending target when the
        // current lands. Prefers `fastSeek` so frames flow while dragging.
        self.seek_state.target.set(Some((seconds.max(0.0) as f64, false)));
        pump_seek(&self.video, &self.seek_state);
    }
    fn set_muted(&self, muted: bool) {
        self.muted.set(muted);
        self.video.set_muted(muted);
    }
    fn set_rate(&self, rate: f32) {
        self.video.set_playback_rate(rate.max(0.0) as f64);
    }
    fn position(&self) -> f32 {
        let t = self.video.current_time();
        if t.is_finite() {
            t as f32
        } else {
            0.0
        }
    }
    fn duration(&self) -> f32 {
        // `duration` is NaN before metadata loads and +inf for live/unknown.
        let d = self.video.duration();
        if d.is_finite() {
            d as f32
        } else {
            0.0
        }
    }
    fn is_playing(&self) -> bool {
        !self.video.paused()
    }
    fn is_muted(&self) -> bool {
        self.muted.get()
    }
}

// ===========================================================================
// StreamHandle — keeps decode alive; Drop stops it.
// ===========================================================================

/// Holds everything decode needs alive. Dropping it pauses the video,
/// disconnects + tears down the WebAudio graph, removes the element from the
/// DOM, and stops the rAF pump (the [`RafLoop`](runtime_core::scheduling::RafLoop)
/// cancels on its own drop).
struct StreamHandle {
    video: HtmlVideoElement,
    _raf: runtime_core::scheduling::RafLoop,
    /// WebAudio tap, present only if the `AudioContext` built successfully. The
    /// `onaudioprocess` `Closure` is held here so it isn't dropped while the node
    /// still references it.
    audio: Option<AudioTap>,
    /// A `Blob` object URL created for a `Bytes` source; revoked on drop so the
    /// in-memory clip is freed.
    object_url: Option<String>,
    /// The `seeked` event handler, held so it stays valid while the element lives.
    _onseeked: Closure<dyn FnMut()>,
}

/// The WebAudio PCM-tap graph: `MediaElementSource → ScriptProcessor →
/// destination`. All three are retained for the tap's lifetime.
struct AudioTap {
    context: AudioContext,
    source: MediaElementAudioSourceNode,
    processor: ScriptProcessorNode,
    /// Kept alive so the node's `onaudioprocess` callback stays valid.
    _on_process: Closure<dyn FnMut(AudioProcessingEvent)>,
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        let _ = self.video.pause();
        // Detach the seek handler before teardown so it can't fire mid-drop.
        self.video.set_onseeked(None);
        // Stop pulling from the element + drop the canvas src.
        self.video.set_src("");
        if let Some(audio) = self.audio.take() {
            // Detach the callback first so it can't fire mid-teardown, then
            // disconnect the graph and close the context.
            audio.processor.set_onaudioprocess(None);
            let _ = audio.processor.disconnect();
            let _ = audio.source.disconnect();
            let _ = audio.context.close();
        }
        // Remove the offscreen element from the DOM.
        if let Some(parent) = self.video.parent_node() {
            let _ = parent.remove_child(&self.video);
        }
        // Free the in-memory clip blob, if any.
        if let Some(u) = &self.object_url {
            let _ = web_sys::Url::revoke_object_url(u);
        }
        // `_raf` cancels the pump on its own drop.
    }
}

// ===========================================================================
// Open.
// ===========================================================================

pub(crate) async fn open(
    source: DecodeSource,
    config: DecodeConfig,
    frames: FrameWriter,
    audio: AudioWriter,
) -> Result<Opened, VideoDecodeError> {
    // Resolve to a URL the <video> can load. `Bytes` (the web file-picker hands
    // back a `Blob` with no path) becomes an in-memory `Blob` object URL, revoked
    // on teardown.
    let (url, object_url) = match source {
        DecodeSource::Url(u) => (u, None),
        DecodeSource::Bytes(data) => {
            let arr = js_sys::Uint8Array::from(data.as_slice());
            let parts = js_sys::Array::new();
            parts.push(&arr);
            let opts = web_sys::BlobPropertyBag::new();
            opts.set_type("video/mp4");
            let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts)
                .map_err(|e| VideoDecodeError::Backend(format!("blob: {}", err_string(&e))))?;
            let obj = web_sys::Url::create_object_url_with_blob(&blob)
                .map_err(|e| VideoDecodeError::Backend(format!("object url: {}", err_string(&e))))?;
            (obj.clone(), Some(obj))
        }
    };

    let document = web_sys::window()
        .and_then(|w| w.document())
        .ok_or(VideoDecodeError::Unsupported)?;

    // Hidden <video> that drives decode + the clock.
    let video: HtmlVideoElement = document
        .create_element("video")
        .map_err(|e| VideoDecodeError::Backend(format!("create video: {}", err_string(&e))))?
        .dyn_into()
        .map_err(|_| VideoDecodeError::Backend("element is not a video".into()))?;

    video.set_muted(config.muted);
    video.set_loop(config.loop_playback);
    video.set_cross_origin(Some("anonymous"));
    video.set_preload("auto");
    let _ = video.set_attribute("playsinline", "");
    // Offscreen but with a real (tiny) size and NOT `visibility:hidden` /
    // zero-size: browsers throttle or refuse playback of hidden / 0×0 / display:none
    // media, which freezes `currentTime` (play() appears dead). `opacity:0` +
    // a 2px box pinned offscreen keeps it decoding AND advancing while invisible.
    let _ = video.set_attribute(
        "style",
        "position:fixed;left:0;top:0;width:2px;height:2px;opacity:0;pointer-events:none;z-index:-1;",
    );
    // src triggers the load; setting it after the attributes keeps `muted` /
    // `loop` honored from frame zero.
    video.set_src(&url);

    if let Some(body) = document.body() {
        let _ = body.append_child(&video);
    }

    // Autoplay (browsers require muted for unprompted autoplay; if the caller
    // asked for autoplay && !muted we still attempt play() — it may be rejected,
    // which is acceptable: the caller can re-`play()` from a user gesture).
    if config.autoplay {
        let _ = video.play();
    }

    // Offscreen canvas + 2d context, reused across pump ticks.
    let canvas: HtmlCanvasElement = document
        .create_element("canvas")
        .map_err(|e| VideoDecodeError::Backend(format!("create canvas: {}", err_string(&e))))?
        .dyn_into()
        .map_err(|_| VideoDecodeError::Backend("element is not a canvas".into()))?;
    let ctx: CanvasRenderingContext2d = canvas
        .get_context_with_context_options(
            "2d",
            // These 2D contexts read pixels back every frame via `get_image_data`;
            // `willReadFrequently` keeps the backing store CPU-side (avoids a per-
            // readback GPU→CPU stall) and silences the browser's "Multiple readback
            // operations" warning.
            &{
                let o = js_sys::Object::new();
                let _ = js_sys::Reflect::set(
                    &o,
                    &wasm_bindgen::JsValue::from_str("willReadFrequently"),
                    &wasm_bindgen::JsValue::TRUE,
                );
                wasm_bindgen::JsValue::from(o)
            },
        )
        .map_err(|e| VideoDecodeError::Backend(format!("get 2d context: {}", err_string(&e))))?
        .ok_or_else(|| VideoDecodeError::Backend("no 2d context".into()))?
        .dyn_into()
        .map_err(|_| VideoDecodeError::Backend("context is not 2d".into()))?;

    // Set by the `<video>`'s `seeked` event: a seek's decoded frame just became
    // available, so the pump must redraw even though currentTime is now steady at
    // the target. Without this, scrubbing a PAUSED video sets currentTime but the
    // landed frame never repaints (the picture lags / "hangs" then jumps).
    let redraw = Rc::new(Cell::new(false));
    let seek_state = Rc::new(SeekState {
        target: Cell::new(None),
        seeking: Cell::new(false),
        no_fast_seek: Cell::new(false),
    });
    let onseeked = {
        let redraw = redraw.clone();
        let video_cb = video.clone();
        let seek_state_cb = seek_state.clone();
        // A seek completed: draw the landed frame, mark no seek in flight, and
        // issue the newest pending target (if the user kept dragging) — dropping
        // the intermediate ones. This is what keeps scrubbing backlog-free.
        let cb = Closure::<dyn FnMut()>::new(move || {
            seek_state_cb.seeking.set(false);
            redraw.set(true);
            pump_seek(&video_cb, &seek_state_cb);
        });
        video.set_onseeked(Some(cb.as_ref().unchecked_ref()));
        cb
    };

    // Frame pump: each display tick, draw + read back the current frame as RGBA8.
    let raf = {
        let video = video.clone();
        let max_dim = config.max_dimension;
        let redraw = redraw.clone();
        let mut last_t = -1.0_f64;
        let mut drew_once = false;
        runtime_core::scheduling::raf_loop(move || {
            // Only push a frame when there's actually a NEW one: while playing
            // (currentTime advances) or right after a seek. A paused, unchanged
            // frame is pushed once then skipped — so a paused video stops driving
            // repaints (the "rerenders constantly when paused" bug). The readback
            // is also a GPU→CPU stall, so gate it on a real consumer too.
            if !frames.wants_cpu_frames() {
                return;
            }
            let ready = video.ready_state() >= 2 && video.video_width() > 0;
            if !ready {
                return;
            }
            let t = video.current_time();
            let advancing = !video.paused();
            let changed = (t - last_t).abs() > 1e-4;
            // `redraw` (a completed seek) forces one draw even when t is steady.
            let seeked = redraw.replace(false);
            if advancing || changed || seeked || !drew_once {
                pump_frame(&video, &canvas, &ctx, &frames, max_dim);
                last_t = t;
                drew_once = true;
            }
        })
    };

    // Audio tap → PCM for the recorder. GATED OFF (see `ENABLE_AUDIO_TAP`):
    // `createMediaElementSource` reroutes the element's audio into an
    // `AudioContext` that starts suspended without a user-gesture resume, which
    // stalls `<video>` playback (so play/scrub appear dead). With it off the
    // element plays natively (its own sound); only capturing that audio INTO a
    // recording is deferred — mirrors the macOS `ENABLE_AUDIO_TAP` gate.
    let audio_tap = if ENABLE_AUDIO_TAP {
        install_audio_tap(&video, audio)
    } else {
        let _ = audio; // unused writer dropped → no audio stream advertised
        None
    };

    let control: Rc<dyn TransportControl> = Rc::new(WebTransport {
        video: video.clone(),
        muted: Cell::new(config.muted),
        seek_state: seek_state.clone(),
    });

    let has_audio = audio_tap.is_some();
    let handle = StreamHandle {
        video,
        _raf: raf,
        audio: audio_tap,
        object_url,
        _onseeked: onseeked,
    };

    Ok(Opened {
        handle: Box::new(handle),
        control,
        has_audio,
        // videoWidth isn't known until metadata loads; report None at open.
        natural_size: None,
    })
}

/// Draw the video's current frame into the canvas (downscaled per `max_dim`)
/// and push it back as tightly-packed `RGBA8`. A no-op until the video has
/// decoded a frame (`readyState >= HAVE_CURRENT_DATA` and non-zero dimensions).
fn pump_frame(
    video: &HtmlVideoElement,
    canvas: &HtmlCanvasElement,
    ctx: &CanvasRenderingContext2d,
    frames: &FrameWriter,
    max_dim: Option<u32>,
) {
    // `HAVE_CURRENT_DATA` == 2: there's a frame for the current playback position.
    if video.ready_state() < 2 {
        return;
    }
    let nat_w = video.video_width();
    let nat_h = video.video_height();
    if nat_w == 0 || nat_h == 0 {
        return;
    }
    let (w, h) = target_size(nat_w, nat_h, max_dim);
    if canvas.width() != w {
        canvas.set_width(w);
    }
    if canvas.height() != h {
        canvas.set_height(h);
    }
    // Scale into the (possibly smaller) canvas in one draw.
    if ctx
        .draw_image_with_html_video_element_and_dw_and_dh(video, 0.0, 0.0, w as f64, h as f64)
        .is_err()
    {
        return;
    }
    let image = match ctx.get_image_data(0.0, 0.0, w as f64, h as f64) {
        Ok(d) => d,
        Err(_) => return,
    };
    // `ImageData::data()` is straight (non-premultiplied) RGBA8, tightly packed.
    let bytes = image.data();
    frames.write_rgba8(w, h, &bytes.0);
}

/// Target decode size honoring `max_dim` (aspect-preserving). `(0,0)` natural
/// size never reaches here (the pump bails earlier).
fn target_size(nat_w: u32, nat_h: u32, max_dim: Option<u32>) -> (u32, u32) {
    match max_dim {
        Some(max) if nat_w.max(nat_h) > max && max > 0 => {
            let scale = max as f32 / nat_w.max(nat_h) as f32;
            (
                ((nat_w as f32 * scale) as u32).max(1),
                ((nat_h as f32 * scale) as u32).max(1),
            )
        }
        _ => (nat_w, nat_h),
    }
}

/// Build the `MediaElementSource → ScriptProcessor → destination` PCM tap.
///
/// `createMediaElementSource` *routes* the element's audio through WebAudio, so
/// the processor MUST connect on to the destination or playback goes silent. We
/// set `has_audio` optimistically: if the clip has no audio track the processor
/// just receives silence, which is acceptable (we can't cheaply pre-check tracks
/// on the web). Returns `None` only if the WebAudio graph itself fails to build.
fn install_audio_tap(video: &HtmlVideoElement, writer: AudioWriter) -> Option<AudioTap> {
    let context = AudioContext::new().ok()?;
    let source = context.create_media_element_source(video).ok()?;
    let processor = context
        .create_script_processor_with_buffer_size_and_number_of_input_channels_and_number_of_output_channels(
            SCRIPT_PROCESSOR_BUFFER,
            2,
            2,
        )
        .ok()?;

    let sample_rate = context.sample_rate() as u32;

    // Interleave each input channel into one frame-major f32 buffer and push it.
    let on_process = Closure::wrap(Box::new(move |event: AudioProcessingEvent| {
        let buffer = match event.input_buffer() {
            Ok(b) => b,
            Err(_) => return,
        };
        let channels = buffer.number_of_channels() as usize;
        let frames = buffer.length() as usize;
        if channels == 0 || frames == 0 {
            return;
        }
        // Gather per-channel data, then interleave [L0,R0,L1,R1,...].
        let mut planar: Vec<Vec<f32>> = Vec::with_capacity(channels);
        for c in 0..channels {
            match buffer.get_channel_data(c as u32) {
                Ok(data) => planar.push(data),
                Err(_) => return,
            }
        }
        let mut interleaved = vec![0.0f32; frames * channels];
        for (c, chan) in planar.iter().enumerate() {
            let n = chan.len().min(frames);
            for f in 0..n {
                interleaved[f * channels + c] = chan[f];
            }
        }
        writer.write_pcm_f32(sample_rate, channels as u16, &interleaved);
    }) as Box<dyn FnMut(AudioProcessingEvent)>);

    processor.set_onaudioprocess(Some(on_process.as_ref().unchecked_ref()));

    // source → processor → destination (keeps audio audible while we tap it).
    source.connect_with_audio_node(&processor).ok()?;
    processor
        .connect_with_audio_node(&context.destination())
        .ok()?;

    Some(AudioTap {
        context,
        source,
        processor,
        _on_process: on_process,
    })
}

/// Best-effort string from a `JsValue` (its `.message` or debug form).
fn err_string(value: &JsValue) -> String {
    value
        .as_string()
        .or_else(|| {
            js_sys::Reflect::get(value, &"message".into())
                .ok()
                .and_then(|v| v.as_string())
        })
        .unwrap_or_else(|| format!("{value:?}"))
}
