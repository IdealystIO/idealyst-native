//! Android renderer for the canvas SDK — native `android.graphics`.
//!
//! Replays the author's [`Scene`](canvas_core::Scene) into a
//! `Bitmap`-backed `android.graphics.Canvas` and shows the result via
//! `ImageView.setImageBitmap`. The bitmap is allocated at the view's pixel
//! size with the canvas scaled by the display density, so the author's
//! logical (dp) coordinates land at device resolution — drawing at the
//! view's actual bounds, like the iOS and web renderers.
//!
//! A real `Bitmap` canvas (rather than a `Picture` replayed through a
//! `PictureDrawable`) is used because it composites on the GPU as a normal
//! ImageView and rasterizes static content once.
//!
//! **Known clip limitation** (see `DrawOp::Clip` in `apply`): a `clipPath`
//! followed by an author transform (`concat`) before the clipped geometry
//! draws does not reliably crop on this trivial path. Non-transformed clips
//! work; clip+transform needs the custom-`View.onDraw` refinement or the
//! vello renderer. Every other op converges with web/iOS.
//!
//! This is the deliberately **trivial** approach: one JNI call per path
//! op, re-rasterizing the whole bitmap on every scene change. That makes
//! the per-op JNI cost easy to measure later (the native-vs-vello
//! benchmark) without pre-optimizing — exactly the brief. Two obvious v2
//! refinements if the numbers justify them: a batched flat-buffer replay
//! (one JNI crossing per frame), and reusing the bitmap across frames
//! instead of allocating a fresh one each render.
//!
//! Modeled on the `svg` SDK's Android painter (same JNI vocabulary),
//! narrowed to the canvas op set and driven by a flat op list instead of
//! a parsed tree.

use backend_android::{with_jni_env, AndroidBackend};
use canvas_core::{
    CanvasProps, Color, DrawOp, FillRule, GradientStop, LineCap, LineJoin, Paint, PaintKind, Path,
    PathSeg, Scene, TextureLayer,
};
use runtime_core::{after_ms_scoped, Effect};

use jni::objects::{GlobalRef, JObject, JValue};
use jni::sys::{jfloat, jint};
use jni::JNIEnv;

use std::cell::RefCell;
use std::rc::Rc;

/// Register the Android canvas renderer against an `AndroidBackend`.
pub fn register(backend: &mut AndroidBackend) {
    canvas_core::ensure_wire_serde();
    backend.register_external::<CanvasProps, _>(|props, b| build_canvas(props, b));
}

// Self-register at backend construction (no app-side `register` call needed).
// See [[project_inventory_self_registration]].
inventory::submit! {
    backend_android::AndroidExternalRegistrar(register)
}

fn build_canvas(props: &Rc<CanvasProps>, b: &mut AndroidBackend) -> GlobalRef {
    let view = b.with_jni(|env, ctx| {
        let class = env
            .find_class("android/widget/ImageView")
            .expect("find_class android/widget/ImageView");
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&ctx.as_obj())],
            )
            .expect("new ImageView(Context)");
        backend_android_core::helpers::apply_default_layout_params(env, &local);
        // FIT_XY: the Picture is recorded at the view's exact pixel size,
        // so this maps it 1:1 onto the view.
        let scale_type_class = env
            .find_class("android/widget/ImageView$ScaleType")
            .expect("find_class ImageView$ScaleType");
        let fit_xy = env
            .get_static_field(&scale_type_class, "FIT_XY", "Landroid/widget/ImageView$ScaleType;")
            .expect("FIT_XY static field")
            .l()
            .expect("FIT_XY as object");
        let _ = env.call_method(
            &local,
            "setScaleType",
            "(Landroid/widget/ImageView$ScaleType;)V",
            &[JValue::Object(&fit_xy)],
        );
        env.new_global_ref(local).expect("new_global_ref")
    });

    // Latest painted scene, shared between the reactive effect (writer +
    // renderer) and the initial-layout nudges (renderer).
    let cell: Rc<RefCell<Scene>> = Rc::new(RefCell::new(Scene::new()));

    // One throwaway CPU-frame subscription per active texture layer, so a camera
    // producer keeps delivering frames our `latest()` pull reads (see
    // `canvas_core::sync_layer_subscriptions`). Persists across renders.
    let layer_subs: Rc<RefCell<Vec<Option<canvas_core::Subscription>>>> =
        Rc::new(RefCell::new(Vec::new()));

    let render: Rc<dyn Fn()> = {
        let view = view.clone();
        let cell = cell.clone();
        let props = props.clone();
        let layer_subs = layer_subs.clone();
        Rc::new(move || {
            canvas_core::sync_layer_subscriptions(&props.layers, &mut layer_subs.borrow_mut());
            render_scene_into_view(&view, &cell.borrow(), &props);
        })
    };

    // Reactive repaint: re-record the Picture whenever a signal the draw
    // closure reads changes (animation re-records every frame).
    let _effect = Effect::new({
        let props = props.clone();
        let cell = cell.clone();
        let render = render.clone();
        move || {
            *cell.borrow_mut() = canvas_core::paint_scene(&props);
            render();
        }
    });

    // The reactive effect first runs at mount, before the view is laid out
    // (size 0 → render no-ops). Nudge a few renders over the next ~300ms so
    // a static canvas paints once it has real bounds. Animated canvases
    // don't need this (the effect re-renders every frame), but the nudges
    // are harmless there.
    for delay in [16i32, 50, 120, 300] {
        let render = render.clone();
        after_ms_scoped(delay, move || render());
    }

    view
}

