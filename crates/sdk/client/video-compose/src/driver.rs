//! The frame driver: subscribes to the input for a size/liveness signal, then on
//! each owner-thread animation frame reads the reactive ops, composites, and
//! publishes to the output stream.
//!
//! # Threading
//!
//! `input.subscribe`'s callback fires on the producer's CAPTURE thread (a camera
//! delivers frames off the main thread) and is `Send`; it must touch nothing
//! `!Send`. So it does the minimum: pack the frame's `(width, height)` into an
//! `AtomicU64`. The wgpu device, the reactive op closures, and the whole
//! [`HeadlessCompositor`] live on the OWNER thread inside the [`raf_loop`]
//! callback, which reads that atomic for the output size and reads the reactive
//! params directly (we poll every frame, so no reactive subscription is needed —
//! and we only ever READ signals, so there's no arena-borrow hazard). See
//! `[[project_reactive_window_one_per_logical_update]]`.
//!
//! `raf_loop` needs a scheduler installed on the owner thread (a mounted app);
//! with none (a bare unit test) it's inert, so tests drive [`Driver::tick`]
//! directly instead.

use crate::Op;
use media_stream::{FrameWriter, MediaStream};

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use native::spawn;

#[cfg(target_arch = "wasm32")]
pub(crate) use web::spawn;

