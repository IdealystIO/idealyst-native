//! Web capture backend — `getDisplayMedia`.
//!
//! The browser shows its source picker (tab/window/screen); `Source::ThisApp`
//! adds the `preferCurrentTab` hint so the app's own tab is the default
//! choice. We play the resulting `MediaStream` into a hidden `<video>` and,
//! on a `setInterval` cadence at the configured fps, draw it into an
//! offscreen `<canvas>` and read back RGBA pixels for the frame callback.
//!
//! Why the canvas pump and not `MediaStreamTrackProcessor`/WebCodecs: the
//! canvas path is supported in every browser that has `getDisplayMedia`
//! and keeps the first working path simple. A WebCodecs `VideoFrame` fast
//! path (zero readback) can replace the pump later behind the same callback
//! contract.
//!
//! Every browser call goes through web-glue, the framework-owned JS boundary
//! (docs/proposals/own-web-bindings.md). The capture stream is published as
//! the stream's `native_source` as a `web_glue::dom::MediaStream` — THE
//! capture stream, not a copy (see [`start`]).
//!
//! Layer exclusion (Element Capture `restrictTo`) is a separate, later
//! addition — see the module docs in `private_layer`.

use crate::{NativeSource, RecorderError, RecordingConfig, Source};
use media_stream::FrameWriter;
use std::rc::Rc;
use web_glue::dom::{MediaStream, MediaStreamTrack};
use web_glue::js::Object;
use web_glue::{Closure, JsCast, JsError, JsFuture, JsValue};

web_glue::import! {
    // `navigator.mediaDevices`, 0 when absent (insecure context, old engine).
    fn js_media_devices() -> u32 =
        "() => { const n = typeof navigator === 'undefined' ? null : navigator; \
           const m = n == null ? null : n.mediaDevices; return m == null ? 0 : G.add(m); }";
    // `mediaDevices.getDisplayMedia(constraints)` → its Promise.
    #[catch]
    fn js_get_display_media(md: u32, c: u32) -> u32 = "(m, c) => G.add(G.get(m).getDisplayMedia(G.get(c)))";
    // A hidden, muted, autoplaying, inline `<video>` playing `s`, so the
    // browser plays it without user-gesture / fullscreen friction. `play()`'s
    // Promise is observed here; the pump waits for real dimensions.
    #[catch]
    fn js_video_for(s: u32) -> u32 =
        "(s) => { const v = document.createElement('video'); v.muted = true; v.autoplay = true; \
           v.setAttribute('playsinline', 'true'); v.srcObject = G.get(s); \
           const p = v.play(); if (p) p.catch(() => {}); return G.add(v); }";
    fn js_video_width(v: u32) -> u32 = "(v) => G.get(v).videoWidth >>> 0";
    fn js_video_height(v: u32) -> u32 = "(v) => G.get(v).videoHeight >>> 0";
    fn js_video_release(v: u32) = "(v) => { const e = G.get(v); e.pause(); e.srcObject = null; }";
    // An offscreen canvas's 2D context. `willReadFrequently` keeps the
    // backing store CPU-side: every tick reads back with `getImageData`, so
    // this avoids a per-readback GPU→CPU stall (and the browser's "Multiple
    // readback operations" warning).
    #[catch]
    fn js_create_ctx() -> u32 =
        "() => { const c = document.createElement('canvas'); \
           const x = c.getContext('2d', { willReadFrequently: true }); \
           if (x == null) throw new Error('no 2d canvas context'); return G.add(x); }";
    // Draw the video's current frame at its native w×h and copy the
    // straight RGBA8 `ImageData` into the w*h*4 bytes at `out`. 1 on
    // success, 0 if the draw / readback threw (tainted canvas, …).
    fn js_pump(x: u32, v: u32, w: u32, h: u32, out: usize) -> u32 =
        "(x, v, w, h, o) => { const ctx = G.get(x); const c = ctx.canvas; \
           if (c.width !== w) c.width = w; if (c.height !== h) c.height = h; \
           try { ctx.drawImage(G.get(v), 0, 0); \
             G.u8().set(ctx.getImageData(0, 0, w, h).data, o >>> 0); return 1; } \
           catch (_) { return 0; } }";
    // The interval id crosses as `f64`, not `i32`: a fake clock (Playwright's
    // `page.clock`) issues ids from `1e12`, which an `i32` truncates — the
    // clear then misses and the interval fires into the dropped `_pump`
    // (see `web_glue::dom::Window::request_animation_frame`).
    fn js_set_interval(f: u32, ms: i32) -> f64 = "(f, ms) => setInterval(G.get(f), ms)";
    fn js_clear_interval(id: f64) = "(id) => { clearInterval(id); }";
}

/// No pre-prompt on web: `getDisplayMedia` must run from a user gesture and
/// shows the picker at [`start`]. Resolving `Ok` here just defers consent
/// to that call.
pub(crate) async fn request_permission(_source: &Source) -> Result<(), RecorderError> {
    Ok(())
}