/// Rasterize `scene` into a fresh `Bitmap` sized to the view's pixels
/// (density-scaled), composite any texture `layers` (e.g. a camera) over it,
/// install the result via `setImageBitmap`, and — while a recorder is
/// subscribed to `props.capture` — read the composited bitmap back and push it
/// to that stream (self-capture). No-op until the view has been laid out
/// (non-zero size).
fn render_scene_into_view(view: &GlobalRef, scene: &Scene, props: &CanvasProps) {
    let layers = &props.layers;
    with_jni_env(|env| {
        let w_px = call_int(env, view.as_obj(), "getWidth");
        let h_px = call_int(env, view.as_obj(), "getHeight");
        if w_px <= 0 || h_px <= 0 {
            return;
        }
        let density = view_density(env, view).max(0.01);

        // Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888)
        let config = match argb_8888_config(env) {
            Some(c) => c,
            None => return,
        };
        let bitmap_class = match env.find_class("android/graphics/Bitmap") {
            Ok(c) => c,
            Err(_) => return,
        };
        let bitmap = match env
            .call_static_method(
                &bitmap_class,
                "createBitmap",
                "(IILandroid/graphics/Bitmap$Config;)Landroid/graphics/Bitmap;",
                &[JValue::Int(w_px), JValue::Int(h_px), JValue::Object(&config)],
            )
            .and_then(|v| v.l())
        {
            Ok(b) => b,
            Err(_) => return,
        };

        // Canvas(bitmap) — a real raster canvas, so clipPath is honored.
        let canvas_class = match env.find_class("android/graphics/Canvas") {
            Ok(c) => c,
            Err(_) => return,
        };
        let canvas = match env.new_object(
            &canvas_class,
            "(Landroid/graphics/Bitmap;)V",
            &[JValue::Object(&bitmap)],
        ) {
            Ok(c) => c,
            Err(_) => return,
        };

        // Map logical (dp) author coordinates to device pixels.
        let _ = env.call_method(
            &canvas,
            "scale",
            "(FF)V",
            &[JValue::Float(density), JValue::Float(density)],
        );

        // Protect the base (density) CTM from any unbalanced author
        // save/restore in the scene, so layers composite in a known transform.
        let _ = env.call_method(&canvas, "save", "()I", &[]);
        if let Some(mut painter) = CanvasPainter::new(env, &canvas) {
            for op in scene.ops() {
                painter.apply(op);
            }
            drop(painter);
        }
        let _ = env.call_method(&canvas, "restore", "()V", &[]);

        // Texture layers (camera, screen share, …) composited OVER the scene,
        // in the same logical coordinate space (the density scale is still
        // active). Mirrors the web `draw_layers` path; shares `Fit::map_rects`.
        for layer in layers {
            composite_layer(env, &canvas, layer);
        }

        let _ = env.call_method(
            view.as_obj(),
            "setImageBitmap",
            "(Landroid/graphics/Bitmap;)V",
            &[JValue::Object(&bitmap)],
        );

        // Self-capture: while a recorder has subscribed to the capture stream
        // (`wants_cpu_frames`), read the just-composited bitmap (scene + camera)
        // back to RGBA and push it — so the recording is WYSIWYG. The bitmap is
        // ARGB_8888 (byte order R,G,B,A), matching `write_rgba8`. Gated on
        // `wants_cpu_frames` so the ~w·h·4 readback only happens while recording.
        if let Some(writer) = props.capture.as_ref() {
            if writer.wants_cpu_frames() {
                let n = (w_px as usize) * (h_px as usize) * 4;
                let mut rgba = vec![0u8; n];
                // SAFETY: `copyPixelsToBuffer` fills the buffer synchronously;
                // Java does not retain it past the call.
                if let Ok(buf) = unsafe { env.new_direct_byte_buffer(rgba.as_mut_ptr(), n) } {
                    if env
                        .call_method(
                            &bitmap,
                            "copyPixelsToBuffer",
                            "(Ljava/nio/Buffer;)V",
                            &[JValue::Object(&buf)],
                        )
                        .is_ok()
                    {
                        writer.write_rgba8(w_px as u32, h_px as u32, &rgba);
                    }
                }
            }
        }
    });
}