// ---------------------------------------------------------------------------
// Native driver — macOS is the implemented backend (zero-copy in and out);
// other native targets composite through the same path but with no GPU layer
// compositor yet (the base/PiP show through only on macOS for now).
// ---------------------------------------------------------------------------
#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::*;
    use canvas_core::Scene;
    use canvas_vello::HeadlessCompositor;
    use media_stream::Subscription;
    use runtime_shared::scheduling::{raf_loop, RafLoop};
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    /// Keeps the pipeline running: the input subscription (size/liveness signal)
    /// and the animation-frame loop. Dropping it stops both — the compositor and
    /// its GPU device are freed, and the input's CPU readback tap is released.
    pub(crate) struct DriverHandle {
        _sub: Subscription,
        _raf: Option<RafLoop>,
    }

    /// One pipeline's mutable per-frame state, ticked on the owner thread.
    pub(crate) struct Driver {
        input: MediaStream,
        writer: FrameWriter,
        output_size: Option<(u32, u32)>,
        ops: Vec<Op>,
        compositor: HeadlessCompositor,
        /// Latest input `(w << 32 | h)`, written from the capture thread.
        dims: Arc<AtomicU64>,
    }

    impl Driver {
        /// Composite one output frame from the current input + reactive params, and
        /// publish it to the output stream. No-op until the output size is known
        /// (fixed via `output_size`, or once the first input frame reports its size).
        pub(crate) fn tick(&mut self) {
            let packed = self.dims.load(Ordering::Relaxed);
            let (in_w, in_h) = ((packed >> 32) as u32, packed as u32);
            let (out_w, out_h) = match self.output_size {
                Some(s) => s,
                None => {
                    if in_w == 0 || in_h == 0 {
                        return; // no frame yet and no fixed size — nothing to size to
                    }
                    (in_w, in_h)
                }
            };
            // Fall back to the output size for crop normalization if the input's
            // own size isn't known yet (fixed-output-size + first frame race).
            let (in_wf, in_hf) = if in_w > 0 && in_h > 0 {
                (in_w as f32, in_h as f32)
            } else {
                (out_w as f32, out_h as f32)
            };

            let layers =
                crate::build_layers(&self.input, &self.ops, out_w as f32, out_h as f32, in_wf, in_hf);
            let overlay = crate::build_overlay_scene(&self.ops);
            self.compositor
                .composite(&Scene::new(), &layers, &overlay, out_w, out_h, 1.0);

            // Native zero-copy output (macOS); CPU frames only when a consumer taps.
            self.compositor.publish_output();
            if self.writer.wants_cpu_frames() {
                if let Some(img) = self.compositor.read_rgba() {
                    self.writer.write_rgba8(img.width, img.height, &img.data);
                }
            }
        }
    }

    pub(crate) fn spawn(
        input: MediaStream,
        writer: FrameWriter,
        output_size: Option<(u32, u32)>,
        ops: Vec<Op>,
    ) -> DriverHandle {
        // Capture-thread callback: pack the input frame size; touch nothing !Send.
        let dims = Arc::new(AtomicU64::new(0));
        let sub = input.subscribe({
            let dims = dims.clone();
            move |f| dims.store(((f.width as u64) << 32) | f.height as u64, Ordering::Relaxed)
        });

        // No GPU adapter → a live-but-empty output stream (the subscription still
        // holds so the input isn't disturbed). Keeps callers running everywhere.
        let Some(mut compositor) = HeadlessCompositor::new() else {
            return DriverHandle { _sub: sub, _raf: None };
        };
        compositor.attach_output(writer.clone());

        let driver = Rc::new(RefCell::new(Driver {
            input,
            writer,
            output_size,
            ops,
            compositor,
            dims,
        }));

        // Owner-thread frame loop. Inert without an installed scheduler (bare
        // test); a mounted app drives it at the display cadence.
        let raf = raf_loop({
            let driver = driver.clone();
            move || driver.borrow_mut().tick()
        });

        DriverHandle { _sub: sub, _raf: Some(raf) }
    }

    impl Driver {
        /// Build a driver WITHOUT the raf loop (no scheduler in a unit test) so a
        /// test can `tick()` it deterministically. Seeds the input size from the
        /// latest frame. `None` if no GPU adapter is available.
        #[cfg(test)]
        pub(crate) fn for_test(
            input: MediaStream,
            writer: FrameWriter,
            output_size: Option<(u32, u32)>,
            ops: Vec<Op>,
        ) -> Option<Driver> {
            let dims = Arc::new(AtomicU64::new(0));
            let mut buf = Vec::new();
            if let Some((w, h)) = input.latest(&mut buf) {
                dims.store(((w as u64) << 32) | h as u64, Ordering::Relaxed);
            }
            let mut compositor = HeadlessCompositor::new()?;
            compositor.attach_output(writer.clone());
            Some(Driver { input, writer, output_size, ops, compositor, dims })
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::{Corner, Op};
        use canvas_core::ImageSource;
        use std::rc::Rc;
        use std::sync::Arc as StdArc;

        fn fill(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
            let mut v = Vec::with_capacity((w * h * 4) as usize);
            for _ in 0..w * h {
                v.extend_from_slice(&rgba);
            }
            v
        }

        fn px(data: &[u8], w: u32, x: u32, y: u32) -> (u8, u8, u8, u8) {
            let i = ((y * w + x) * 4) as usize;
            (data[i], data[i + 1], data[i + 2], data[i + 3])
        }

        /// The load-bearing invariant: a watermark appears on the OUTPUT stream but
        /// the INPUT stream is never modified. Feeds a solid-blue input, adds a red
        /// watermark, and asserts the output has red at the watermark corner + blue
        /// elsewhere, while a subscriber on the INPUT still sees pure blue.
        #[test]
        fn regression_watermark_on_output_not_input() {
            let (w, h) = (32u32, 32u32);
            let (input, in_writer) = MediaStream::new();

            // Watch the input for any mutation.
            let input_seen: StdArc<std::sync::Mutex<Option<Vec<u8>>>> = StdArc::new(std::sync::Mutex::new(None));
            let _in_sub = input.subscribe({
                let seen = input_seen.clone();
                move |f| *seen.lock().unwrap() = Some(f.data.to_vec())
            });
            in_writer.write_rgba8(w, h, &fill(w, h, [0, 0, 255, 255]));

            let logo = ImageSource::from_rgba8(1, 8, 8, fill(8, 8, [255, 0, 0, 255]));
            let ops = vec![Op::Watermark {
                image: StdArc::new(logo),
                corner: Corner::TopLeft,
                margin: 0.0,
                opacity: Rc::new(|| 1.0),
            }];

            let (out_stream, out_writer) = MediaStream::new();
            let Some(mut driver) = Driver::for_test(input, out_writer, Some((w, h)), ops) else {
                eprintln!("no GPU adapter — skipping");
                return;
            };
            // A CPU subscriber on the OUTPUT makes the driver emit read-back frames.
            let out_seen: StdArc<std::sync::Mutex<Option<(u32, u32, Vec<u8>)>>> =
                StdArc::new(std::sync::Mutex::new(None));
            let _out_sub = out_stream.subscribe({
                let seen = out_seen.clone();
                move |f| *seen.lock().unwrap() = Some((f.width, f.height, f.data.to_vec()))
            });

            driver.tick();

            // OUTPUT: red watermark at the top-left corner, blue base elsewhere.
            let (ow, _oh, out) = out_seen.lock().unwrap().clone().expect("output frame");
            let (r, _g, b, _) = px(&out, ow, 3, 3);
            assert!(r > 200 && b < 60, "watermark red at corner, got r={r} b={b}");
            let (r2, _g2, b2, _) = px(&out, ow, 24, 24);
            assert!(b2 > 200 && r2 < 60, "blue input base away from watermark, got r={r2} b={b2}");

            // INPUT: still pure blue — never touched by the compositor.
            let seen = input_seen.lock().unwrap().clone().expect("input frame seen");
            let (ir, _ig, ib, _) = px(&seen, w, 3, 3);
            assert!(ib > 200 && ir < 60, "INPUT must stay blue (unmodified), got r={ir} b={ib}");
        }

        /// A reactive opacity param is re-read each tick: driving the watermark
        /// opacity to 0 removes it from the output on the next composite.
        #[test]
        fn reactive_opacity_reflected_on_output() {
            let (w, h) = (16u32, 16u32);
            let (input, in_writer) = MediaStream::new();
            in_writer.write_rgba8(w, h, &fill(w, h, [0, 0, 255, 255]));

            let opacity = StdArc::new(std::sync::atomic::AtomicU32::new(1_000));
            let logo = ImageSource::from_rgba8(2, 16, 16, fill(16, 16, [255, 0, 0, 255]));
            let ops = vec![Op::Watermark {
                image: StdArc::new(logo),
                corner: Corner::TopLeft,
                margin: 0.0,
                opacity: {
                    let opacity = opacity.clone();
                    Rc::new(move || opacity.load(Ordering::Relaxed) as f32 / 1000.0)
                },
            }];

            let (out_stream, out_writer) = MediaStream::new();
            let Some(mut driver) = Driver::for_test(input, out_writer, Some((w, h)), ops) else {
                return;
            };
            let out_seen: StdArc<std::sync::Mutex<Option<Vec<u8>>>> = StdArc::new(std::sync::Mutex::new(None));
            let _out_sub = out_stream.subscribe({
                let seen = out_seen.clone();
                move |f| *seen.lock().unwrap() = Some(f.data.to_vec())
            });

            driver.tick();
            let (r1, ..) = px(&out_seen.lock().unwrap().clone().unwrap(), w, 8, 8);
            assert!(r1 > 200, "opaque watermark should be red, got r={r1}");

            // Drop opacity to 0 → watermark vanishes, blue base shows.
            opacity.store(0, Ordering::Relaxed);
            driver.tick();
            let out = out_seen.lock().unwrap().clone().unwrap();
            let (r2, _g2, b2, _) = px(&out, w, 8, 8);
            assert!(r2 < 60 && b2 > 200, "opacity 0 removes the watermark, got r={r2} b={b2}");
        }
    }
}

