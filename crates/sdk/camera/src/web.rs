//! Web capture via `getUserMedia` + a `<video>`/`<canvas>` frame pump.
//!
//! `getUserMedia({video:…})` yields a `MediaStream` (and triggers the
//! browser's permission prompt). We attach it to a detached `<video>`
//! element, then on each animation frame draw the current video frame into
//! an offscreen `<canvas>` and read it back with `getImageData`, which
//! hands us straight (non-premultiplied) `RGBA8` — exactly the SDK's frame
//! format, no conversion needed.
//!
//! `requestAnimationFrame` (rather than `requestVideoFrameCallback`, which
//! isn't universally exposed) keeps this a dependency-free single crate; it
//! samples at the display refresh, which for a preview/processing feed is
//! the right cadence. Swapping in `requestVideoFrameCallback` later is a
//! transparent change behind this same API.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use js_sys::Reflect;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    CanvasRenderingContext2d, HtmlCanvasElement, HtmlVideoElement, MediaStream,
    MediaStreamConstraints, MediaStreamTrack,
};

use crate::{CameraConfig, CameraError, CameraFacing, NativeSource};
use media_stream::FrameWriter;

/// The self-rescheduling rAF closure, held in an `Rc<RefCell<Option<…>>>`
/// so it can re-arm itself by reference each frame.
type PumpClosure = Rc<RefCell<Option<Closure<dyn FnMut()>>>>;

/// Keeps the capture pump and its media tracks alive. Drop stops the pump
/// and releases the camera (clearing the browser's recording indicator).
pub(crate) struct StreamHandle {
    video: HtmlVideoElement,
    stream: MediaStream,
    running: Rc<Cell<bool>>,
    raf_id: Rc<Cell<i32>>,
    // Owns the rAF closure (and, inside it, the user callback) for the
    // pump's lifetime; dropped last.
    _pump: PumpClosure,
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        self.running.set(false);
        if let Some(win) = web_sys::window() {
            let _ = win.cancel_animation_frame(self.raf_id.get());
        }
        // Detach the source and stop every track.
        self.video.set_src_object(None);
        for track in self.stream.get_tracks().iter() {
            if let Ok(track) = track.dyn_into::<MediaStreamTrack>() {
                track.stop();
            }
        }
    }
}

pub(crate) async fn request_permission() -> Result<(), CameraError> {
    // Acquire a stream purely to surface the prompt, then immediately stop
    // its tracks. A granted prompt is cached by the browser, so the later
    // `open()` won't prompt again.
    let stream = get_user_media(&CameraConfig::default()).await?;
    for track in stream.get_tracks().iter() {
        if let Ok(track) = track.dyn_into::<MediaStreamTrack>() {
            track.stop();
        }
    }
    Ok(())
}

pub(crate) async fn open(
    config: CameraConfig,
    writer: FrameWriter,
) -> Result<(StreamHandle, Option<NativeSource>), CameraError> {
    let document = web_sys::window()
        .and_then(|w| w.document())
        .ok_or(CameraError::Unsupported)?;

    let stream = get_user_media(&config).await?;

    // Detached <video> playing the stream — never inserted into the DOM.
    let video: HtmlVideoElement = document
        .create_element("video")
        .map_err(|e| CameraError::Backend(format!("create video: {}", err_string(&e))))?
        .dyn_into()
        .map_err(|_| CameraError::Backend("element is not a video".into()))?;
    video.set_muted(true);
    let _ = video.set_attribute("playsinline", "");
    video.set_src_object(Some(&stream));
    // play() returns a Promise; we don't need to await it — the pump waits
    // for non-zero dimensions before sampling.
    let _ = video.play();

    let canvas: HtmlCanvasElement = document
        .create_element("canvas")
        .map_err(|e| CameraError::Backend(format!("create canvas: {}", err_string(&e))))?
        .dyn_into()
        .map_err(|_| CameraError::Backend("element is not a canvas".into()))?;
    let ctx: CanvasRenderingContext2d = canvas
        .get_context("2d")
        .map_err(|e| CameraError::Backend(format!("get 2d context: {}", err_string(&e))))?
        .ok_or_else(|| CameraError::Backend("no 2d context".into()))?
        .dyn_into()
        .map_err(|_| CameraError::Backend("context is not 2d".into()))?;

    let running = Rc::new(Cell::new(true));
    let raf_id = Rc::new(Cell::new(0));
    let pump: PumpClosure = Rc::new(RefCell::new(None));

    let closure = {
        let video = video.clone();
        let running = running.clone();
        let raf_id = raf_id.clone();
        let pump = pump.clone();
        let writer = writer;
        Closure::wrap(Box::new(move || {
            if !running.get() {
                return;
            }
            // Display goes through `<video>.srcObject` (zero-copy); the canvas
            // readback in `pump_frame` exists only to feed the CPU RGBA channel.
            // Its `get_image_data` is a GPU→CPU readback that stalls the wgpu
            // graphics surface, so skip it unless a consumer is tapping CPU
            // frames (a `subscribe`r). See `FrameWriter::wants_cpu_frames`.
            if writer.wants_cpu_frames() {
                pump_frame(&video, &canvas, &ctx, &writer);
            }
            // Re-arm for the next display frame. rAF calls us later (not
            // re-entrantly), so a plain FnMut closure is safe to re-schedule.
            if let Some(c) = pump.borrow().as_ref() {
                raf_id.set(request_animation_frame(c));
            }
        }) as Box<dyn FnMut()>)
    };
    *pump.borrow_mut() = Some(closure);
    raf_id.set(request_animation_frame(pump.borrow().as_ref().unwrap()));

    // Publish the real `web_sys::MediaStream` as the stream's zero-copy
    // native source: a future display consumer can `set_src_object` it (no
    // canvas pump), and a GPU compositor can import it as an external
    // texture. Downcast back to `web_sys::MediaStream` by the web consumer.
    let native: NativeSource = std::rc::Rc::new(stream.clone());

    Ok((
        StreamHandle {
            video,
            stream,
            running,
            raf_id,
            _pump: pump,
        },
        Some(native),
    ))
}

