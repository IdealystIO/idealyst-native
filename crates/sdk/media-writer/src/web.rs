//! Web recording via `MediaRecorder`.
//!
//! On the web the browser owns the encoder/muxer, so this backend's job is to
//! assemble one `web_sys::MediaStream` carrying the right tracks and hand it to
//! a `MediaRecorder`; the recorded `Blob` is written back through the `files`
//! store.
//!
//! # Fast path — native tracks
//!
//! `camera` / `screen-recorder` (via `getUserMedia` / `getDisplayMedia`) and
//! `microphone` (via `getUserMedia`) each publish their live
//! `web_sys::MediaStream` as the stream's
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

use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    Blob, BlobEvent, BlobPropertyBag, CanvasRenderingContext2d, HtmlCanvasElement, MediaRecorder,
    MediaRecorderOptions, MediaStream, MediaStreamTrack,
};

use crate::{MediaInputs, MediaWriterError, RecordConfig};
use media_stream::Subscription;

/// Candidate MIME types, best (real MP4) first.
const MIME_CANDIDATES: &[&str] = &[
    "video/mp4",
    "video/webm;codecs=vp9,opus",
    "video/webm;codecs=vp8,opus",
    "video/webm",
];

fn err(msg: impl Into<String>) -> MediaWriterError {
    MediaWriterError::Backend(msg.into())
}

fn js_err(ctx: &str, e: JsValue) -> MediaWriterError {
    MediaWriterError::Backend(format!("{ctx}: {e:?}"))
}

pub(crate) struct RecordingHandle {
    recorder: MediaRecorder,
    chunks: Rc<RefCell<Vec<Blob>>>,
    mime: String,
    store: std::sync::Arc<dyn files::FileStore>,
    path: String,
    // Keep callbacks + the canvas frame pump alive for the recording's life.
    _on_data: Closure<dyn FnMut(BlobEvent)>,
    _video_pump: Option<Subscription>,
    _canvas: Option<HtmlCanvasElement>,
}

impl RecordingHandle {
    pub(crate) async fn stop(self) -> Result<(), MediaWriterError> {
        // Await the recorder's `stop` event so every buffered chunk has landed.
        let (tx, rx) = futures_oneshot();
        let tx = Rc::new(RefCell::new(Some(tx)));
        let on_stop = Closure::<dyn FnMut()>::new({
            let tx = tx.clone();
            move || {
                if let Some(tx) = tx.borrow_mut().take() {
                    let _ = tx.send(());
                }
            }
        });
        self.recorder
            .set_onstop(Some(on_stop.as_ref().unchecked_ref()));

        if self.recorder.state() != web_sys::RecordingState::Inactive {
            self.recorder
                .stop()
                .map_err(|e| js_err("MediaRecorder.stop", e))?;
            let _ = rx.await;
        }

        // Concatenate the recorded chunks into one Blob and read its bytes.
        let parts = js_sys::Array::new();
        for blob in self.chunks.borrow().iter() {
            parts.push(blob);
        }
        let opts = BlobPropertyBag::new();
        opts.set_type(&self.mime);
        let blob = Blob::new_with_blob_sequence_and_options(&parts, &opts)
            .map_err(|e| js_err("assemble Blob", e))?;
        let buf = JsFuture::from(blob.array_buffer())
            .await
            .map_err(|e| js_err("Blob.arrayBuffer", e))?;
        let bytes = js_sys::Uint8Array::new(&buf).to_vec();

        self.store.write(&self.path, &bytes).await?;
        drop(on_stop);
        Ok(())
    }
}

pub(crate) async fn start(
    inputs: MediaInputs<'_>,
    config: &RecordConfig,
) -> Result<RecordingHandle, MediaWriterError> {
    let combined = MediaStream::new().map_err(|e| js_err("new MediaStream", e))?;
    let mut video_pump = None;
    let mut canvas_keep = None;

    // --- Video track ---
    if let Some(stream) = inputs.video {
        if let Some(native) = stream
            .native_source()
            .and_then(|rc| rc.downcast::<MediaStream>().ok())
        {
            for track in native.get_video_tracks().iter() {
                combined.add_track(&track.unchecked_into::<MediaStreamTrack>());
            }
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
            Some(native) => {
                for track in native.get_audio_tracks().iter() {
                    combined.add_track(&track.unchecked_into::<MediaStreamTrack>());
                }
            }
            None => {
                return Err(MediaWriterError::Unsupported);
            }
        }
    }

    // --- Recorder ---
    let mime = pick_mime();
    let options = MediaRecorderOptions::new();
    if let Some(m) = &mime {
        options.set_mime_type(m);
    }
    if let Some(bps) = config.video_bitrate {
        options.set_video_bits_per_second(bps);
    }
    if let Some(bps) = config.audio_bitrate {
        options.set_audio_bits_per_second(bps);
    }
    let recorder = MediaRecorder::new_with_media_stream_and_media_recorder_options(
        &combined, &options,
    )
    .map_err(|e| js_err("new MediaRecorder", e))?;

    let chunks: Rc<RefCell<Vec<Blob>>> = Rc::new(RefCell::new(Vec::new()));
    let on_data = Closure::<dyn FnMut(BlobEvent)>::new({
        let chunks = chunks.clone();
        move |e: BlobEvent| {
            if let Some(blob) = e.data() {
                if blob.size() > 0.0 {
                    chunks.borrow_mut().push(blob);
                }
            }
        }
    });
    recorder.set_ondataavailable(Some(on_data.as_ref().unchecked_ref()));

    // Emit a chunk per second so a long recording isn't buffered as one giant
    // blob in memory.
    recorder
        .start_with_time_slice(1_000)
        .map_err(|e| js_err("MediaRecorder.start", e))?;

    Ok(RecordingHandle {
        recorder,
        chunks,
        mime: mime.unwrap_or_else(|| "video/webm".into()),
        store: config.store.clone(),
        path: config.path.clone(),
        _on_data: on_data,
        _video_pump: video_pump,
        _canvas: canvas_keep,
    })
}