// ---------------------------------------------------------------------------
// Web driver — a hidden `<canvas>` composites the input `<video>` + watermark +
// PiP via Canvas2D `drawImage`, then `captureStream()` becomes the output
// stream's native source. Drawn-graphics ops (`.draw()`) aren't rendered here
// yet (they'd need a full Canvas2D scene replay, which lives in `canvas-native`).
//
// Every browser call is a web-glue binding (docs/proposals/own-web-bindings.md);
// the input's / PiP's native source and the published output are
// `web_glue::dom::MediaStream`s — the type every media SDK publishes.
// ---------------------------------------------------------------------------
#[cfg(target_arch = "wasm32")]
mod web {
    use super::*;
    use crate::{normalized_crop, watermark_rect, Corner};
    use canvas_core::Fit;
    use runtime_shared::scheduling::{raf_loop, RafLoop};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use web_glue::dom::MediaStream as WebMediaStream;
    use web_glue::{Closure, JsCast, JsValue};

    /// Style that parks the capture canvas off-screen: some browsers only
    /// drive `captureStream` for a canvas in the document, and `display:none`
    /// can suspend painting.
    const OFFSCREEN_STYLE: &str =
        "position:absolute;left:-99999px;top:0;width:1px;height:1px;pointer-events:none;";
    /// Output size until the input reports one (and no fixed size was asked).
    const DEFAULT_OUTPUT: (u32, u32) = (640, 480);

