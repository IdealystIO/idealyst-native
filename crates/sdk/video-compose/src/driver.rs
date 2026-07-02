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
    use runtime_core::{raf_loop, RafLoop};
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
// ---------------------------------------------------------------------------
#[cfg(target_arch = "wasm32")]
mod web {
    use super::*;
    use crate::{normalized_crop, watermark_rect, Corner};
    use canvas_core::Fit;
    use runtime_core::{raf_loop, RafLoop};
    use std::cell::RefCell;
    use std::rc::Rc;
    use wasm_bindgen::{Clamped, JsCast};
    use web_sys::{
        CanvasCaptureMediaStreamTrack, CanvasRenderingContext2d, Document, HtmlCanvasElement,
        HtmlVideoElement, ImageData, MediaStream as WebMediaStream,
    };

    pub(crate) struct DriverHandle {
        _raf: Option<RafLoop>,
    }

    /// One resolved op with its web resources (built once; only params reactive).
    enum WebOp {
        Crop { fit: Fit, rect: Rc<dyn Fn() -> (f32, f32, f32, f32)> },
        Watermark {
            canvas: HtmlCanvasElement,
            w: f32,
            h: f32,
            corner: Corner,
            margin: f32,
            opacity: Rc<dyn Fn() -> f32>,
        },
        Overlay {
            stream: MediaStream,
            video: HtmlVideoElement,
            id: RefCell<Option<String>>,
            rect: Rc<dyn Fn() -> (f32, f32, f32, f32)>,
        },
    }

    struct Driver {
        input: MediaStream,
        input_video: HtmlVideoElement,
        input_id: RefCell<Option<String>>,
        canvas: HtmlCanvasElement,
        ctx: CanvasRenderingContext2d,
        track: CanvasCaptureMediaStreamTrack,
        ops: Vec<WebOp>,
        output_size: Option<(u32, u32)>,
    }

    /// Attach `stream`'s web `MediaStream` to `video` (only when the id changes),
    /// keeping a detached element playing as a frame source.
    fn ensure_srcobject(
        video: &HtmlVideoElement,
        id_cell: &RefCell<Option<String>>,
        stream: &MediaStream,
    ) -> bool {
        let Some(ms) = stream
            .native_source()
            .and_then(|rc| rc.downcast::<WebMediaStream>().ok())
        else {
            return false;
        };
        let id = ms.id();
        if id_cell.borrow().as_deref() != Some(id.as_str()) {
            video.set_src_object(Some(&ms));
            let _ = video.play();
            *id_cell.borrow_mut() = Some(id);
        }
        true
    }

    fn new_video(doc: &Document) -> HtmlVideoElement {
        let v: HtmlVideoElement = doc
            .create_element("video")
            .expect("create video")
            .dyn_into()
            .expect("video cast");
        v.set_muted(true);
        v.set_autoplay(true);
        let _ = v.set_attribute("playsinline", "");
        v
    }

    /// Paint an `ImageSource`'s RGBA into a fresh offscreen `<canvas>` (once).
    fn image_to_canvas(doc: &Document, img: &canvas_core::ImageSource) -> Option<HtmlCanvasElement> {
        let canvas: HtmlCanvasElement = doc.create_element("canvas").ok()?.dyn_into().ok()?;
        canvas.set_width(img.width);
        canvas.set_height(img.height);
        let ctx: CanvasRenderingContext2d = canvas.get_context("2d").ok()??.dyn_into().ok()?;
        let data = ImageData::new_with_u8_clamped_array_and_sh(
            Clamped(img.rgba.as_slice()),
            img.width,
            img.height,
        )
        .ok()?;
        ctx.put_image_data(&data, 0.0, 0.0).ok()?;
        Some(canvas)
    }

    impl Drop for Driver {
        fn drop(&mut self) {
            // Remove the off-screen capture canvas when the pipeline stops (the
            // output stream was dropped) so it doesn't linger in the document.
            self.canvas.remove();
        }
    }

    impl Driver {
        fn tick(&self) {
            // Feed the input `<video>` FIRST — before any early-return — or it
            // never gets its stream, so it never reports a size, so we'd bail here
            // forever (a deadlock: no size ⇒ no draw ⇒ no source set).
            let _ = ensure_srcobject(&self.input_video, &self.input_id, &self.input);

            // Output size: fixed, or the input video's intrinsic size once known.
            let (iw, ih) = (self.input_video.video_width(), self.input_video.video_height());
            let (out_w, out_h) = match self.output_size {
                Some(s) => s,
                None => {
                    if iw == 0 || ih == 0 {
                        return;
                    }
                    (iw, ih)
                }
            };
            if self.canvas.width() != out_w {
                self.canvas.set_width(out_w);
            }
            if self.canvas.height() != out_h {
                self.canvas.set_height(out_h);
            }
            let (ow, oh) = (out_w as f64, out_h as f64);
            self.ctx.clear_rect(0.0, 0.0, ow, oh);

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
                let _ = self
                    .ctx
                    .draw_image_with_html_video_element_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
                        &self.input_video,
                        src.0 as f64, src.1 as f64, src.2 as f64, src.3 as f64,
                        dst.0 as f64, dst.1 as f64, dst.2 as f64, dst.3 as f64,
                    );
            }

