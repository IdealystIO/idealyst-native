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
    BlendMode, CanvasProps, Color, DrawOp, FillRule, GradientStop, ImageSource, LineCap, LineJoin,
    Paint, PaintKind, Path, PathSeg, Scene, TextureLayer,
};
use std::collections::HashMap;
use runtime_core::{after_ms_scoped, effect};

use jni::objects::{GlobalRef, JObject, JValue};
use jni::sys::{jfloat, jint};
use jni::JNIEnv;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

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
    // closure reads changes (animation re-records every frame). Built in the
    // canvas walker, so the component scope owns it. Clones hoisted so the
    // macro's `move` captures them (cloned once).
    {
        let props = props.clone();
        let cell = cell.clone();
        let render = render.clone();
        effect!({
            *cell.borrow_mut() = canvas_core::paint_scene(&props);
            render();
        });
    }

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
                // Announce the slow path ONCE. This canvas-native CPU read-back is
                // the EMULATOR fallback (real devices run vello, which captures
                // on-GPU). Warn so a developer recording on the emulator knows why
                // it's sluggish and validates performance on a physical device.
                static LOGGED: AtomicBool = AtomicBool::new(false);
                if !LOGGED.swap(true, Ordering::Relaxed) {
                    if let Ok(log_class) = env.find_class("android/util/Log") {
                        if let (Ok(tag), Ok(msg)) = (
                            env.new_string("canvas"),
                            env.new_string(
                                "recording via the android.graphics CPU renderer \
                                 (emulator fallback — vello can't run here). Expect \
                                 SEVERE performance loss; record on a physical device \
                                 for representative performance.",
                            ),
                        ) {
                            let _ = env.call_static_method(
                                &log_class,
                                "w",
                                "(Ljava/lang/String;Ljava/lang/String;)I",
                                &[JValue::Object(&tag), JValue::Object(&msg)],
                            );
                        }
                    }
                }

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
    let mut rgba: Vec<u8> = Vec::new();
    let Some((vw, vh)) = layer.resolve_rgba(&mut rgba) else { return };
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
    let r = (layer.corner_radius)().clamp(0.0, ow.min(oh) * 0.5);
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

    // Border frame, composited WITH the image (stays locked to the moving
    // picture). Stroked on a rounded rect inset by half the width.
    let bw = layer.border_width;
    if bw > 0.0 {
        if let Ok(border_paint) = env.new_object(&paint_class, "()V", &[]) {
            let _ = env.call_method(&border_paint, "setAntiAlias", "(Z)V", &[JValue::Bool(1)]);
            if let Ok(style) = env
                .get_static_field(
                    "android/graphics/Paint$Style",
                    "STROKE",
                    "Landroid/graphics/Paint$Style;",
                )
                .and_then(|v| v.l())
            {
                let _ = env.call_method(
                    &border_paint,
                    "setStyle",
                    "(Landroid/graphics/Paint$Style;)V",
                    &[JValue::Object(&style)],
                );
            }
            let _ = env.call_method(
                &border_paint,
                "setStrokeWidth",
                "(F)V",
                &[JValue::Float(bw as jfloat)],
            );
            let c = layer.border_color;
            let argb = ((c.a as i32) << 24)
                | ((c.r as i32) << 16)
                | ((c.g as i32) << 8)
                | (c.b as i32);
            let _ = env.call_method(&border_paint, "setColor", "(I)V", &[JValue::Int(argb)]);
            let inset = bw * 0.5;
            let br = (r - inset).max(0.0);
            if let Some(brect) = rect_f(env, ox + inset, oy + inset, ow - bw, oh - bw) {
                let _ = env.call_method(
                    canvas,
                    "drawRoundRect",
                    "(Landroid/graphics/RectF;FFLandroid/graphics/Paint;)V",
                    &[
                        JValue::Object(&brect),
                        JValue::Float(br as jfloat),
                        JValue::Float(br as jfloat),
                        JValue::Object(&border_paint),
                    ],
                );
            }
        }
    }

    let _ = env.call_method(canvas, "restore", "()V", &[]);
}