pub(crate) async fn start(
    config: RecordingConfig,
    writer: FrameWriter,
) -> Result<(Recording, Option<NativeSource>), RecorderError> {
    // SAFETY: the result is 0 or a fresh handle.
    let media_devices = match unsafe { js_media_devices() } {
        0 => return Err(platform("no navigator.mediaDevices")),
        h => unsafe { JsValue::from_raw(h) },
    };

    // `{ video: true }`, plus the non-standardized `preferCurrentTab` hint
    // for `Source::ThisApp`.
    let constraints = Object::new();
    constraints.set("video", &JsValue::from_bool(true)).map_err(js_err)?;
    if matches!(config.source, Source::ThisApp) {
        let _ = constraints.set("preferCurrentTab", &JsValue::from_bool(true));
    }

    // SAFETY: live handles; the result is a fresh Promise handle.
    let promise = unsafe { js_get_display_media(media_devices.raw(), constraints.as_js().raw()) }
        .map(|p| unsafe { JsValue::from_raw(p) })
        .map_err(js_err)?;
    let stream: MediaStream = JsFuture::new(&promise)
        .await
        .map_err(|e| map_get_display_media_err(&e))?
        .dyn_into()
        .map_err(|_| platform("getDisplayMedia did not return a MediaStream"))?;

    // SAFETY: a live stream handle; the result is a fresh element handle.
    let video = unsafe { js_video_for(stream.as_js().raw()) }
        .map(|v| unsafe { JsValue::from_raw(v) })
        .map_err(js_err)?;
    // SAFETY: the result is a fresh context handle.
    let ctx = unsafe { js_create_ctx() }.map(|c| unsafe { JsValue::from_raw(c) }).map_err(js_err)?;

    // The per-tick pump. Owns clones of everything it touches; the browser
    // invokes it asynchronously each interval, so a plain `FnMut` (no
    // self-reentrancy) is correct here. The `FrameWriter` is moved in and
    // pushed through a shared `&self` (`write_rgba8`), so the pump owns it.
    let pump = {
        let video = video.clone();
        let mut frame: Vec<u8> = Vec::new();
        Closure::new(move |_: JsValue| {
            // Display goes through `<video>.srcObject` (zero-copy) — the canvas
            // pump exists ONLY to feed the CPU RGBA channel. Its `getImageData`
            // is a GPU→CPU readback that stalls the wgpu graphics surface every
            // tick, so skip the whole pump unless a consumer is actually tapping
            // CPU frames (a `subscribe`r — e.g. a file encoder). A preview-only
            // recording session does zero readback. See `wants_cpu_frames`.
            if !writer.wants_cpu_frames() {
                return;
            }
            // SAFETY: live element handle.
            let (w, h) = unsafe { (js_video_width(video.raw()), js_video_height(video.raw())) };
            if w == 0 || h == 0 {
                return; // metadata not ready yet
            }
            frame.resize(w as usize * h as usize * 4, 0);
            // SAFETY: live handles; `frame` holds exactly w*h*4 bytes.
            if unsafe { js_pump(ctx.raw(), video.raw(), w, h, frame.as_mut_ptr() as usize) } == 0 {
                return; // tainted canvas / read failure — skip frame
            }
            writer.write_rgba8(w, h, &frame);
        })
    };

    let interval_ms = (1_000 / config.fps.max(1)) as i32;
    // SAFETY: a live closure handle.
    let interval_id = unsafe { js_set_interval(pump.as_js().raw(), interval_ms) };

    // Publish THE capture stream as the zero-copy native source so a
    // same-platform display / GPU / recording consumer (`<video srcObject>`,
    // media-writer) uses it instead of the canvas readback. The `Recording`
    // holds a handle clone — the same JS stream — and stops its tracks on
    // drop, which ends every consumer's view. (The web-sys port kept
    // `stream.clone()` — web-sys's inherent `clone()` is the JS
    // `MediaStream.clone()` — so its drop stopped a copy's tracks and the
    // published capture kept sharing the screen.)
    let native: NativeSource = Rc::new(stream.clone());
    let recording = Recording {
        interval_id,
        _pump: pump,
        stream,
        video,
    };
    Ok((recording, Some(native)))
}

/// A live web recording. Holds the DOM/stream resources alive; tearing it
/// down stops the interval and the capture tracks.
pub(crate) struct Recording {
    interval_id: f64,
    // Kept alive so the interval callback stays valid; dropped with us,
    // after `Drop` cleared the interval.
    _pump: Closure,
    stream: MediaStream,
    video: JsValue,
}

impl Drop for Recording {
    fn drop(&mut self) {
        // SAFETY: an interval this recording set.
        unsafe { js_clear_interval(self.interval_id) };
        // Stop every capture track so the browser drops the "sharing" UI.
        for track in self.stream.get_tracks().iter() {
            track.unchecked_into::<MediaStreamTrack>().stop();
        }
        // SAFETY: a live element handle.
        unsafe { js_video_release(self.video.raw()) };
    }
}

fn platform(msg: &str) -> RecorderError {
    RecorderError::Platform(msg.to_string())
}

fn js_err(e: JsError) -> RecorderError {
    RecorderError::Platform(format!("{e:?}"))
}

/// Map a `getDisplayMedia` rejection: a `NotAllowedError` (the user
/// dismissed the picker or denied) becomes [`RecorderError::PermissionDenied`];
/// anything else carries the DOM exception name + message.
fn map_get_display_media_err(e: &JsError) -> RecorderError {
    let name = e.get("name").ok().and_then(|v| v.as_string());
    match name.as_deref() {
        Some("NotAllowedError") => RecorderError::PermissionDenied,
        Some(name) => RecorderError::Platform(format!("{name}: {}", e.message())),
        None => RecorderError::Platform(format!("{e:?}")),
    }
}