/// Draw the current video frame into the canvas and read it back as
/// `RGBA8`, invoking the callback. A no-op until the video has dimensions
/// (metadata loaded).
fn pump_frame(
    video: &HtmlVideoElement,
    canvas: &HtmlCanvasElement,
    ctx: &CanvasRenderingContext2d,
    writer: &FrameWriter,
) {
    let width = video.video_width();
    let height = video.video_height();
    if width == 0 || height == 0 {
        return;
    }
    if canvas.width() != width {
        canvas.set_width(width);
    }
    if canvas.height() != height {
        canvas.set_height(height);
    }
    if ctx
        .draw_image_with_html_video_element(video, 0.0, 0.0)
        .is_err()
    {
        return;
    }
    let image = match ctx.get_image_data(0.0, 0.0, width as f64, height as f64) {
        Ok(d) => d,
        Err(_) => return,
    };
    // `ImageData::data()` is straight (non-premultiplied) RGBA8, tightly
    // packed — exactly the SDK's frame format.
    let bytes = image.data();
    writer.write_rgba8(width, height, &bytes.0);
}

fn request_animation_frame(f: &Closure<dyn FnMut()>) -> i32 {
    web_sys::window()
        .and_then(|w| w.request_animation_frame(f.as_ref().unchecked_ref()).ok())
        .unwrap_or(0)
}

/// Run `getUserMedia({ video: <constraints> })` and await the resulting
/// `MediaStream`. Maps a rejected promise to the closest [`CameraError`].
async fn get_user_media(config: &CameraConfig) -> Result<MediaStream, CameraError> {
    let window = web_sys::window().ok_or(CameraError::Unsupported)?;
    let devices = window
        .navigator()
        .media_devices()
        .map_err(|_| CameraError::Unsupported)?;

    let constraints = MediaStreamConstraints::new();
    constraints.set_video(&video_constraint(config));

    let promise = devices
        .get_user_media_with_constraints(&constraints)
        .map_err(|e| CameraError::Backend(format!("getUserMedia: {}", err_string(&e))))?;

    let value = JsFuture::from(promise).await.map_err(map_gum_error)?;
    value
        .dyn_into::<MediaStream>()
        .map_err(|_| CameraError::Backend("getUserMedia did not return a MediaStream".into()))
}

/// Build the `video` member of the constraints. `true` for device defaults,
/// or an object carrying explicit `width`/`height`/`frameRate`/`facingMode`
/// the browser treats as preferences.
fn video_constraint(config: &CameraConfig) -> JsValue {
    let facing = match config.facing {
        CameraFacing::Default => None,
        CameraFacing::Front => Some("user"),
        CameraFacing::Back => Some("environment"),
    };
    if config.width.is_none()
        && config.height.is_none()
        && config.fps.is_none()
        && facing.is_none()
    {
        return JsValue::TRUE;
    }
    let obj = js_sys::Object::new();
    if let Some(w) = config.width {
        let _ = Reflect::set(&obj, &"width".into(), &JsValue::from_f64(w as f64));
    }
    if let Some(h) = config.height {
        let _ = Reflect::set(&obj, &"height".into(), &JsValue::from_f64(h as f64));
    }
    if let Some(fps) = config.fps {
        let _ = Reflect::set(&obj, &"frameRate".into(), &JsValue::from_f64(fps as f64));
    }
    if let Some(f) = facing {
        let _ = Reflect::set(&obj, &"facingMode".into(), &JsValue::from_str(f));
    }
    obj.into()
}

/// Map a rejected `getUserMedia` to a [`CameraError`]. The DOMException name
/// distinguishes a user/policy denial from no device / over-constrained.
fn map_gum_error(err: JsValue) -> CameraError {
    let name = Reflect::get(&err, &"name".into())
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default();
    match name.as_str() {
        "NotAllowedError" | "SecurityError" | "PermissionDeniedError" => {
            CameraError::PermissionDenied
        }
        "NotFoundError" | "DevicesNotFoundError" => CameraError::NoCamera,
        "OverconstrainedError" | "ConstraintNotSatisfiedError" => {
            CameraError::UnsupportedConfig(format!("getUserMedia over-constrained: {}", err_string(&err)))
        }
        _ => CameraError::Backend(format!("getUserMedia rejected: {}", err_string(&err))),
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