/// Build a mutable `ARGB_8888` `Bitmap` of `w × h` from tightly-packed RGBA8
/// bytes. Android's `ARGB_8888` is byte-order R,G,B,A in memory, matching the
/// `MediaStream` frame layout, so `copyPixelsFromBuffer` is a straight copy. A
/// direct `ByteBuffer` wraps the Rust slice (no intermediate Java array); the
/// copy is synchronous, so the slice need only outlive this call.
thread_local! {
    /// Per-thread cache of uploaded image `Bitmap`s (as `GlobalRef`s) keyed
    /// by [`ImageSource::id`], so a static image isn't re-uploaded to the JVM
    /// every frame. Never evicts — canvas authors use a small, stable set of
    /// image ids; the held global refs free at process exit.
    static BITMAP_CACHE: RefCell<HashMap<u64, GlobalRef>> = RefCell::new(HashMap::new());

    /// Persistent [`DrawOp::Layer`] surfaces — a `Bitmap` (`GlobalRef`) plus
    /// its `w × h` per layer id, retained across frames so baked strokes
    /// survive and accumulate. A size change rebuilds the bitmap.
    static LAYER_BITMAPS: RefCell<HashMap<u32, (GlobalRef, i32, i32)>> =
        RefCell::new(HashMap::new());

    /// Persistent [`DrawOp::LayerCached`] surfaces — a `Bitmap` (`GlobalRef`) +
    /// its `w × h` per layer id, baked once (`dirty`) and composited under a
    /// camera transform every frame. Distinct from [`LAYER_BITMAPS`] so a cached
    /// and an accumulate layer can share an id.
    static CACHED_LAYER_BITMAPS: RefCell<HashMap<u32, (GlobalRef, i32, i32)>> =
        RefCell::new(HashMap::new());
}

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
                self.set_blend(paint.blend);
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
                self.set_dash(&stroke.dash, stroke.dash_offset);
                self.apply_paint_source(paint);
                self.set_blend(paint.blend);
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
            DrawOp::Layer { id, clear, ops: nested, alpha, blend } => {
                self.draw_layer(*id, *clear, nested, *alpha, *blend);
            }
            DrawOp::LayerCached { id, dirty, transform, ops: nested, alpha, blend } => {
                self.draw_cached_layer(*id, *dirty, transform, nested, *alpha, *blend);
            }
            DrawOp::Shapes { shapes, blend } => {
                // android.graphics has no instanced fast path: expand the batch to
                // per-shape fills, in array order, replaying each through the Fill
                // arm so a batched shape matches a hand-authored fill (CLAUDE.md §7).
                for sh in shapes {
                    self.apply(&sh.to_fill_op(*blend));
                }
            }
            DrawOp::Image { image, dst, alpha, blend } => {
                let Some(bmp) = self.cached_bitmap(image) else { return };
                self.reset_paint();
                let p = self.paint.clone();
                let a = (alpha.clamp(0.0, 1.0) * 255.0).round() as i32;
                let _ = self.env.call_method(p.as_obj(), "setAlpha", "(I)V", &[JValue::Int(a)]);
                // Bilinear filtering for the scale to `dst`.
                let _ = self.env.call_method(
                    p.as_obj(),
                    "setFilterBitmap",
                    "(Z)V",
                    &[JValue::Bool(1)],
                );
                self.set_blend(*blend);
                if let (Some(src), Some(dr)) = (
                    int_rect(&mut self.env, 0.0, 0.0, image.width as f32, image.height as f32),
                    rect_f(&mut self.env, dst.x, dst.y, dst.w, dst.h),
                ) {
                    let _ = self.env.call_method(
                        self.canvas,
                        "drawBitmap",
                        "(Landroid/graphics/Bitmap;Landroid/graphics/Rect;Landroid/graphics/RectF;Landroid/graphics/Paint;)V",
                        &[
                            JValue::Object(bmp.as_obj()),
                            JValue::Object(&src),
                            JValue::Object(&dr),
                            JValue::Object(p.as_obj()),
                        ],
                    );
                }
            }
            DrawOp::Glyphs { font, glyphs, paint } => {
                // No glyph engine here for embedded PDF fonts: outline each glyph
                // and fill it, matching the GPU (vello) path's geometry
                // (CLAUDE.md §7).
                for op in crate::glyphs::expand_run(font, glyphs, paint) {
                    self.apply(&op);
                }
            }
            DrawOp::MaskGroup { content, .. } => {
                // No soft-mask primitive wired on android.graphics yet: draw the
                // content unmasked so it doesn't vanish (the GPU/vello path masks
                // correctly). canvas-native is the emulator fallback.
                for op in content {
                    self.apply(op);
                }
            }
            // `DrawOp` is `#[non_exhaustive]`; future ops no-op until wired.
            _ => {}
        }
    }

    /// Replay `nested` into the persistent layer `id`'s offscreen `Bitmap`
    /// (wiping first if `clear`), then composite it into the main canvas at
    /// `alpha`/`blend`. The CPU-raster counterpart of the vello retained
    /// op-log layer — same observable pixels (CLAUDE.md §7).
    ///
    /// Only the `Bitmap` is cached (its pixels persist); a fresh `Canvas`
    /// wrapping it is made each frame (cheap — no pixel copy) and the main
    /// canvas's current `Matrix` is mirrored onto it, so nested logical ops
    /// land at the same device pixels. The composite is a 1:1 device blit
    /// (main matrix reset to identity).
    fn draw_layer(
        &mut self,
        id: u32,
        clear: bool,
        nested: &[DrawOp],
        alpha: f32,
        blend: BlendMode,
    ) {
        let w = self
            .env
            .call_method(self.canvas, "getWidth", "()I", &[])
            .ok()
            .and_then(|v| v.i().ok())
            .unwrap_or(0);
        let h = self
            .env
            .call_method(self.canvas, "getHeight", "()I", &[])
            .ok()
            .and_then(|v| v.i().ok())
            .unwrap_or(0);
        if w <= 0 || h <= 0 {
            return;
        }
        let Some(bitmap) = self.layer_bitmap(id, w, h) else { return };
        if clear {
            let _ = self.env.call_method(bitmap.as_obj(), "eraseColor", "(I)V", &[JValue::Int(0)]);
        }

        // Fresh local Canvas over the persistent bitmap; mirror the main
        // canvas transform so nested logical ops align with device pixels.
        let Ok(canvas_class) = self.env.find_class("android/graphics/Canvas") else { return };
        let Ok(layer_canvas) = self.env.new_object(
            &canvas_class,
            "(Landroid/graphics/Bitmap;)V",
            &[JValue::Object(bitmap.as_obj())],
        ) else {
            return;
        };
        if let Ok(m) = self
            .env
            .call_method(self.canvas, "getMatrix", "()Landroid/graphics/Matrix;", &[])
            .and_then(|v| v.l())
        {
            let _ = self.env.call_method(
                &layer_canvas,
                "setMatrix",
                "(Landroid/graphics/Matrix;)V",
                &[JValue::Object(&m)],
            );
        }
        // Replay nested ops into the layer canvas with a sub-painter.
        if let Some(mut sub) = CanvasPainter::new(&mut *self.env, &layer_canvas) {
            for op in nested {
                sub.apply(op);
            }
        }

        // Composite the bitmap into the main canvas at identity (device 1:1).
        self.reset_paint();
        let p = self.paint.clone();
        let a = (alpha.clamp(0.0, 1.0) * 255.0).round() as i32;
        let _ = self.env.call_method(p.as_obj(), "setAlpha", "(I)V", &[JValue::Int(a)]);
        self.set_blend(blend);
        let _ = self.env.call_method(self.canvas, "save", "()I", &[]);
        if let Ok(idm) = self.env.new_object("android/graphics/Matrix", "()V", &[]) {
            let _ = self.env.call_method(
                self.canvas,
                "setMatrix",
                "(Landroid/graphics/Matrix;)V",
                &[JValue::Object(&idm)],
            );
        }
        let _ = self.env.call_method(
            self.canvas,
            "drawBitmap",
            "(Landroid/graphics/Bitmap;FFLandroid/graphics/Paint;)V",
            &[
                JValue::Object(bitmap.as_obj()),
                JValue::Float(0.0),
                JValue::Float(0.0),
                JValue::Object(p.as_obj()),
            ],
        );
        let _ = self.env.call_method(self.canvas, "restore", "()V", &[]);
    }

    /// Replay `nested` into the cached layer `id`'s offscreen `Bitmap` (only
    /// when `dirty` — or first sight / after a resize), then composite it into
    /// the main canvas under the camera `transform` at `alpha`/`blend`. The CPU
    /// **fallback** counterpart of the vello `TransformCompositor` (on a real
    /// device the GPU vello path handles this; this runs on the emulator).
    ///
    /// The bitmap holds device-resolution content (baked at the main canvas
    /// matrix, like [`draw_layer`](Self::draw_layer)). To composite it under the
    /// LOGICAL `transform`, the main matrix `M` is replaced with `M · T · M⁻¹`
    /// (the transform conjugated into device space) and the bitmap drawn 1:1 —
    /// so the cached raster moves with the camera at `O(1)`, correct for any base
    /// matrix (scale and/or translation). A `dirty: false` pan reuses the bitmap.
    fn draw_cached_layer(
        &mut self,
        id: u32,
        dirty: bool,
        transform: &canvas_core::Transform,
        nested: &[DrawOp],
        alpha: f32,
        blend: BlendMode,
    ) {
        let w = self
            .env
            .call_method(self.canvas, "getWidth", "()I", &[])
            .ok()
            .and_then(|v| v.i().ok())
            .unwrap_or(0);
        let h = self
            .env
            .call_method(self.canvas, "getHeight", "()I", &[])
            .ok()
            .and_then(|v| v.i().ok())
            .unwrap_or(0);
        if w <= 0 || h <= 0 {
            return;
        }
        let Some((bitmap, fresh)) = self.cached_layer_bitmap(id, w, h) else { return };

        // Bake only on `dirty` (or first sight / post-resize) — a not-dirty pan
        // reuses the retained bitmap, which is the whole point.
        if dirty || fresh {
            let _ = self.env.call_method(bitmap.as_obj(), "eraseColor", "(I)V", &[JValue::Int(0)]);
            let Ok(canvas_class) = self.env.find_class("android/graphics/Canvas") else { return };
            let Ok(layer_canvas) = self.env.new_object(
                &canvas_class,
                "(Landroid/graphics/Bitmap;)V",
                &[JValue::Object(bitmap.as_obj())],
            ) else {
                return;
            };
            // Mirror the main canvas transform so nested logical ops align with
            // device pixels (the bitmap is device-resolution).
            if let Ok(m) = self
                .env
                .call_method(self.canvas, "getMatrix", "()Landroid/graphics/Matrix;", &[])
                .and_then(|v| v.l())
            {
                let _ = self.env.call_method(
                    &layer_canvas,
                    "setMatrix",
                    "(Landroid/graphics/Matrix;)V",
                    &[JValue::Object(&m)],
                );
            }
            if let Some(mut sub) = CanvasPainter::new(&mut *self.env, &layer_canvas) {
                for op in nested {
                    sub.apply(op);
                }
            }
        }

        // Composite under the camera transform (conjugated into device space).
        self.reset_paint();
        let p = self.paint.clone();
        let a = (alpha.clamp(0.0, 1.0) * 255.0).round() as i32;
        let _ = self.env.call_method(p.as_obj(), "setAlpha", "(I)V", &[JValue::Int(a)]);
        // Bilinear so a zoomed cached layer stays smooth.
        let _ = self.env.call_method(p.as_obj(), "setFilterBitmap", "(Z)V", &[JValue::Bool(1)]);
        self.set_blend(blend);
        let _ = self.env.call_method(self.canvas, "save", "()I", &[]);
        if let Some(conj) = self.cached_composite_matrix(transform) {
            let _ = self.env.call_method(
                self.canvas,
                "setMatrix",
                "(Landroid/graphics/Matrix;)V",
                &[JValue::Object(&conj)],
            );
        }
        let _ = self.env.call_method(
            self.canvas,
            "drawBitmap",
            "(Landroid/graphics/Bitmap;FFLandroid/graphics/Paint;)V",
            &[
                JValue::Object(bitmap.as_obj()),
                JValue::Float(0.0),
                JValue::Float(0.0),
                JValue::Object(p.as_obj()),
            ],
        );
        let _ = self.env.call_method(self.canvas, "restore", "()V", &[]);
    }

    /// Build `M · T · M⁻¹` (the logical camera `transform` conjugated by the main
    /// canvas matrix `M`) as an Android `Matrix`, so a device-resolution cached
    /// bitmap drawn 1:1 under it lands at the camera-transformed logical position.
    fn cached_composite_matrix(
        &mut self,
        transform: &canvas_core::Transform,
    ) -> Option<JObject<'env>> {
        let m_base = self
            .env
            .call_method(self.canvas, "getMatrix", "()Landroid/graphics/Matrix;", &[])
            .ok()?
            .l()
            .ok()?;
        let c =
            self.make_matrix(transform.a, transform.b, transform.c, transform.d, transform.e, transform.f)?;
        let class = self.env.find_class("android/graphics/Matrix").ok()?;
        // conj = copy(M); conj.preConcat(T) ⇒ M·T.
        let conj = self
            .env
            .new_object(&class, "(Landroid/graphics/Matrix;)V", &[JValue::Object(&m_base)])
            .ok()?;
        let _ = self.env.call_method(
            &conj,
            "preConcat",
            "(Landroid/graphics/Matrix;)Z",
            &[JValue::Object(&c)],
        );
        // inv = M⁻¹; conj.preConcat(inv) ⇒ M·T·M⁻¹.
        let inv = self.env.new_object(&class, "()V", &[]).ok()?;
        let _ = self.env.call_method(
            &m_base,
            "invert",
            "(Landroid/graphics/Matrix;)Z",
            &[JValue::Object(&inv)],
        );
        let _ = self.env.call_method(
            &conj,
            "preConcat",
            "(Landroid/graphics/Matrix;)Z",
            &[JValue::Object(&inv)],
        );
        Some(conj)
    }

    /// Get-or-build the cached-layer `Bitmap` (as a `GlobalRef`) for `id` at
    /// `w × h`, returning `(bitmap, fresh)` where `fresh` is `true` when this
    /// call created (or resized → recreated) it — so the caller bakes on first
    /// sight even if the frame said not-dirty. A size change rebuilds it.
    fn cached_layer_bitmap(&mut self, id: u32, w: i32, h: i32) -> Option<(GlobalRef, bool)> {
        if let Some((g, cw, ch)) =
            CACHED_LAYER_BITMAPS.with(|c| c.borrow().get(&id).map(|(g, w, h)| (g.clone(), *w, *h)))
        {
            if cw == w && ch == h {
                return Some((g, false));
            }
        }
        let config = argb_8888_config(&mut self.env)?;
        let bitmap_class = self.env.find_class("android/graphics/Bitmap").ok()?;
        let bitmap = self
            .env
            .call_static_method(
                &bitmap_class,
                "createBitmap",
                "(IILandroid/graphics/Bitmap$Config;)Landroid/graphics/Bitmap;",
                &[JValue::Int(w), JValue::Int(h), JValue::Object(&config)],
            )
            .ok()?
            .l()
            .ok()?;
        let global = self.env.new_global_ref(&bitmap).ok()?;
        CACHED_LAYER_BITMAPS.with(|c| c.borrow_mut().insert(id, (global.clone(), w, h)));
        Some((global, true))
    }

    /// Get-or-build the persistent layer `Bitmap` (as a `GlobalRef`) for `id`
    /// at `w × h`. A size change rebuilds (and thus clears) it.
    fn layer_bitmap(&mut self, id: u32, w: i32, h: i32) -> Option<GlobalRef> {
        if let Some((g, cw, ch)) =
            LAYER_BITMAPS.with(|c| c.borrow().get(&id).map(|(g, w, h)| (g.clone(), *w, *h)))
        {
            if cw == w && ch == h {
                return Some(g);
            }
        }
        let config = argb_8888_config(&mut self.env)?;
        let bitmap_class = self.env.find_class("android/graphics/Bitmap").ok()?;
        let bitmap = self
            .env
            .call_static_method(
                &bitmap_class,
                "createBitmap",
                "(IILandroid/graphics/Bitmap$Config;)Landroid/graphics/Bitmap;",
                &[JValue::Int(w), JValue::Int(h), JValue::Object(&config)],
            )
            .ok()?
            .l()
            .ok()?;
        let global = self.env.new_global_ref(&bitmap).ok()?;
        LAYER_BITMAPS.with(|c| c.borrow_mut().insert(id, (global.clone(), w, h)));
        Some(global)
    }

    /// Get-or-build a cached `Bitmap` (as a `GlobalRef`) for `src`, keyed by
    /// [`ImageSource::id`]. Building once and caching avoids re-uploading a
    /// static image every frame. Returns `None` for an invalid/empty image.
    fn cached_bitmap(&mut self, src: &ImageSource) -> Option<GlobalRef> {
        if !src.is_valid() || src.width == 0 || src.height == 0 {
            return None;
        }
        if let Some(g) = BITMAP_CACHE.with(|c| c.borrow().get(&src.id).cloned()) {
            return Some(g);
        }
        // `rgba_bitmap` needs a mutable buffer for the direct ByteBuffer; clone
        // once (cached thereafter).
        let mut bytes = src.rgba.clone();
        let bmp = rgba_bitmap(&mut self.env, src.width as i32, src.height as i32, &mut bytes)?;
        let global = self.env.new_global_ref(&bmp).ok()?;
        BITMAP_CACHE.with(|c| c.borrow_mut().insert(src.id, global.clone()));
        Some(global)
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

    /// Install a `PorterDuffXfermode` on the reusable paint for a blended
    /// op. `Normal` is a no-op: [`reset_paint`](Self::reset_paint) already
    /// cleared any prior xfermode (the default = SrcOver), so each op starts
    /// clean and only blended ops touch the xfermode.
    fn set_blend(&mut self, blend: BlendMode) {
        // `PorterDuff.Mode` covers the basics and works on every API level.
        let porterduff = match blend {
            BlendMode::Normal => return,
            BlendMode::DestinationOut => Some("DST_OUT"),
            BlendMode::Multiply => Some("MULTIPLY"),
            BlendMode::Screen => Some("SCREEN"),
            BlendMode::Overlay => Some("OVERLAY"),
            BlendMode::Darken => Some("DARKEN"),
            BlendMode::Lighten => Some("LIGHTEN"),
            _ => None,
        };
        match porterduff {
            Some(field) => self.set_porterduff(field),
            // The advanced W3C separable/non-separable modes aren't in
            // `PorterDuff.Mode`; they need `android.graphics.BlendMode` (API 29+).
            None => {
                let bm = match blend {
                    BlendMode::ColorDodge => "COLOR_DODGE",
                    BlendMode::ColorBurn => "COLOR_BURN",
                    BlendMode::HardLight => "HARD_LIGHT",
                    BlendMode::SoftLight => "SOFT_LIGHT",
                    BlendMode::Difference => "DIFFERENCE",
                    BlendMode::Exclusion => "EXCLUSION",
                    BlendMode::Hue => "HUE",
                    BlendMode::Saturation => "SATURATION",
                    BlendMode::Color => "COLOR",
                    BlendMode::Luminosity => "LUMINOSITY",
                    // `#[non_exhaustive]`; unknown modes stay SrcOver.
                    _ => return,
                };
                self.set_android_blendmode(bm);
            }
        }
    }

    /// Install a `PorterDuffXfermode` for one of the `PorterDuff.Mode` blends.
    fn set_porterduff(&mut self, mode_field: &str) {
        let Ok(mode_class) = self.env.find_class("android/graphics/PorterDuff$Mode") else {
            return;
        };
        let Ok(mode) = self
            .env
            .get_static_field(&mode_class, mode_field, "Landroid/graphics/PorterDuff$Mode;")
            .and_then(|v| v.l())
        else {
            return;
        };
        let Ok(xfer_class) = self.env.find_class("android/graphics/PorterDuffXfermode") else {
            return;
        };
        let Ok(xfer) = self.env.new_object(
            &xfer_class,
            "(Landroid/graphics/PorterDuff$Mode;)V",
            &[JValue::Object(&mode)],
        ) else {
            return;
        };
        let p = self.paint.clone();
        let _ = self.env.call_method(
            p.as_obj(),
            "setXfermode",
            "(Landroid/graphics/Xfermode;)Landroid/graphics/Xfermode;",
            &[JValue::Object(&xfer)],
        );
    }

    /// Install (or clear) a dash pattern via `Paint.setPathEffect(DashPathEffect)`.
    /// `DashPathEffect` requires an even-length interval array, so an odd PDF dash
    /// array is duplicated (preserving the on/off alternation across the repeat).
    fn set_dash(&mut self, dash: &[f32], offset: f32) {
        let p = self.paint.clone();
        if dash.is_empty() {
            let _ = self.env.call_method(
                p.as_obj(),
                "setPathEffect",
                "(Landroid/graphics/PathEffect;)Landroid/graphics/PathEffect;",
                &[JValue::Object(&JObject::null())],
            );
            return;
        }
        let mut intervals = dash.to_vec();
        if intervals.len() % 2 == 1 {
            let dup = intervals.clone();
            intervals.extend(dup);
        }
        let Ok(arr) = self.env.new_float_array(intervals.len() as i32) else { return };
        if self.env.set_float_array_region(&arr, 0, &intervals).is_err() {
            return;
        }
        let Ok(effect_class) = self.env.find_class("android/graphics/DashPathEffect") else {
            return;
        };
        let Ok(effect) = self.env.new_object(
            &effect_class,
            "([FF)V",
            &[JValue::Object(&arr), JValue::Float(offset)],
        ) else {
            return;
        };
        let _ = self.env.call_method(
            p.as_obj(),
            "setPathEffect",
            "(Landroid/graphics/PathEffect;)Landroid/graphics/PathEffect;",
            &[JValue::Object(&effect)],
        );
    }

    /// Install an advanced blend via `Paint.setBlendMode(android.graphics.BlendMode)`
    /// (API 29+). On older API levels the class lookup fails and the op stays
    /// SrcOver — the documented degradation for the emulator-only CPU fallback
    /// (real devices render through vello, which supports every mode).
    fn set_android_blendmode(&mut self, mode_field: &str) {
        let Ok(bm_class) = self.env.find_class("android/graphics/BlendMode") else {
            return;
        };
        let Ok(mode) = self
            .env
            .get_static_field(&bm_class, mode_field, "Landroid/graphics/BlendMode;")
            .and_then(|v| v.l())
        else {
            return;
        };
        let p = self.paint.clone();
        let _ = self.env.call_method(
            p.as_obj(),
            "setBlendMode",
            "(Landroid/graphics/BlendMode;)V",
            &[JValue::Object(&mode)],
        );
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
