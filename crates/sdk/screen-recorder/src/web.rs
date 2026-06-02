//! Web capture backend — `getDisplayMedia`.
//!
//! The browser shows its source picker (tab/window/screen); `Source::ThisApp`
//! adds the `preferCurrentTab` hint so the app's own tab is the default
//! choice. We play the resulting `MediaStream` into a hidden `<video>` and,
//! on a `setInterval` cadence at the configured fps, draw it into an
//! offscreen `<canvas>` and read back RGBA pixels for the frame callback.
//!
//! Why the canvas pump and not `MediaStreamTrackProcessor`/WebCodecs: the
//! canvas path is supported in every browser that has `getDisplayMedia`,
//! needs only stable `web-sys` bindings, and keeps the first working path
//! simple. A WebCodecs `VideoFrame` fast path (zero readback) can replace
//! the pump later behind the same callback contract.
//!
//! Layer exclusion (Element Capture `restrictTo`) is a separate, later
//! addition — see the module docs in `private_layer`.

use crate::{BoxedFrameCallback, PixelFormat, RecorderError, RecordingConfig, Source, VideoFrame};
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

/// No pre-prompt on web: `getDisplayMedia` must run from a user gesture and
/// shows the picker at [`start`]. Resolving `Ok` here just defers consent
/// to that call.
pub(crate) async fn request_permission(_source: &Source) -> Result<(), RecorderError> {
    Ok(())
}

pub(crate) async fn start(
    config: RecordingConfig,
    on_frame: BoxedFrameCallback,
) -> Result<Recording, RecorderError> {
    let window = web_sys::window().ok_or_else(|| platform("no window"))?;
    let document = window.document().ok_or_else(|| platform("no document"))?;
    let performance = window.performance().ok_or_else(|| platform("no performance clock"))?;
    let media_devices = window.navigator().media_devices().map_err(js_err)?;

    // Build the constraints object via Reflect so we don't depend on a
    // specific web-sys setter signature, and can set the non-standardized
    // `preferCurrentTab` hint the typed struct doesn't expose.
    let constraints = web_sys::DisplayMediaStreamConstraints::new();
    js_sys::Reflect::set(constraints.as_ref(), &"video".into(), &JsValue::TRUE).map_err(js_err)?;
    if matches!(config.source, Source::ThisApp) {
        let _ = js_sys::Reflect::set(
            constraints.as_ref(),
            &"preferCurrentTab".into(),
            &JsValue::TRUE,
        );
    }

    let promise = media_devices
        .get_display_media_with_constraints(&constraints)
        .map_err(js_err)?;
    let stream: web_sys::MediaStream = JsFuture::from(promise)
        .await
        .map_err(|e| map_get_display_media_err(&e))?
        .dyn_into()
        .map_err(|_| platform("getDisplayMedia did not return a MediaStream"))?;

    // Hidden <video> playing the captured stream. Muted + inline so the
    // browser autoplays it without user-gesture / fullscreen friction.
    let video: web_sys::HtmlVideoElement = document
        .create_element("video")
        .map_err(js_err)?
        .dyn_into()
        .map_err(|_| platform("could not create <video>"))?;
    video.set_muted(true);
    video.set_autoplay(true);
    let _ = video.set_attribute("playsinline", "true");
    video.set_src_object(Some(&stream));
    // play() returns a promise; we don't need to await it.
    let _ = video.play().map_err(js_err)?;

    // Offscreen <canvas> for pixel read-back.
    let canvas: web_sys::HtmlCanvasElement = document
        .create_element("canvas")
        .map_err(js_err)?
        .dyn_into()
        .map_err(|_| platform("could not create <canvas>"))?;
    let ctx: web_sys::CanvasRenderingContext2d = canvas
        .get_context("2d")
        .map_err(js_err)?
        .ok_or_else(|| platform("no 2d canvas context"))?
        .dyn_into()
        .map_err(|_| platform("unexpected canvas context type"))?;

    let paused = Rc::new(RefCell::new(false));
    let callback = Rc::new(RefCell::new(on_frame));

    // The per-tick pump. Owns clones of everything it touches; the browser
    // invokes it asynchronously each interval, so a plain `FnMut` (no
    // self-reentrancy) is correct here.
    let pump = {
        let video = video.clone();
        let canvas = canvas.clone();
        let ctx = ctx.clone();
        let paused = paused.clone();
        let callback = callback.clone();
        let performance = performance.clone();
        Closure::<dyn FnMut()>::new(move || {
            if *paused.borrow() {
                return;
            }
            let (w, h) = (video.video_width(), video.video_height());
            if w == 0 || h == 0 {
                return; // metadata not ready yet
            }
            if canvas.width() != w {
                canvas.set_width(w);
            }
            if canvas.height() != h {
                canvas.set_height(h);
            }
            if ctx
                .draw_image_with_html_video_element(&video, 0.0, 0.0)
                .is_err()
            {
                return;
            }
            let image_data = match ctx.get_image_data(0.0, 0.0, w as f64, h as f64) {
                Ok(d) => d,
                Err(_) => return, // tainted canvas / read failure — skip frame
            };
            let frame = VideoFrame {
                width: w,
                height: h,
                format: PixelFormat::Rgba8,
                bytes_per_row: (w as usize) * 4,
                data: image_data.data().0,
                timestamp_micros: (performance.now() * 1_000.0) as u64,
            };
            (callback.borrow_mut())(&frame);
        })
    };

    let interval_ms = (1_000 / config.fps.max(1)) as i32;
    let interval_id = window
        .set_interval_with_callback_and_timeout_and_arguments_0(
            pump.as_ref().unchecked_ref(),
            interval_ms,
        )
        .map_err(js_err)?;

    Ok(Recording {
        window,
        interval_id,
        _pump: pump,
        stream,
        video,
        paused,
    })
}