/// Composite one [`TextureLayer`] over the canvas: pull the stream's latest
/// RGBA frame, build a `Bitmap`, and `drawBitmap(src, dst)` into a rounded,
/// alpha-blended rect using the shared [`canvas_core::Fit::map_rects`] geometry.
/// No-op when the stream has no frame yet (camera still warming up).
fn composite_layer(env: &mut JNIEnv, canvas: &JObject, layer: &TextureLayer) {
    let Some(stream) = (layer.source)() else { return };
    let mut rgba: Vec<u8> = Vec::new();
    let Some((vw, vh)) = stream.latest(&mut rgba) else { return };
    if vw == 0 || vh == 0 || rgba.len() < (vw as usize) * (vh as usize) * 4 {
        return;
    }
    let (dx, dy, dw, dh) = (layer.rect)();
    if dw < 1.0 || dh < 1.0 {
        return;
    }
    let ((sx, sy, sw, sh), (ox, oy, ow, oh)) =
        layer.fit.map_rects(vw as f32, vh as f32, dx, dy, dw, dh);

    let Some(bmp) = rgba_bitmap(env, vw as i32, vh as i32, &mut rgba) else { return };

    // Paint: opacity via alpha, bilinear filtering for the scale-down.
    let Ok(paint_class) = env.find_class("android/graphics/Paint") else { return };
    let Ok(paint) = env.new_object(&paint_class, "()V", &[]) else { return };
    let _ = env.call_method(&paint, "setAntiAlias", "(Z)V", &[JValue::Bool(1)]);
    let _ = env.call_method(&paint, "setFilterBitmap", "(Z)V", &[JValue::Bool(1)]);
    let alpha = (layer.opacity.clamp(0.0, 1.0) * 255.0).round() as i32;
    let _ = env.call_method(&paint, "setAlpha", "(I)V", &[JValue::Int(alpha)]);

    let _ = env.call_method(canvas, "save", "()I", &[]);

    // Round the DRAWN rect (letterboxed for Contain) so corners clip the image.
    let r = layer.corner_radius.clamp(0.0, ow.min(oh) * 0.5);
    if r > 0.0 {
        if let Some(clip) = round_rect_path(env, ox, oy, ow, oh, r) {
            let local = unsafe { JObject::from_raw(clip.as_obj().as_raw()) };
            let _ = env.call_method(
                canvas,
                "clipPath",
                "(Landroid/graphics/Path;)Z",
                &[JValue::Object(&local)],
            );
        }
    }

    // src in bitmap pixels (int Rect), dst in logical points (RectF).
    if let (Some(src), Some(dst)) = (
        int_rect(env, sx, sy, sw, sh),
        rect_f(env, ox, oy, ow, oh),
    ) {
        let _ = env.call_method(
            canvas,
            "drawBitmap",
            "(Landroid/graphics/Bitmap;Landroid/graphics/Rect;Landroid/graphics/RectF;Landroid/graphics/Paint;)V",
            &[
                JValue::Object(&bmp),
                JValue::Object(&src),
                JValue::Object(&dst),
                JValue::Object(&paint),
            ],
        );
    }

    let _ = env.call_method(canvas, "restore", "()V", &[]);
}

