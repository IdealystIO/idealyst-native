//! Web recording via `MediaRecorder`.
//!
//! On the web the browser owns the encoder/muxer, so this backend's job is to
//! assemble one `MediaStream` carrying the right tracks and hand it to a
//! `MediaRecorder`; the recorded `Blob` is written back through the `files`
//! store. Every browser call is a web-glue binding
//! (docs/proposals/own-web-bindings.md).
//!
//! # Fast path — native tracks
//!
//! `camera` / `screen-recorder` (via `getUserMedia` / `getDisplayMedia`) and
//! `microphone` (via `getUserMedia`) each publish their live `MediaStream` (a
//! `web_glue::dom::MediaStream`) as the stream's
//! [`native_source`](media_stream::MediaStream::native_source). When present we
//! pull the video track(s) from the video stream and the audio track(s) from
//! the audio stream into one combined stream and record that directly —
//! hardware-encoded, perfectly synced, no per-frame work in wasm.
//!
//! # Fallback — canvas capture
//!
//! If a *video* source has no native handle (a CPU-only producer), we pump its
//! RGBA frames into a `<canvas>` and use `canvas.captureStream()` as the video
//! track. An *audio* source with no native handle can't be reconstructed into
//! a recordable track without rebuilding the browser's audio graph, so that
//! case reports [`MediaWriterError::Unsupported`] with a clear message rather
//! than shipping a fragile WebAudio path.
//!
//! # Container caveat
//!
//! `MediaRecorder`'s output format is browser-chosen. Safari has always yielded
//! real MP4; recent Chromium versions now also support `video/mp4` (H.264/avc1)
//! in `MediaRecorder` and pick it from our candidate list, where older versions
//! fell back to WebM. We request `video/mp4` first and fall back to
//! `video/webm`, writing whatever the browser produces to the path you gave.
//! The bytes are always a valid, playable file; only the container may differ
//! from `.mp4` on a Chromium that lacks MP4 support. This is a genuine platform
//! constraint, documented in the README.
//!
//! Because Chromium now commonly encodes the canvas-capture fallback as
//! H.264/avc1 — a codec that **cannot change resolution mid-stream** — the
//! canvas MUST be sized to the real frame dimensions before `captureStream()`;
//! see [`canvas_capture`].

use std::cell::RefCell;
use std::rc::Rc;

use web_glue::dom::{HtmlCanvasElement, MediaStream, MediaStreamTrack};
use web_glue::js::Uint8Array;
use web_glue::{string, Closure, JsCast, JsError, JsFuture, JsValue};

use crate::{MediaInputs, MediaWriterError, RecordConfig};
use media_stream::Subscription;

/// Candidate MIME types, best (real MP4) first.
const MIME_CANDIDATES: &[&str] = &[
    "video/mp4",
    "video/webm;codecs=vp9,opus",
    "video/webm;codecs=vp8,opus",
    "video/webm",
];

/// `MediaRecorder` time slice: a chunk per second, so a long recording isn't
/// buffered as one giant blob in memory.
const TIME_SLICE_MS: u32 = 1_000;

