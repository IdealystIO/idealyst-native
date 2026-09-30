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
//!
//! Every browser call goes through web-glue, the framework-owned JS boundary
//! (docs/proposals/own-web-bindings.md). The capture stream is published as
//! the stream's `native_source` as a `web_glue::dom::MediaStream` — THE
//! capture stream, not a copy (see [`open`]).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use web_glue::dom::{MediaStream, MediaStreamTrack};
use web_glue::js::Object;
use web_glue::{Closure, JsCast, JsError, JsFuture, JsValue};

use crate::{CameraConfig, CameraError, CameraFacing, NativeSource};
use media_stream::FrameWriter;

web_glue::import! {
    // `navigator.mediaDevices`, 0 when absent (insecure context, old engine).
    fn js_media_devices() -> u32 =
        "() => { const n = typeof navigator === 'undefined' ? null : navigator; \
           const m = n == null ? null : n.mediaDevices; return m == null ? 0 : G.add(m); }";
    // `mediaDevices.getUserMedia(constraints)` → its Promise.
    #[catch]
    fn js_get_user_media(md: u32, c: u32) -> u32 = "(m, c) => G.add(G.get(m).getUserMedia(G.get(c)))";
    // A detached, muted, `playsinline` `<video>` playing `s` — never inserted
    // into the DOM. `play()`'s Promise is observed here: the pump waits for
    // non-zero dimensions before sampling, so a rejected play is harmless.
    #[catch]
    fn js_video_for(s: u32) -> u32 =
        "(s) => { const v = document.createElement('video'); v.muted = true; \
           v.setAttribute('playsinline', ''); v.srcObject = G.get(s); \
           const p = v.play(); if (p) p.catch(() => {}); return G.add(v); }";
    fn js_video_width(v: u32) -> u32 = "(v) => G.get(v).videoWidth >>> 0";
    fn js_video_height(v: u32) -> u32 = "(v) => G.get(v).videoHeight >>> 0";
    // Detach the source.
    fn js_video_release(v: u32) = "(v) => { const e = G.get(v); e.pause(); e.srcObject = null; }";
    // An offscreen canvas's 2D context. `willReadFrequently` keeps the
    // backing store CPU-side: every frame is read back with `getImageData`,
    // so this avoids a per-readback GPU→CPU stall (and the browser's
    // "Multiple readback operations" warning).
    #[catch]
    fn js_create_ctx() -> u32 =
        "() => { const c = document.createElement('canvas'); \
           const x = c.getContext('2d', { willReadFrequently: true }); \
           if (x == null) throw new Error('no 2d context'); return G.add(x); }";
    // Draw the video's current frame at its native w×h and copy the
    // straight RGBA8 `ImageData` into the w*h*4 bytes at `out`. 1 on
    // success, 0 if the draw / readback threw.
    fn js_pump(x: u32, v: u32, w: u32, h: u32, out: usize) -> u32 =
        "(x, v, w, h, o) => { const ctx = G.get(x); const c = ctx.canvas; \
           if (c.width !== w) c.width = w; if (c.height !== h) c.height = h; \
           try { ctx.drawImage(G.get(v), 0, 0); \
             G.u8().set(ctx.getImageData(0, 0, w, h).data, o >>> 0); return 1; } \
           catch (_) { return 0; } }";
}

/// The self-rescheduling rAF closure, held in an `Rc<RefCell<Option<…>>>`
/// so it can re-arm itself by reference each frame.
type PumpClosure = Rc<RefCell<Option<Closure>>>;

/// Keeps the capture pump and its media tracks alive. Drop stops the pump
/// and releases the camera (clearing the browser's recording indicator).
pub(crate) struct StreamHandle {
    video: JsValue,
    stream: MediaStream,
    running: Rc<Cell<bool>>,
    raf_id: Rc<Cell<i32>>,
    // Owns the rAF closure (and, inside it, the frame writer) for the pump's
    // lifetime. The closure holds a clone of this `Rc` to re-arm itself, so
    // `Drop` takes it out to break that cycle.
    pump: PumpClosure,
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        self.running.set(false);
        if let Some(win) = web_glue::dom::window() {
            win.cancel_animation_frame(self.raf_id.get());
        }
        // Detach the source and stop every track — of THE capture stream,
        // which is also what the published native source is, so a consumer
        // showing it sees its tracks end.
        // SAFETY: a live element handle.
        unsafe { js_video_release(self.video.raw()) };
        stop_tracks(&self.stream);
        self.pump.borrow_mut().take();
    }
}

fn stop_tracks(stream: &MediaStream) {
    for track in stream.get_tracks().iter() {
        track.unchecked_into::<MediaStreamTrack>().stop();
    }
}

pub(crate) async fn request_permission() -> Result<(), CameraError> {
    // Delegated to the shared `permissions` SDK. On web the browser has no
    // explicit camera-request API — the prompt fires on the first
    // `getUserMedia` (which `open()` calls). `permissions::request` honestly
    // reports the queried `navigator.permissions` state without prompting; a
    // `Denied` is a real denial, while `Undetermined`/`Unsupported` ("will
    // prompt on first use") map to `Ok(())` so a caller doesn't treat
    // "not yet decided" as a failure before `open()` ever runs. See the
    // `permissions` web backend docs.
    let status = permissions::request(permissions::Permission::Camera).await;
    if matches!(status, permissions::PermissionStatus::Denied) {
        Err(CameraError::PermissionDenied)
    } else {
        Ok(())
    }
}