/// Build a mutable `ARGB_8888` `Bitmap` of `w × h` from tightly-packed RGBA8
/// bytes. Android's `ARGB_8888` is byte-order R,G,B,A in memory, matching the
/// `MediaStream` frame layout, so `copyPixelsFromBuffer` is a straight copy. A
/// direct `ByteBuffer` wraps the Rust slice (no intermediate Java array); the
/// copy is synchronous, so the slice need only outlive this call.
fn rgba_bitmap<'env>(
    env: &mut JNIEnv<'env>,
    w: i32,
    h: i32,
    rgba: &mut [u8],
) -> Option<JObject<'env>> {
    let config = argb_8888_config(env)?;
    let bitmap_class = env.find_class("android/graphics/Bitmap").ok()?;
    let bitmap = env
        .call_static_method(
            &bitmap_class,
            "createBitmap",
            "(IILandroid/graphics/Bitmap$Config;)Landroid/graphics/Bitmap;",
            &[JValue::Int(w), JValue::Int(h), JValue::Object(&config)],
        )
        .ok()?
        .l()
        .ok()?;
    // SAFETY: the buffer is consumed synchronously by copyPixelsFromBuffer
    // below; Java does not retain it past this call.
    let buf = unsafe { env.new_direct_byte_buffer(rgba.as_mut_ptr(), rgba.len()).ok()? };
    env.call_method(
        &bitmap,
        "copyPixelsFromBuffer",
        "(Ljava/nio/Buffer;)V",
        &[JValue::Object(&buf)],
    )
    .ok()?;
    Some(bitmap)
}

/// `new Rect(round(x), round(y), round(x+w), round(y+h))` — integer source crop.
fn int_rect<'env>(env: &mut JNIEnv<'env>, x: f32, y: f32, w: f32, h: f32) -> Option<JObject<'env>> {
    let class = env.find_class("android/graphics/Rect").ok()?;
    env.new_object(
        &class,
        "(IIII)V",
        &[
            JValue::Int(x.round() as i32),
            JValue::Int(y.round() as i32),
            JValue::Int((x + w).round() as i32),
            JValue::Int((y + h).round() as i32),
        ],
    )
    .ok()
}

/// `new RectF(x, y, x+w, y+h)` — float destination rect.
fn rect_f<'env>(env: &mut JNIEnv<'env>, x: f32, y: f32, w: f32, h: f32) -> Option<JObject<'env>> {
    let class = env.find_class("android/graphics/RectF").ok()?;
    env.new_object(
        &class,
        "(FFFF)V",
        &[
            JValue::Float(x),
            JValue::Float(y),
            JValue::Float(x + w),
            JValue::Float(y + h),
        ],
    )
    .ok()
}

/// A `Path` with a single rounded rect — used as the layer clip.
fn round_rect_path(env: &mut JNIEnv, x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<GlobalRef> {
    let rect = rect_f(env, x, y, w, h)?;
    let path_class = env.find_class("android/graphics/Path").ok()?;
    let path = env.new_object(&path_class, "()V", &[]).ok()?;
    let dir_class = env.find_class("android/graphics/Path$Direction").ok()?;
    let cw = env
        .get_static_field(&dir_class, "CW", "Landroid/graphics/Path$Direction;")
        .ok()?
        .l()
        .ok()?;
    env.call_method(
        &path,
        "addRoundRect",
        "(Landroid/graphics/RectF;FFLandroid/graphics/Path$Direction;)V",
        &[
            JValue::Object(&rect),
            JValue::Float(r),
            JValue::Float(r),
            JValue::Object(&cw),
        ],
    )
    .ok()?;
    env.new_global_ref(&path).ok()
}

/// `Bitmap.Config.ARGB_8888` static field.
fn argb_8888_config<'env>(env: &mut JNIEnv<'env>) -> Option<JObject<'env>> {
    let class = env.find_class("android/graphics/Bitmap$Config").ok()?;
    env.get_static_field(&class, "ARGB_8888", "Landroid/graphics/Bitmap$Config;")
        .ok()?
        .l()
        .ok()
}