web_glue::import! {
    // `MediaRecorder.isTypeSupported(mime)`; 0 without MediaRecorder.
    fn js_is_type_supported(p: usize, l: usize) -> u32 =
        "(p, l) => typeof MediaRecorder !== 'undefined' && MediaRecorder.isTypeSupported(G.str(p, l)) ? 1 : 0";
    // `new MediaRecorder(stream, options)` wrapped as `{ rec, chunks }`: each
    // non-empty `dataavailable` Blob is pushed onto `chunks` in JS (the
    // chunks never cross into Rust until `stop` assembles them). `mime` is
    // omitted when empty; a bitrate of 0 is omitted.
    #[catch]
    fn js_recorder_new(s: u32, mp: usize, ml: usize, vbps: f64, abps: f64) -> u32 =
        "(s, mp, ml, v, a) => { const o = {}; const m = G.str(mp, ml); if (m) o.mimeType = m; \
           if (v > 0) o.videoBitsPerSecond = v; if (a > 0) o.audioBitsPerSecond = a; \
           const r = new MediaRecorder(G.get(s), o); const x = { rec: r, chunks: [] }; \
           r.ondataavailable = (e) => { if (e.data && e.data.size > 0) x.chunks.push(e.data); }; \
           return G.add(x); }";
    #[catch]
    fn js_recorder_start(x: u32, slice: u32) = "(x, t) => { G.get(x).rec.start(t); }";
    // 1 while the recorder is recording / paused, 0 once inactive.
    fn js_recorder_active(x: u32) -> u32 = "(x) => G.get(x).rec.state !== 'inactive' ? 1 : 0";
    fn js_recorder_onstop(x: u32, f: u32) = "(x, f) => { G.get(x).rec.onstop = G.get(f); }";
    #[catch]
    fn js_recorder_stop(x: u32) = "(x) => { G.get(x).rec.stop(); }";
    // The recorded chunks as one Blob of `mime`, read out: a Promise of its
    // ArrayBuffer.
    #[catch]
    fn js_recorder_bytes(x: u32, mp: usize, ml: usize) -> u32 =
        "(x, mp, ml) => G.add(new Blob(G.get(x).chunks, { type: G.str(mp, ml) }).arrayBuffer())";
    // A `<canvas>` and its 2D context (`willReadFrequently`: see
    // `canvas_capture`), as the context; throws without a document.
    #[catch]
    fn js_new_ctx() -> u32 =
        "() => { const c = document.createElement('canvas'); \
           const x = c.getContext('2d', { willReadFrequently: true }); \
           if (x == null) throw new Error('canvas 2d context missing'); return G.add(x); }";
    fn js_ctx_canvas(x: u32) -> u32 = "(x) => G.add(G.get(x).canvas)";
    // Size the canvas to w×h (only when it differs — a resize clears it) and
    // `putImageData` the w*h*4 straight RGBA8 bytes at `p`.
    fn js_put_frame(x: u32, p: usize, l: usize, w: u32, h: u32) =
        "(x, p, l, w, h) => { const ctx = G.get(x); const c = ctx.canvas; \
           if (c.width !== w) c.width = w; if (c.height !== h) c.height = h; \
           const px = new Uint8ClampedArray(G.u8().slice(p >>> 0, (p >>> 0) + (l >>> 0)).buffer); \
           try { ctx.putImageData(new ImageData(px, w, h), 0, 0); } catch (_) {} }";
    #[catch]
    fn js_capture_stream(c: u32) -> u32 = "(c) => G.add(G.get(c).captureStream())";
}

fn err(msg: impl Into<String>) -> MediaWriterError {
    MediaWriterError::Backend(msg.into())
}

fn js_err(ctx: &str, e: JsError) -> MediaWriterError {
    MediaWriterError::Backend(format!("{ctx}: {}", e.message()))
}

pub(crate) struct RecordingHandle {
    /// `{ rec: MediaRecorder, chunks: Blob[] }` (see [`js_recorder_new`]).
    recorder: JsValue,
    mime: String,
    store: std::sync::Arc<dyn files::FileStore>,
    path: String,
    // Keep the canvas frame pump alive for the recording's life.
    _video_pump: Option<Subscription>,
    _canvas: Option<HtmlCanvasElement>,
}