    web_glue::import! {
        // A detached, muted, autoplaying, inline `<video>` — a frame source.
        fn js_new_video() -> u32 =
            "() => { const v = document.createElement('video'); v.muted = true; v.autoplay = true; \
               v.setAttribute('playsinline', ''); return G.add(v); }";
        // `srcObject = s; play()` (the Promise is observed; a rejected play
        // just leaves the video without frames, which every reader tolerates).
        fn js_video_attach(v: u32, s: u32) =
            "(v, s) => { const e = G.get(v); e.srcObject = G.get(s); \
               const p = e.play(); if (p) p.catch(() => {}); }";
        fn js_video_width(v: u32) -> u32 = "(v) => G.get(v).videoWidth >>> 0";
        fn js_video_height(v: u32) -> u32 = "(v) => G.get(v).videoHeight >>> 0";
        // `requestVideoFrameCallback(f)` → its handle, or -1 when the
        // browser lacks rVFC (the caller then falls back to the raf driver).
        #[catch]
        fn js_request_vfc(v: u32, f: u32) -> f64 =
            "(v, f) => { const e = G.get(v); \
               return typeof e.requestVideoFrameCallback === 'function' \
                 ? e.requestVideoFrameCallback(G.get(f)) : -1; }";
        fn js_cancel_vfc(v: u32, h: f64) =
            "(v, h) => { const e = G.get(v); \
               if (typeof e.cancelVideoFrameCallback === 'function') e.cancelVideoFrameCallback(h); }";
        // The capture `<canvas>` (w×h, parked off-screen in <body>) and its
        // 2D context, as `{ canvas, ctx }`; throws if there is no document /
        // context.
        #[catch]
        fn js_new_capture_canvas(w: u32, h: u32, sp: usize, sl: usize) -> u32 =
            "(w, h, sp, sl) => { const c = document.createElement('canvas'); c.width = w; c.height = h; \
               c.setAttribute('style', G.str(sp, sl)); if (document.body) document.body.appendChild(c); \
               const x = c.getContext('2d'); if (x == null) throw new Error('no 2d context'); \
               return G.add({ canvas: c, ctx: x }); }";
        // `captureStream(0)` (manual mode: one frame per `requestFrame`) as
        // `{ stream, track }`; throws when unsupported / trackless.
        #[catch]
        fn js_capture(o: u32) -> u32 =
            "(o) => { const s = G.get(o).canvas.captureStream(0); const t = s.getVideoTracks()[0]; \
               if (t == null || typeof t.requestFrame !== 'function') throw new Error('no capture track'); \
               return G.add({ stream: s, track: t }); }";
        fn js_get(o: u32, k: u32) -> u32 = "(o, k) => G.add(k === 0 ? G.get(o).stream : G.get(o).track)";
        fn js_request_frame(t: u32) = "(t) => { G.get(t).requestFrame(); }";
        // Resize the capture canvas when the output size changes, then clear.
        fn js_begin_frame(o: u32, w: u32, h: u32) =
            "(o, w, h) => { const { canvas: c, ctx: x } = G.get(o); \
               if (c.width !== w) c.width = w; if (c.height !== h) c.height = h; x.clearRect(0, 0, w, h); }";
        fn js_set_alpha(o: u32, a: f64) = "(o, a) => { G.get(o).ctx.globalAlpha = a; }";
        // `drawImage(src, sx, sy, sw, sh, dx, dy, dw, dh)`; a throw (a source
        // with no decodable frame yet) skips the draw.
        fn js_draw_src_dst(o: u32, s: u32, sx: f64, sy: f64, sw: f64, sh: f64, dx: f64, dy: f64, dw: f64, dh: f64) =
            "(o, s, sx, sy, sw, sh, dx, dy, dw, dh) => { \
               try { G.get(o).ctx.drawImage(G.get(s), sx, sy, sw, sh, dx, dy, dw, dh); } catch (_) {} }";
        // `drawImage(src, dx, dy, dw, dh)`.
        fn js_draw_dst(o: u32, s: u32, dx: f64, dy: f64, dw: f64, dh: f64) =
            "(o, s, dx, dy, dw, dh) => { try { G.get(o).ctx.drawImage(G.get(s), dx, dy, dw, dh); } catch (_) {} }";
        fn js_remove_canvas(o: u32) = "(o) => { G.get(o).canvas.remove(); }";
        // A fresh w×h `<canvas>` holding the w*h*4 straight RGBA8 bytes at
        // `p` (copied in with `putImageData`); throws on failure.
        #[catch]
        fn js_image_canvas(p: usize, l: usize, w: u32, h: u32) -> u32 =
            "(p, l, w, h) => { const c = document.createElement('canvas'); c.width = w; c.height = h; \
               const x = c.getContext('2d'); if (x == null) throw new Error('no 2d context'); \
               const px = new Uint8ClampedArray(G.u8().slice(p >>> 0, (p >>> 0) + (l >>> 0)).buffer); \
               x.putImageData(new ImageData(px, w, h), 0, 0); return G.add(c); }";
    }