// ============================================================================
// Painter — replays DrawOps into a recording Canvas
// ============================================================================

struct CanvasPainter<'p, 'env> {
    env: &'p mut JNIEnv<'env>,
    canvas: &'p JObject<'env>,
    /// Reusable Paint, reset before each fill/stroke (JNI object alloc is
    /// the expensive part; reconfiguring one Paint is the standard pattern).
    paint: GlobalRef,
    fill_style: GlobalRef,
    stroke_style: GlobalRef,
}

impl<'p, 'env> CanvasPainter<'p, 'env> {
    fn new(env: &'p mut JNIEnv<'env>, canvas: &'p JObject<'env>) -> Option<Self> {
        let paint_class = env.find_class("android/graphics/Paint").ok()?;
        let paint = env.new_object(&paint_class, "()V", &[]).ok()?;
        let _ = env.call_method(&paint, "setAntiAlias", "(Z)V", &[JValue::Bool(1)]);
        let paint = env.new_global_ref(&paint).ok()?;

        let style_class = env.find_class("android/graphics/Paint$Style").ok()?;
        let fill_local = env
            .get_static_field(&style_class, "FILL", "Landroid/graphics/Paint$Style;")
            .ok()?
            .l()
            .ok()?;
        let fill_style = env.new_global_ref(&fill_local).ok()?;
        let stroke_local = env
            .get_static_field(&style_class, "STROKE", "Landroid/graphics/Paint$Style;")
            .ok()?
            .l()
            .ok()?;
        let stroke_style = env.new_global_ref(&stroke_local).ok()?;

        Some(Self { env, canvas, paint, fill_style, stroke_style })
    }

    fn apply(&mut self, op: &DrawOp) {
        match op {
            DrawOp::Save => {
                let _ = self.env.call_method(self.canvas, "save", "()I", &[]);
            }
            DrawOp::Restore => {
                let _ = self.env.call_method(self.canvas, "restore", "()V", &[]);
            }
            DrawOp::Transform(t) => {
                if let Some(m) = self.make_matrix(t.a, t.b, t.c, t.d, t.e, t.f) {
                    let _ = self.env.call_method(
                        self.canvas,
                        "concat",
                        "(Landroid/graphics/Matrix;)V",
                        &[JValue::Object(&m)],
                    );
                }
            }
            DrawOp::Fill { path, paint, fill_rule } => {
                let jpath = self.build_path(path, *fill_rule);
                self.reset_paint();
                self.set_style(true);
                self.apply_paint_source(paint);
                self.draw_path(&jpath);
            }
            DrawOp::Stroke { path, paint, stroke } => {
                let jpath = self.build_path(path, FillRule::NonZero);
                self.reset_paint();
                self.set_style(false);
                let p = self.paint.clone();
                let _ = self.env.call_method(
                    p.as_obj(),
                    "setStrokeWidth",
                    "(F)V",
                    &[JValue::Float(stroke.width)],
                );
                let _ = self.env.call_method(
                    p.as_obj(),
                    "setStrokeMiter",
                    "(F)V",
                    &[JValue::Float(stroke.miter_limit)],
                );
                self.set_stroke_cap(stroke.cap);
                self.set_stroke_join(stroke.join);
                self.apply_paint_source(paint);
                self.draw_path(&jpath);
            }
            DrawOp::Clip { path, fill_rule } => {
                let jpath = self.build_path(path, *fill_rule);
                // `local` is a borrowed `JObject` view of the `GlobalRef`
                // path. `JObject` has no `Drop` in jni 0.21 (only
                // `GlobalRef`/`AutoLocal`/`WeakRef` free), so it just falls
                // out of scope — no `mem::forget` to avoid a double-free.
                let local = unsafe { JObject::from_raw(jpath.as_obj().as_raw()) };
                let _ = self.env.call_method(
                    self.canvas,
                    "clipPath",
                    "(Landroid/graphics/Path;)Z",
                    &[JValue::Object(&local)],
                );
                // KNOWN LIMITATION: a `clipPath` immediately followed by a
                // `concat` (author transform) before the clipped geometry
                // draws does not reliably crop on this trivial JNI path — the
                // clip is dropped by draw time. An intervening draw realizes
                // it, but `getClipBounds()` does not, so there's no cheap
                // commit. Non-transformed clips work. The proper fix is the
                // custom `View.onDraw` (Kotlin shim) or the vello renderer,
                // both of which honor clip+transform. Tracked, not silent
                // (CLAUDE.md §7).
            }
            // `DrawOp` is `#[non_exhaustive]`; future ops no-op until wired.
            _ => {}
        }
    }

