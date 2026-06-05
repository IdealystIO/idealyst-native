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
//! `MediaRecorder`'s output format is browser-chosen: Safari yields real MP4,
//! Chromium yields WebM. We request `video/mp4` and fall back to `video/webm`,
//! writing whatever the browser produces to the path you gave. The bytes are
//! always a valid, playable file; only the container may differ from `.mp4` on
//! Chromium. This is a genuine platform constraint, documented in the README.

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
            let (canvas, sub) = canvas_capture(stream, &combined)?;
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
fn canvas_capture(
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

    let canvas_for_cb = canvas.clone();
    let sub = stream.subscribe(move |frame| {
        if canvas_for_cb.width() != frame.width {
            canvas_for_cb.set_width(frame.width);
            canvas_for_cb.set_height(frame.height);
        }
        let clamped = wasm_bindgen::Clamped(frame.data);
        if let Ok(image) = web_sys::ImageData::new_with_u8_clamped_array_and_sh(
            clamped,
            frame.width,
            frame.height,
        ) {
            let _ = ctx.put_image_data(&image, 0.0, 0.0);
        }
    });

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