/// First `MediaRecorder.isTypeSupported` MIME from [`MIME_CANDIDATES`], or
/// `None` to let the browser choose.
fn pick_mime() -> Option<String> {
    MIME_CANDIDATES
        .iter()
        .find(|m| MediaRecorder::is_type_supported(m))
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
    let document = web_sys::window()
        .and_then(|w| w.document())
        .ok_or_else(|| err("no document for canvas fallback"))?;
    let canvas: HtmlCanvasElement = document
        .create_element("canvas")
        .map_err(|e| js_err("create canvas", e))?
        .unchecked_into();
    let ctx: CanvasRenderingContext2d = canvas
        .get_context("2d")
        .map_err(|e| js_err("canvas 2d context", e))?
        .ok_or_else(|| err("canvas 2d context missing"))?
        .unchecked_into();

    // The persistent pump draws every frame and resizes the canvas if the
    // source dimensions ever change. It also fires `first_tx` exactly once, so
    // the size-before-capture step below can park on the first pushed frame
    // when the producer hasn't buffered one yet.
    let (first_tx, first_rx) = futures_oneshot();
    let first_tx = Rc::new(RefCell::new(Some(first_tx)));
    let canvas_for_cb = canvas.clone();
    let ctx_for_cb = ctx.clone();
    let first_tx_cb = first_tx.clone();
    let sub = stream.subscribe(move |frame| {
        if canvas_for_cb.width() != frame.width || canvas_for_cb.height() != frame.height {
            canvas_for_cb.set_width(frame.width);
            canvas_for_cb.set_height(frame.height);
        }
        let clamped = wasm_bindgen::Clamped(frame.data);
        if let Ok(image) = web_sys::ImageData::new_with_u8_clamped_array_and_sh(
            clamped,
            frame.width,
            frame.height,
        ) {
            let _ = ctx_for_cb.put_image_data(&image, 0.0, 0.0);
        }
        if let Some(tx) = first_tx_cb.borrow_mut().take() {
            let _ = tx.send(());
        }
    });

    // Lock the canvas to a real frame size BEFORE captureStream() — see the
    // doc comment: avc1 can't survive a mid-stream resolution change.
    let mut buf = Vec::new();
    match stream.latest(&mut buf) {
        Some((w, h)) => {
            // The producer already has a frame: size + draw it synchronously so
            // captureStream()'s very first emitted frame carries content at the
            // locked resolution (no initial blank frame). `add_subscriber` does
            // not replay buffered frames, so without this pull the pump wouldn't
            // fire until the *next* push and the canvas would stay 300×150.
            canvas.set_width(w);
            canvas.set_height(h);
            let clamped = wasm_bindgen::Clamped(buf.as_slice());
            if let Ok(image) =
                web_sys::ImageData::new_with_u8_clamped_array_and_sh(clamped, w, h)
            {
                let _ = ctx.put_image_data(&image, 0.0, 0.0);
            }
        }
        // No buffered frame yet: park until the pump draws the first pushed one.
        None => first_rx.await,
    }

    let capture: MediaStream = canvas
        .capture_stream()
        .map_err(|e| js_err("canvas.captureStream", e))?;
    for track in capture.get_video_tracks().iter() {
        combined.add_track(&track.unchecked_into::<MediaStreamTrack>());
    }
    Ok((canvas, sub))
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
        wasm_bindgen_futures::spawn_local(async move {
            writer.write_rgba8(W, H, &vec![0u8; (W * H * 4) as usize]);
        });

        let (canvas, _sub) = canvas_capture(&stream, &combined)
            .await
            .expect("canvas_capture");

        assert_eq!(canvas.width(), W, "canvas sized to the first pushed frame");
        assert_eq!(canvas.height(), H, "canvas sized to the first pushed frame");
    }
}