    fn build_path(&mut self, path: &Path, rule: FillRule) -> GlobalRef {
        let path_class =
            self.env.find_class("android/graphics/Path").expect("find_class Path");
        let jpath = self.env.new_object(&path_class, "()V", &[]).expect("new Path()");
        for seg in &path.segs {
            match seg {
                PathSeg::MoveTo { x, y } => {
                    let _ = self.env.call_method(&jpath, "moveTo", "(FF)V", &[
                        JValue::Float(*x),
                        JValue::Float(*y),
                    ]);
                }
                PathSeg::LineTo { x, y } => {
                    let _ = self.env.call_method(&jpath, "lineTo", "(FF)V", &[
                        JValue::Float(*x),
                        JValue::Float(*y),
                    ]);
                }
                PathSeg::QuadTo { cx, cy, x, y } => {
                    let _ = self.env.call_method(&jpath, "quadTo", "(FFFF)V", &[
                        JValue::Float(*cx),
                        JValue::Float(*cy),
                        JValue::Float(*x),
                        JValue::Float(*y),
                    ]);
                }
                PathSeg::CubicTo { c1x, c1y, c2x, c2y, x, y } => {
                    let _ = self.env.call_method(&jpath, "cubicTo", "(FFFFFF)V", &[
                        JValue::Float(*c1x),
                        JValue::Float(*c1y),
                        JValue::Float(*c2x),
                        JValue::Float(*c2y),
                        JValue::Float(*x),
                        JValue::Float(*y),
                    ]);
                }
                PathSeg::Close => {
                    let _ = self.env.call_method(&jpath, "close", "()V", &[]);
                }
            }
        }
        if rule == FillRule::EvenOdd {
            self.set_fill_type(&jpath, "EVEN_ODD");
        }
        self.env.new_global_ref(&jpath).expect("new_global_ref(Path)")
    }

    fn set_fill_type(&mut self, path: &JObject, field: &str) {
        let Ok(class) = self.env.find_class("android/graphics/Path$FillType") else {
            return;
        };
        if let Ok(value) = self
            .env
            .get_static_field(&class, field, "Landroid/graphics/Path$FillType;")
            .and_then(|v| v.l())
        {
            let _ = self.env.call_method(
                path,
                "setFillType",
                "(Landroid/graphics/Path$FillType;)V",
                &[JValue::Object(&value)],
            );
        }
    }

    fn reset_paint(&mut self) {
        let p = self.paint.clone();
        let _ = self.env.call_method(p.as_obj(), "reset", "()V", &[]);
        let _ = self.env.call_method(p.as_obj(), "setAntiAlias", "(Z)V", &[JValue::Bool(1)]);
    }

    fn set_style(&mut self, fill: bool) {
        let style = if fill { self.fill_style.clone() } else { self.stroke_style.clone() };
        let p = self.paint.clone();
        let _ = self.env.call_method(
            p.as_obj(),
            "setStyle",
            "(Landroid/graphics/Paint$Style;)V",
            &[JValue::Object(style.as_obj())],
        );
    }