/// A live web recording. Holds the DOM/stream resources alive; tearing it
/// down stops the interval and the capture tracks.
pub(crate) struct Recording {
    window: web_sys::Window,
    interval_id: i32,
    // Kept alive so the interval callback stays valid; dropped with us.
    _pump: Closure<dyn FnMut()>,
    stream: web_sys::MediaStream,
    video: web_sys::HtmlVideoElement,
    paused: Rc<RefCell<bool>>,
}

impl Recording {
    pub(crate) fn pause(&self) -> Result<(), RecorderError> {
        *self.paused.borrow_mut() = true;
        Ok(())
    }

    pub(crate) fn resume(&self) -> Result<(), RecorderError> {
        *self.paused.borrow_mut() = false;
        Ok(())
    }

    pub(crate) async fn stop(self) -> Result<(), RecorderError> {
        // Teardown happens in Drop; `self` drops at the end of this scope.
        Ok(())
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        self.window.clear_interval_with_handle(self.interval_id);
        // Stop every capture track so the browser drops the "sharing" UI.
        let tracks = self.stream.get_tracks();
        for i in 0..tracks.length() {
            if let Ok(track) = tracks.get(i).dyn_into::<web_sys::MediaStreamTrack>() {
                track.stop();
            }
        }
        self.video.set_src_object(None);
    }
}

fn platform(msg: &str) -> RecorderError {
    RecorderError::Platform(msg.to_string())
}

fn js_err(e: JsValue) -> RecorderError {
    RecorderError::Platform(format!("{e:?}"))
}

/// Map a `getDisplayMedia` rejection: a `NotAllowedError` (the user
/// dismissed the picker or denied) becomes [`RecorderError::PermissionDenied`];
/// anything else carries the DOM exception name + message.
fn map_get_display_media_err(e: &JsValue) -> RecorderError {
    if let Some(ex) = e.dyn_ref::<web_sys::DomException>() {
        if ex.name() == "NotAllowedError" {
            return RecorderError::PermissionDenied;
        }
        return RecorderError::Platform(format!("{}: {}", ex.name(), ex.message()));
    }
    RecorderError::Platform(format!("{e:?}"))
}