    /// A detached `<video>` element.
    #[derive(Clone)]
    struct Video(JsValue);

    impl Video {
        fn new() -> Video {
            // SAFETY: a fresh element handle.
            Video(unsafe { JsValue::from_raw(js_new_video()) })
        }
        fn attach(&self, stream: &WebMediaStream) {
            // SAFETY: live handles.
            unsafe { js_video_attach(self.0.raw(), stream.as_js().raw()) }
        }
        fn size(&self) -> (u32, u32) {
            // SAFETY: a live element handle.
            unsafe { (js_video_width(self.0.raw()), js_video_height(self.0.raw())) }
        }
        /// `requestVideoFrameCallback(f)`; `None` without rVFC.
        fn request_vfc(&self, f: &Closure) -> Option<f64> {
            // SAFETY: live handles.
            match unsafe { js_request_vfc(self.0.raw(), f.as_js().raw()) } {
                Ok(h) if h >= 0.0 => Some(h),
                _ => None,
            }
        }
    }

    pub(crate) struct DriverHandle {
        _raf: Option<RafLoop>,
        _rvfc: Option<RvfcState>,
    }

    /// Keeps a `requestVideoFrameCallback` loop alive: the self-re-arming closure
    /// and the latest pending handle. Dropping it cancels the pending callback and
    /// releases the closure (breaking the closure↔slot cycle), stopping the loop.
    struct RvfcState {
        video: Video,
        handle: Rc<Cell<f64>>,
        cb: Rc<RefCell<Option<Closure>>>,
    }

    impl Drop for RvfcState {
        fn drop(&mut self) {
            // SAFETY: a live element handle.
            unsafe { js_cancel_vfc(self.video.0.raw(), self.handle.get()) };
            self.cb.borrow_mut().take();
        }
    }

    /// One resolved op with its web resources (built once; only params reactive).
    enum WebOp {
        Crop { fit: Fit, rect: Rc<dyn Fn() -> (f32, f32, f32, f32)> },
        Watermark {
            canvas: JsValue,
            w: f32,
            h: f32,
            corner: Corner,
            margin: f32,
            opacity: Rc<dyn Fn() -> f32>,
        },
        Overlay {
            stream: MediaStream,
            video: Video,
            id: RefCell<Option<String>>,
            rect: Rc<dyn Fn() -> (f32, f32, f32, f32)>,
        },
    }

    struct Driver {
        input: MediaStream,
        input_video: Video,
        input_id: RefCell<Option<String>>,
        /// `{ canvas, ctx }` — the capture canvas and its 2D context.
        surface: JsValue,
        /// The capture's `CanvasCaptureMediaStreamTrack`.
        track: JsValue,
        ops: Vec<WebOp>,
        output_size: Option<(u32, u32)>,
    }

    /// Attach `stream`'s web `MediaStream` to `video` (only when the id changes),
    /// keeping a detached element playing as a frame source.
    fn ensure_srcobject(video: &Video, id_cell: &RefCell<Option<String>>, stream: &MediaStream) -> bool {
        let Some(ms) = stream
            .native_source()
            .and_then(|rc| rc.downcast::<WebMediaStream>().ok())
        else {
            return false;
        };
        let id = ms.id();
        if id_cell.borrow().as_deref() != Some(id.as_str()) {
            video.attach(&ms);
            *id_cell.borrow_mut() = Some(id);
        }
        true
    }

    /// Paint an `ImageSource`'s RGBA into a fresh offscreen `<canvas>` (once).
    fn image_to_canvas(img: &canvas_core::ImageSource) -> Option<JsValue> {
        let rgba = img.rgba.as_slice();
        if rgba.len() != img.width as usize * img.height as usize * 4 {
            return None;
        }
        // SAFETY: `rgba` is live for the call; the result is a fresh handle.
        let c = unsafe { js_image_canvas(rgba.as_ptr() as usize, rgba.len(), img.width, img.height) }.ok()?;
        Some(unsafe { JsValue::from_raw(c) })
    }