    /// Configure the reusable paint's color or shader from a [`Paint`].
    fn apply_paint_source(&mut self, paint: &Paint) {
        match &paint.kind {
            PaintKind::Solid(c) => self.set_color(*c),
            PaintKind::Linear(g) => {
                if let Some(sh) = self.linear_shader(g.x0, g.y0, g.x1, g.y1, &g.stops) {
                    self.set_shader(&sh);
                }
            }
            PaintKind::Radial(g) => {
                if let Some(sh) = self.radial_shader(g.cx, g.cy, g.r, &g.stops) {
                    self.set_shader(&sh);
                }
            }
            _ => self.set_color(Color::TRANSPARENT),
        }
    }

    fn set_color(&mut self, c: Color) {
        let p = self.paint.clone();
        let _ = self
            .env
            .call_method(p.as_obj(), "setColor", "(I)V", &[JValue::Int(color_argb(c))]);
    }

    fn set_shader(&mut self, shader: &JObject) {
        let p = self.paint.clone();
        let _ = self.env.call_method(
            p.as_obj(),
            "setShader",
            "(Landroid/graphics/Shader;)Landroid/graphics/Shader;",
            &[JValue::Object(shader)],
        );
    }

    fn draw_path(&mut self, path: &GlobalRef) {
        // `local` is a borrowed `JObject` view of the `GlobalRef` path.
        // `JObject` has no `Drop` in jni 0.21 (only `GlobalRef`/`AutoLocal`/
        // `WeakRef` free), so it falls out of scope harmlessly — no
        // `mem::forget` is needed to avoid double-freeing the GlobalRef.
        let local = unsafe { JObject::from_raw(path.as_obj().as_raw()) };
        let p = self.paint.clone();
        let _ = self.env.call_method(
            self.canvas,
            "drawPath",
            "(Landroid/graphics/Path;Landroid/graphics/Paint;)V",
            &[JValue::Object(&local), JValue::Object(p.as_obj())],
        );
    }

    fn set_stroke_cap(&mut self, cap: LineCap) {
        let field = match cap {
            LineCap::Butt => "BUTT",
            LineCap::Round => "ROUND",
            LineCap::Square => "SQUARE",
        };
        let Ok(class) = self.env.find_class("android/graphics/Paint$Cap") else {
            return;
        };
        if let Ok(value) = self
            .env
            .get_static_field(&class, field, "Landroid/graphics/Paint$Cap;")
            .and_then(|v| v.l())
        {
            let p = self.paint.clone();
            let _ = self.env.call_method(
                p.as_obj(),
                "setStrokeCap",
                "(Landroid/graphics/Paint$Cap;)V",
                &[JValue::Object(&value)],
            );
        }
    }

    fn set_stroke_join(&mut self, join: LineJoin) {
        let field = match join {
            LineJoin::Miter => "MITER",
            LineJoin::Round => "ROUND",
            LineJoin::Bevel => "BEVEL",
        };
        let Ok(class) = self.env.find_class("android/graphics/Paint$Join") else {
            return;
        };
        if let Ok(value) = self
            .env
            .get_static_field(&class, field, "Landroid/graphics/Paint$Join;")
            .and_then(|v| v.l())
        {
            let p = self.paint.clone();
            let _ = self.env.call_method(
                p.as_obj(),
                "setStrokeJoin",
                "(Landroid/graphics/Paint$Join;)V",
                &[JValue::Object(&value)],
            );
        }
    }