impl RecordingHandle {
    pub(crate) async fn stop(self) -> Result<(), MediaWriterError> {
        // Await the recorder's `stop` event so every buffered chunk has landed.
        let (tx, rx) = futures_oneshot();
        let tx = Rc::new(RefCell::new(Some(tx)));
        let on_stop = Closure::new({
            let tx = tx.clone();
            move |_: JsValue| {
                if let Some(tx) = tx.borrow_mut().take() {
                    let _ = tx.send(());
                }
            }
        });
        // SAFETY (below): live handles.
        unsafe { js_recorder_onstop(self.recorder.raw(), on_stop.as_js().raw()) };

        if unsafe { js_recorder_active(self.recorder.raw()) } != 0 {
            unsafe { js_recorder_stop(self.recorder.raw()) }.map_err(|e| js_err("MediaRecorder.stop", e))?;
            let _ = rx.await;
        }

        // Concatenate the recorded chunks into one Blob and read its bytes.
        let (mp, ml) = string::abi(&self.mime);
        // SAFETY: a live handle; the result is a fresh Promise handle.
        let buf = unsafe { js_recorder_bytes(self.recorder.raw(), mp, ml) }
            .map(|p| unsafe { JsValue::from_raw(p) })
            .map_err(|e| js_err("assemble Blob", e))?;
        let buf = JsFuture::new(&buf).await.map_err(|e| js_err("Blob.arrayBuffer", e))?;
        let bytes = Uint8Array::new(&buf).to_vec();

        self.store.write(&self.path, &bytes).await?;
        drop(on_stop);
        Ok(())
    }
}

/// Add every track of `tracks` (a `getVideoTracks()` / `getAudioTracks()`
/// array) to `combined`.
fn add_tracks(combined: &MediaStream, tracks: web_glue::js::Array) {
    for track in tracks.iter() {
        combined.add_track(&track.unchecked_into::<MediaStreamTrack>());
    }
}

pub(crate) async fn start(
    inputs: MediaInputs<'_>,
    config: &RecordConfig,
) -> Result<(RecordingHandle, String), MediaWriterError> {
    let combined = MediaStream::new().map_err(|e| js_err("new MediaStream", e))?;
    let mut video_pump = None;
    let mut canvas_keep = None;

    // --- Video track ---
    if let Some(stream) = inputs.video {
        if let Some(native) = stream
            .native_source()
            .and_then(|rc| rc.downcast::<MediaStream>().ok())
        {
            add_tracks(&combined, native.get_video_tracks());
        } else {
            // CPU-only producer: pump frames into a canvas and capture it.
            let (canvas, sub) = canvas_capture(stream, &combined).await?;
            canvas_keep = Some(canvas);
            video_pump = Some(sub);
        }
    }

    // --- Audio track ---
    if let Some(stream) = inputs.audio {
        match stream
            .native_source()
            .and_then(|rc| rc.downcast::<MediaStream>().ok())
        {
            Some(native) => add_tracks(&combined, native.get_audio_tracks()),
            None => {
                return Err(MediaWriterError::Unsupported);
            }
        }
    }

    // --- Recorder ---
    let mime = pick_mime();
    let (mp, ml) = string::abi(mime.as_deref().unwrap_or(""));
    // SAFETY: a live stream handle; the result is a fresh handle.
    let recorder = unsafe {
        js_recorder_new(
            combined.as_js().raw(),
            mp,
            ml,
            config.video_bitrate.map_or(0.0, f64::from),
            config.audio_bitrate.map_or(0.0, f64::from),
        )
    }
    .map(|r| unsafe { JsValue::from_raw(r) })
    .map_err(|e| js_err("new MediaRecorder", e))?;

    // SAFETY: a live handle.
    unsafe { js_recorder_start(recorder.raw(), TIME_SLICE_MS) }.map_err(|e| js_err("MediaRecorder.start", e))?;

    // The web backend writes the recorded blob to the requested path verbatim
    // (only the encoded container inside may differ per the browser's choice —
    // see the module's "Container caveat"), so the effective relative path is
    // the one asked for.
    Ok((
        RecordingHandle {
            recorder,
            mime: mime.unwrap_or_else(|| "video/webm".into()),
            store: config.store.clone(),
            path: config.path.clone(),
            _video_pump: video_pump,
            _canvas: canvas_keep,
        },
        config.path.clone(),
    ))
}