    impl Drop for Driver {
        fn drop(&mut self) {
            // Remove the off-screen capture canvas when the pipeline stops (the
            // output stream was dropped) so it doesn't linger in the document.
            // SAFETY: a live handle.
            unsafe { js_remove_canvas(self.surface.raw()) };
        }
    }

    impl Driver {
        /// Attach the input stream to the `<video>` (idempotent). Returns whether a
        /// stream is attached — the rVFC path needs this true before its first frame
        /// can present.
        fn ensure_input(&self) -> bool {
            ensure_srcobject(&self.input_video, &self.input_id, &self.input)
        }

        /// The raf-driven tick (fallback path): attach the input, then composite +
        /// emit one frame. On the rVFC path the input is attached at spawn and only
        /// `render_and_emit` runs, once per presented frame.
        fn tick(&self) {
            // Feed the input `<video>` FIRST — before any early-return — or it
            // never gets its stream, so it never reports a size, so we'd bail here
            // forever (a deadlock: no size ⇒ no draw ⇒ no source set).
            let _ = self.ensure_input();
            self.render_and_emit();
        }

        fn draw_src_dst(&self, src: &JsValue, s: (f32, f32, f32, f32), d: (f32, f32, f32, f32)) {
            // SAFETY: live handles.
            unsafe {
                js_draw_src_dst(
                    self.surface.raw(),
                    src.raw(),
                    s.0 as f64, s.1 as f64, s.2 as f64, s.3 as f64,
                    d.0 as f64, d.1 as f64, d.2 as f64, d.3 as f64,
                )
            }
        }

        /// Composite the current input frame + ops into the output canvas and pin
        /// one captured frame. Assumes the input `<video>` is already attached.
        fn render_and_emit(&self) {
            // Output size: fixed, or the input video's intrinsic size once known.
            let (iw, ih) = self.input_video.size();
            let (out_w, out_h) = match self.output_size {
                Some(s) => s,
                None => {
                    if iw == 0 || ih == 0 {
                        return;
                    }
                    (iw, ih)
                }
            };
            // SAFETY: a live handle.
            unsafe { js_begin_frame(self.surface.raw(), out_w, out_h) };

            // Base input, cropped/fit into the whole output.
            if iw > 0 && ih > 0 {
                let (fit, crop) = self
                    .ops
                    .iter()
                    .find_map(|op| match op {
                        WebOp::Crop { fit, rect } => {
                            Some((*fit, normalized_crop(rect(), iw as f32, ih as f32)))
                        }
                        _ => None,
                    })
                    .unwrap_or((Fit::Cover, None));
                let (src, dst) = match crop {
                    // A normalized crop selects the source region; fill the output.
                    Some((cx, cy, cw, ch)) => (
                        (cx * iw as f32, cy * ih as f32, cw * iw as f32, ch * ih as f32),
                        (0.0, 0.0, out_w as f32, out_h as f32),
                    ),
                    None => fit.map_rects(iw as f32, ih as f32, 0.0, 0.0, out_w as f32, out_h as f32),
                };
                self.draw_src_dst(&self.input_video.0, src, dst);
            }

            // Watermark + PiP layers, in order, over the base.
            for op in &self.ops {
                match op {
                    WebOp::Watermark { canvas, w, h, corner, margin, opacity } => {
                        let (x, y, ww, hh) =
                            watermark_rect(*corner, *margin, *w, *h, out_w as f32, out_h as f32);
                        // SAFETY (below): live handles.
                        unsafe {
                            js_set_alpha(self.surface.raw(), opacity().clamp(0.0, 1.0) as f64);
                            js_draw_dst(self.surface.raw(), canvas.raw(), x as f64, y as f64, ww as f64, hh as f64);
                            js_set_alpha(self.surface.raw(), 1.0);
                        }
                    }
                    WebOp::Overlay { stream, video, id, rect } => {
                        if !ensure_srcobject(video, id, stream) {
                            continue;
                        }
                        let (pw, ph) = video.size();
                        if pw == 0 || ph == 0 {
                            continue;
                        }
                        let (dx, dy, dw, dh) = rect();
                        let (src, dst) = Fit::Cover.map_rects(pw as f32, ph as f32, dx, dy, dw, dh);
                        self.draw_src_dst(&video.0, src, dst);
                    }
                    WebOp::Crop { .. } => {}
                }
            }

            // Pin one captured frame to this render (manual `captureStream`).
            // SAFETY: a live track handle.
            unsafe { js_request_frame(self.track.raw()) };
        }
    }