    fn make_matrix(&mut self, a: f32, b: f32, c: f32, d: f32, e: f32, f: f32) -> Option<JObject<'env>> {
        let class = self.env.find_class("android/graphics/Matrix").ok()?;
        let matrix = self.env.new_object(&class, "()V", &[]).ok()?;
        // Android Matrix row-major: [MSCALE_X, MSKEW_X, MTRANS_X,
        //                            MSKEW_Y,  MSCALE_Y, MTRANS_Y, 0, 0, 1]
        // maps x' = MSCALE_X·x + MSKEW_X·y + MTRANS_X, matching our
        // Transform's (a, c, e) / (b, d, f).
        let vals: [jfloat; 9] = [a, c, e, b, d, f, 0.0, 0.0, 1.0];
        let arr = self.env.new_float_array(9).ok()?;
        self.env.set_float_array_region(&arr, 0, &vals).ok()?;
        self.env
            .call_method(&matrix, "setValues", "([F)V", &[JValue::Object(&JObject::from(arr))])
            .ok()?;
        Some(matrix)
    }

    fn linear_shader(
        &mut self,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        stops: &[GradientStop],
    ) -> Option<JObject<'env>> {
        if stops.is_empty() {
            return None;
        }
        let (colors, positions) = self.stop_arrays(stops)?;
        let class = self.env.find_class("android/graphics/LinearGradient").ok()?;
        let clamp = self.tile_clamp()?;
        self.env
            .new_object(&class, "(FFFF[I[FLandroid/graphics/Shader$TileMode;)V", &[
                JValue::Float(x0),
                JValue::Float(y0),
                JValue::Float(x1),
                JValue::Float(y1),
                JValue::Object(&JObject::from(colors)),
                JValue::Object(&JObject::from(positions)),
                JValue::Object(&clamp),
            ])
            .ok()
    }

    fn radial_shader(
        &mut self,
        cx: f32,
        cy: f32,
        r: f32,
        stops: &[GradientStop],
    ) -> Option<JObject<'env>> {
        if stops.is_empty() || r <= 0.0 {
            return None;
        }
        let (colors, positions) = self.stop_arrays(stops)?;
        let class = self.env.find_class("android/graphics/RadialGradient").ok()?;
        let clamp = self.tile_clamp()?;
        self.env
            .new_object(&class, "(FFF[I[FLandroid/graphics/Shader$TileMode;)V", &[
                JValue::Float(cx),
                JValue::Float(cy),
                JValue::Float(r),
                JValue::Object(&JObject::from(colors)),
                JValue::Object(&JObject::from(positions)),
                JValue::Object(&clamp),
            ])
            .ok()
    }

    fn tile_clamp(&mut self) -> Option<JObject<'env>> {
        let tile_class = self.env.find_class("android/graphics/Shader$TileMode").ok()?;
        self.env
            .get_static_field(&tile_class, "CLAMP", "Landroid/graphics/Shader$TileMode;")
            .ok()?
            .l()
            .ok()
    }

    fn stop_arrays(
        &mut self,
        stops: &[GradientStop],
    ) -> Option<(jni::objects::JIntArray<'env>, jni::objects::JFloatArray<'env>)> {
        let colors: Vec<jint> = stops.iter().map(|s| color_argb(s.color)).collect();
        let positions: Vec<jfloat> = stops.iter().map(|s| s.offset).collect();
        let colors_arr = self.env.new_int_array(colors.len() as jni::sys::jsize).ok()?;
        self.env.set_int_array_region(&colors_arr, 0, &colors).ok()?;
        let positions_arr = self.env.new_float_array(positions.len() as jni::sys::jsize).ok()?;
        self.env.set_float_array_region(&positions_arr, 0, &positions).ok()?;
        Some((colors_arr, positions_arr))
    }
}

// ============================================================================
// Free helpers
// ============================================================================

/// `Rgba` → Android's `0xAARRGGBB` int.
fn color_argb(c: Color) -> i32 {
    ((c.a as i32) << 24) | ((c.r as i32) << 16) | ((c.g as i32) << 8) | (c.b as i32)
}

fn call_int(env: &mut JNIEnv, obj: &JObject, method: &str) -> i32 {
    env.call_method(obj, method, "()I", &[]).and_then(|v| v.i()).unwrap_or(0)
}

/// `view.getResources().getDisplayMetrics().density`.
fn view_density(env: &mut JNIEnv, view: &GlobalRef) -> f32 {
    let resources = env
        .call_method(view.as_obj(), "getResources", "()Landroid/content/res/Resources;", &[])
        .and_then(|v| v.l());
    let Ok(resources) = resources else { return 1.0 };
    let dm = env
        .call_method(&resources, "getDisplayMetrics", "()Landroid/util/DisplayMetrics;", &[])
        .and_then(|v| v.l());
    let Ok(dm) = dm else { return 1.0 };
    env.get_field(&dm, "density", "F").and_then(|v| v.f()).unwrap_or(1.0)
}