/// First `MediaRecorder.isTypeSupported` MIME from [`MIME_CANDIDATES`], or
/// `None` to let the browser choose.
fn pick_mime() -> Option<String> {
    MIME_CANDIDATES
        .iter()
        .find(|m| {
            let (p, l) = string::abi(m);
            // SAFETY: a borrowed string for the call.
            unsafe { js_is_type_supported(p, l) != 0 }
        })
        .map(|m| m.to_string())
}

/// Build a `<canvas>` fed by `stream`'s RGBA frames and add its captured video
/// track to `combined`. Returns the canvas (kept alive) + the frame
/// subscription.
///
/// ## Why this awaits the first frame before `captureStream()`
///
/// A bare `<canvas>` is 300×150 until something sizes it. If we captured the
/// stream at that default and let the first frame resize the canvas afterward,
/// the captured video track would change resolution one frame in. The browser
/// now commonly encodes this fallback as H.264/avc1 (see the module-level
/// container note), and **avc1 cannot change resolution mid-stream** — Chrome
/// logs `avc1.* … codec description is not supposed to change` and the recorded
/// file is corrupt. So we lock the canvas to the real frame dimensions *before*
/// `captureStream()`: pull an already-buffered frame via
/// [`latest`](media_stream::MediaStream::latest) if the producer has one, else
/// park until the pump draws the first pushed frame (which sizes the canvas).
///
/// A producer that never emits a single frame leaves this pending — by design,
/// a zero-frame recording is degenerate, and 300×150 black is not a useful
/// substitute.
async fn canvas_capture(
    stream: &media_stream::MediaStream,
    combined: &MediaStream,
) -> Result<(HtmlCanvasElement, Subscription), MediaWriterError> {
    // These 2D contexts are written every frame; `willReadFrequently` keeps the
    // backing store CPU-side (avoids a per-frame GPU round trip) and silences
    // the browser's "Multiple readback operations" warning.
    // SAFETY: the result is a fresh context handle.
    let ctx = unsafe { js_new_ctx() }
        .map(|c| unsafe { JsValue::from_raw(c) })
        .map_err(|_| err("no document for canvas fallback"))?;
    // SAFETY: a live context handle.
    let canvas: HtmlCanvasElement = unsafe { JsValue::from_raw(js_ctx_canvas(ctx.raw())) }.unchecked_into();

    // The persistent pump draws every frame and resizes the canvas if the
    // source dimensions ever change. It also fires `first_tx` exactly once, so
    // the size-before-capture step below can park on the first pushed frame
    // when the producer hasn't buffered one yet.
    let (first_tx, first_rx) = futures_oneshot();
    let first_tx = Rc::new(RefCell::new(Some(first_tx)));
    let ctx_for_cb = ctx.clone();
    let first_tx_cb = first_tx.clone();
    let sub = stream.subscribe(move |frame| {
        put_frame(&ctx_for_cb, frame.data, frame.width, frame.height);
        if let Some(tx) = first_tx_cb.borrow_mut().take() {
            let _ = tx.send(());
        }
    });

    // Lock the canvas to a real frame size BEFORE captureStream() — see the
    // doc comment: avc1 can't survive a mid-stream resolution change.
    let mut buf = Vec::new();
    match stream.latest(&mut buf) {
        // The producer already has a frame: size + draw it synchronously so
        // captureStream()'s very first emitted frame carries content at the
        // locked resolution (no initial blank frame). `add_subscriber` does
        // not replay buffered frames, so without this pull the pump wouldn't
        // fire until the *next* push and the canvas would stay 300×150.
        Some((w, h)) => put_frame(&ctx, &buf, w, h),
        // No buffered frame yet: park until the pump draws the first pushed one.
        None => first_rx.await,
    }

    // SAFETY: a live canvas handle; the result is a fresh stream handle.
    let capture: MediaStream = unsafe { js_capture_stream(canvas.as_js().raw()) }
        .map(|s| unsafe { JsValue::from_raw(s) })
        .map_err(|e| js_err("canvas.captureStream", e))?
        .unchecked_into();
    add_tracks(combined, capture.get_video_tracks());
    Ok((canvas, sub))
}