pub(crate) async fn open(
    config: CameraConfig,
    writer: FrameWriter,
) -> Result<(StreamHandle, Option<NativeSource>), CameraError> {
    let stream = get_user_media(&config).await?;

    // SAFETY: a live stream handle; the result is a fresh element handle.
    let video = unsafe { js_video_for(stream.as_js().raw()) }
        .map(|v| unsafe { JsValue::from_raw(v) })
        .map_err(|e| CameraError::Backend(format!("create video: {}", err_string(&e))))?;
    // SAFETY: the result is a fresh context handle.
    let ctx = unsafe { js_create_ctx() }
        .map(|c| unsafe { JsValue::from_raw(c) })
        .map_err(|e| CameraError::Backend(format!("get 2d context: {}", err_string(&e))))?;

    let running = Rc::new(Cell::new(true));
    let raf_id = Rc::new(Cell::new(0));
    let pump: PumpClosure = Rc::new(RefCell::new(None));

    let closure = {
        let video = video.clone();
        let running = running.clone();
        let raf_id = raf_id.clone();
        let pump = pump.clone();
        let mut frame: Vec<u8> = Vec::new();
        Closure::new(move |_: JsValue| {
            if !running.get() {
                return;
            }
            // Display goes through `<video>.srcObject` (zero-copy); the canvas
            // readback in `pump_frame` exists only to feed the CPU RGBA channel.
            // Its `getImageData` is a GPU→CPU readback that stalls the wgpu
            // graphics surface, so skip it unless a consumer is tapping CPU
            // frames (a `subscribe`r). See `FrameWriter::wants_cpu_frames`.
            if writer.wants_cpu_frames() {
                pump_frame(&video, &ctx, &writer, &mut frame);
            }
            // Re-arm for the next display frame. rAF calls us later (not
            // re-entrantly), so a plain FnMut closure is safe to re-schedule.
            if let Some(c) = pump.borrow().as_ref() {
                raf_id.set(request_animation_frame(c));
            }
        })
    };
    raf_id.set(request_animation_frame(&closure));
    *pump.borrow_mut() = Some(closure);

    // Publish THE capture stream as the stream's zero-copy native source: a
    // display consumer sets it as `srcObject` (no canvas pump), a GPU
    // compositor imports it as an external texture, media-writer records its
    // tracks. The handle clone is the same JS object, so when the handle
    // below stops the tracks, every consumer's view ends with them. (The
    // web-sys port published `Rc::new(stream.clone())` — web-sys's inherent
    // `clone()` is the JS `MediaStream.clone()`, a NEW stream with cloned
    // tracks that stopping the capture left running.)
    let native: NativeSource = Rc::new(stream.clone());

    Ok((
        StreamHandle {
            video,
            stream,
            running,
            raf_id,
            pump,
        },
        Some(native),
    ))
}

/// Draw the current video frame into the canvas and read it back as
/// `RGBA8`, invoking the callback. A no-op until the video has dimensions
/// (metadata loaded).
fn pump_frame(video: &JsValue, ctx: &JsValue, writer: &FrameWriter, frame: &mut Vec<u8>) {
    // SAFETY: live element handle.
    let (width, height) = unsafe { (js_video_width(video.raw()), js_video_height(video.raw())) };
    if width == 0 || height == 0 {
        return;
    }
    frame.resize(width as usize * height as usize * 4, 0);
    // SAFETY: live handles; `frame` holds exactly width*height*4 bytes.
    if unsafe { js_pump(ctx.raw(), video.raw(), width, height, frame.as_mut_ptr() as usize) } == 0 {
        return;
    }
    // `ImageData.data` is straight (non-premultiplied) RGBA8, tightly
    // packed — exactly the SDK's frame format.
    writer.write_rgba8(width, height, frame);
}

fn request_animation_frame(f: &Closure) -> i32 {
    web_glue::dom::window().map(|w| w.request_animation_frame(f)).unwrap_or(0)
}

/// Run `getUserMedia({ video: <constraints> })` and await the resulting
/// `MediaStream`. Maps a rejected promise to the closest [`CameraError`].
async fn get_user_media(config: &CameraConfig) -> Result<MediaStream, CameraError> {
    // SAFETY: the result is 0 or a fresh handle.
    let devices = match unsafe { js_media_devices() } {
        0 => return Err(CameraError::Unsupported),
        h => unsafe { JsValue::from_raw(h) },
    };

    let constraints = Object::new();
    let _ = constraints.set("video", &video_constraint(config));

    // SAFETY: live handles; the result is a fresh Promise handle.
    let promise = unsafe { js_get_user_media(devices.raw(), constraints.as_js().raw()) }
        .map(|p| unsafe { JsValue::from_raw(p) })
        .map_err(|e| CameraError::Backend(format!("getUserMedia: {}", err_string(&e))))?;

    let value = JsFuture::new(&promise).await.map_err(map_gum_error)?;
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
        return JsValue::from_bool(true);
    }
    let obj = Object::new();
    if let Some(w) = config.width {
        let _ = obj.set("width", &JsValue::from_f64(w as f64));
    }
    if let Some(h) = config.height {
        let _ = obj.set("height", &JsValue::from_f64(h as f64));
    }
    if let Some(fps) = config.fps {
        let _ = obj.set("frameRate", &JsValue::from_f64(fps as f64));
    }
    if let Some(f) = facing {
        let _ = obj.set("facingMode", &JsValue::from_str(f));
    }
    obj.into()
}

/// Map a rejected `getUserMedia` to a [`CameraError`]. The DOMException name
/// distinguishes a user/policy denial from no device / over-constrained.
fn map_gum_error(err: JsError) -> CameraError {
    let name = err.get("name").ok().and_then(|v| v.as_string()).unwrap_or_default();
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
        .or_else(|| value.get("message").ok().and_then(|v| v.as_string()))
        .unwrap_or_else(|| format!("{value:?}"))
}