            // Watermark + PiP layers, in order, over the base.
            for op in &self.ops {
                match op {
                    WebOp::Watermark { canvas, w, h, corner, margin, opacity } => {
                        let (x, y, ww, hh) =
                            watermark_rect(*corner, *margin, *w, *h, out_w as f32, out_h as f32);
                        self.ctx.set_global_alpha(opacity().clamp(0.0, 1.0) as f64);
                        let _ = self
                            .ctx
                            .draw_image_with_html_canvas_element_and_dw_and_dh(
                                canvas, x as f64, y as f64, ww as f64, hh as f64,
                            );
                        self.ctx.set_global_alpha(1.0);
                    }
                    WebOp::Overlay { stream, video, id, rect } => {
                        if !ensure_srcobject(video, id, stream) {
                            continue;
                        }
                        let (pw, ph) = (video.video_width(), video.video_height());
                        if pw == 0 || ph == 0 {
                            continue;
                        }
                        let (dx, dy, dw, dh) = rect();
                        let (src, dst) =
                            Fit::Cover.map_rects(pw as f32, ph as f32, dx, dy, dw, dh);
                        let _ = self
                            .ctx
                            .draw_image_with_html_video_element_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
                                video,
                                src.0 as f64, src.1 as f64, src.2 as f64, src.3 as f64,
                                dst.0 as f64, dst.1 as f64, dst.2 as f64, dst.3 as f64,
                            );
                    }
                    WebOp::Crop { .. } => {}
                }
            }

            // Pin one captured frame to this render (manual `captureStream`).
            self.track.request_frame();
        }
    }

    pub(crate) fn spawn(
        input: MediaStream,
        writer: FrameWriter,
        output_size: Option<(u32, u32)>,
        ops: Vec<Op>,
    ) -> DriverHandle {
        let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
            return DriverHandle { _raf: None };
        };
        let Some(canvas) = doc
            .create_element("canvas")
            .ok()
            .and_then(|e| e.dyn_into::<HtmlCanvasElement>().ok())
        else {
            return DriverHandle { _raf: None };
        };
        let (w, h) = output_size.unwrap_or((640, 480));
        canvas.set_width(w);
        canvas.set_height(h);
        // Some browsers only drive `captureStream` for a canvas in the document.
        // Park it off-screen (not `display:none`, which can suspend painting).
        let _ = canvas.set_attribute(
            "style",
            "position:absolute;left:-99999px;top:0;width:1px;height:1px;pointer-events:none;",
        );
        if let Some(body) = doc.body() {
            let _ = body.append_child(&canvas);
        }
        let Some(ctx) = canvas
            .get_context("2d")
            .ok()
            .flatten()
            .and_then(|c| c.dyn_into::<CanvasRenderingContext2d>().ok())
        else {
            return DriverHandle { _raf: None };
        };
        // `captureStream` in manual mode: one frame per `request_frame` (a fixed
        // auto rate under-delivers). Publish it as the output stream's source.
        let Ok(stream) = canvas.capture_stream_with_frame_request_rate(0.0) else {
            return DriverHandle { _raf: None };
        };
        let Some(track) = stream
            .get_video_tracks()
            .get(0)
            .dyn_into::<CanvasCaptureMediaStreamTrack>()
            .ok()
        else {
            return DriverHandle { _raf: None };
        };
        writer.publish_native_source(Rc::new(stream));

        // Resolve each op's web resources once (params stay reactive).
        let web_ops = ops
            .into_iter()
            .filter_map(|op| match op {
                Op::Crop { fit, rect } => Some(WebOp::Crop { fit, rect }),
                Op::Watermark { image, corner, margin, opacity } => {
                    let canvas = image_to_canvas(&doc, &image)?;
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
                    video: new_video(&doc),
                    id: RefCell::new(None),
                    rect,
                }),
                // Drawn graphics aren't rendered on web yet.
                Op::Draw(_) => None,
            })
            .collect();

        let driver = Rc::new(Driver {
            input,
            input_video: new_video(&doc),
            input_id: RefCell::new(None),
            canvas,
            ctx,
            track,
            ops: web_ops,
            output_size,
        });

        let raf = raf_loop(move || driver.tick());
        DriverHandle { _raf: Some(raf) }
    }
}