/// Size the canvas to `w`×`h` and draw one straight-RGBA8 frame into it.
fn put_frame(ctx: &JsValue, rgba: &[u8], w: u32, h: u32) {
    if rgba.len() != w as usize * h as usize * 4 {
        return;
    }
    // SAFETY: a live context handle; `rgba` is borrowed for the call.
    unsafe { js_put_frame(ctx.raw(), rgba.as_ptr() as usize, rgba.len(), w, h) }
}

// The `onstop`-wait future is a WAKER-BASED single-shot signal (see
// `crate::oneshot`). It MUST NOT busy-spin the waker — an earlier mpsc version
// re-woke itself on every empty poll, starving the wasm event loop so the
// `onstop` DOM event never fired and the tab FROZE on stop. The shared module
// carries the regression tests.
use crate::oneshot::futures_oneshot;

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::*;

    // These need a DOM (`document`, `<canvas>`) + a real `captureStream`, so
    // they run in a headless browser, not Node.
    wasm_bindgen_test_configure!(run_in_browser);

    /// Regression: the canvas-capture fallback must lock the canvas to the real
    /// frame size BEFORE `captureStream()`. Pre-fix, `canvas_capture` captured
    /// the stream while the canvas was still the bare-`<canvas>` 300×150 default
    /// and let the first frame resize it afterward — a mid-stream resolution
    /// change that H.264/avc1 can't encode (Chrome: "codec description is not
    /// supposed to change", corrupt output). Here a 640×480 frame is buffered
    /// before capture starts (the stage-canvas case); `add_subscriber` does not
    /// replay it, so only the `latest()` pull added by the fix sizes the canvas.
    #[wasm_bindgen_test]
    async fn canvas_capture_locks_real_size_before_capturestream() {
        const W: u32 = 640;
        const H: u32 = 480;

        let (stream, writer) = media_stream::MediaStream::new();
        // Producer already has a frame when recording starts.
        writer.write_rgba8(W, H, &vec![0u8; (W * H * 4) as usize]);

        let combined = MediaStream::new().expect("new MediaStream");
        let (canvas, _sub) = canvas_capture(&stream, &combined)
            .await
            .expect("canvas_capture");

        assert_eq!(
            canvas.width(),
            W,
            "canvas width must be locked to the frame before captureStream (was the 300×150 default)"
        );
        assert_eq!(
            canvas.height(),
            H,
            "canvas height must be locked to the frame before captureStream (was the 300×150 default)"
        );
        // The captured track exists and carries the locked resolution.
        let tracks = combined.get_video_tracks();
        assert_eq!(tracks.length(), 1, "exactly one captured video track");
    }

    /// When no frame is buffered yet, `canvas_capture` parks until the first
    /// pushed frame sizes the canvas, then captures at that size — never at the
    /// 300×150 default. Pushing after the await proves the park-then-resume path.
    #[wasm_bindgen_test]
    async fn canvas_capture_awaits_first_pushed_frame() {
        const W: u32 = 800;
        const H: u32 = 600;

        let (stream, writer) = media_stream::MediaStream::new();
        let combined = MediaStream::new().expect("new MediaStream");

        // No buffered frame: kick the first push from a microtask so the
        // `first_rx.await` inside `canvas_capture` parks and then resumes. The
        // pump is subscribed synchronously before that await, so it catches it.
        web_glue::spawn_local(async move {
            writer.write_rgba8(W, H, &vec![0u8; (W * H * 4) as usize]);
        });

        let (canvas, _sub) = canvas_capture(&stream, &combined)
            .await
            .expect("canvas_capture");

        assert_eq!(canvas.width(), W, "canvas sized to the first pushed frame");
        assert_eq!(canvas.height(), H, "canvas sized to the first pushed frame");
    }
}