    pub(crate) fn spawn(
        input: MediaStream,
        writer: FrameWriter,
        output_size: Option<(u32, u32)>,
        ops: Vec<Op>,
    ) -> DriverHandle {
        let inert = || DriverHandle { _raf: None, _rvfc: None };
        let (w, h) = output_size.unwrap_or(DEFAULT_OUTPUT);
        let (sp, sl) = web_glue::string::abi(OFFSCREEN_STYLE);
        // SAFETY: the result is a fresh handle.
        let Ok(surface) = (unsafe { js_new_capture_canvas(w, h, sp, sl) }) else {
            return inert();
        };
        let surface = unsafe { JsValue::from_raw(surface) };
        // `captureStream` in manual mode: one frame per `request_frame` (a fixed
        // auto rate under-delivers). Publish it as the output stream's source.
        // SAFETY: a live handle.
        let Ok(capture) = (unsafe { js_capture(surface.raw()) }) else {
            // SAFETY: a live handle (the canvas must not linger).
            unsafe { js_remove_canvas(surface.raw()) };
            return inert();
        };
        let capture = unsafe { JsValue::from_raw(capture) };
        // SAFETY (both): a live `{ stream, track }` handle.
        let stream: WebMediaStream = unsafe { JsValue::from_raw(js_get(capture.raw(), 0)) }.unchecked_into();
        let track = unsafe { JsValue::from_raw(js_get(capture.raw(), 1)) };
        writer.publish_native_source(Rc::new(stream));

        // Resolve each op's web resources once (params stay reactive).
        let web_ops = ops
            .into_iter()
            .filter_map(|op| match op {
                Op::Crop { fit, rect } => Some(WebOp::Crop { fit, rect }),
                Op::Watermark { image, corner, margin, opacity } => {
                    let canvas = image_to_canvas(&image)?;
                    Some(WebOp::Watermark {
                        canvas,
                        w: image.width as f32,
                        h: image.height as f32,
                        corner,
                        margin,
                        opacity,
                    })
                }
                Op::Overlay { stream, rect, corner_radius: _ } => Some(WebOp::Overlay {
                    stream,
                    video: Video::new(),
                    id: RefCell::new(None),
                    rect,
                }),
                // Drawn graphics aren't rendered on web yet.
                Op::Draw(_) => None,
            })
            .collect();

        let driver = Rc::new(Driver {
            input,
            input_video: Video::new(),
            input_id: RefCell::new(None),
            surface,
            track,
            ops: web_ops,
            output_size,
        });

        // Prefer `requestVideoFrameCallback`: emit exactly one output frame per REAL
        // presented input frame, so the recording's cadence locks to the input's
        // true framerate (no free-running-raf resampling → no duplicated/dropped
        // frames = smooth capture). Requires the input stream to be attachable now
        // (rVFC won't fire until the `<video>` plays) AND the browser to support rVFC
        // (`request_vfc` returns `None` otherwise). Either miss falls through to the
        // raf driver below (older browsers, or a stream not yet published).
        if driver.ensure_input() {
            let video = driver.input_video.clone();
            let handle = Rc::new(Cell::new(0.0f64));
            let cb: Rc<RefCell<Option<Closure>>> = Rc::new(RefCell::new(None));
            let closure = {
                let driver = driver.clone();
                let video = video.clone();
                let cb_slot = cb.clone();
                let handle = handle.clone();
                Closure::new(move |_now: JsValue| {
                    driver.render_and_emit();
                    // rVFC is one-shot — re-arm for the next presented frame.
                    if let Some(cb) = cb_slot.borrow().as_ref() {
                        if let Some(h) = video.request_vfc(cb) {
                            handle.set(h);
                        }
                    }
                })
            };
            if let Some(h) = video.request_vfc(&closure) {
                handle.set(h);
                *cb.borrow_mut() = Some(closure);
                return DriverHandle {
                    _raf: None,
                    _rvfc: Some(RvfcState { video, handle, cb }),
                };
            }
            // rVFC unsupported — drop the closure and fall through to the raf driver.
        }

        let raf = raf_loop(move || driver.tick());
        DriverHandle { _raf: Some(raf), _rvfc: None }
    }
}
