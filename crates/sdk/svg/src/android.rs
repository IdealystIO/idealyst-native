//! Android implementation of the SVG SDK — native vector renderer.
//!
//! Parses the markup with `usvg`, walks the resulting tree against a
//! recording `android.graphics.Canvas` (via `Picture.beginRecording`),
//! and wraps the recorded `Picture` in a `PictureDrawable` set on the
//! ImageView. No rasterization step — `PictureDrawable.draw(canvas)`
//! re-plays the recorded vector operations into the destination
//! canvas at the ImageView's current bounds every frame, so output
//! stays crisp through resize and transform.
//!
//! # Why `Picture` and not a custom `View.onDraw`?
//!
//! `Picture` is Android's native vector recording surface. It lets
//! the SDK ship without a Kotlin/Java shim — the drawable scales
//! automatically (its `draw(canvas)` does `canvas.scale(bounds.w /
//! pic.w, bounds.h / pic.h)` before replaying), and the ImageView
//! pipeline already knows how to host an arbitrary Drawable. A
//! custom View subclass would need a Kotlin source file shipped via
//! `[package.metadata.idealyst.android]` plus a JNI bridge, all to
//! match what Picture+PictureDrawable already do.
//!
//! # Per-instance lifetime
//!
//! Each Picture is built fresh on every markup change. The previous
//! Picture is referenced only by the previous PictureDrawable, which
//! the new `setImageDrawable` call releases — Android's GC collects
//! it. No manual cleanup needed.

use crate::tree_walker::{
    map_point, render_tree, MaskKind, Rect as SvgRect, StrokeParams, SvgPainter,
};
use crate::{SvgOps, SvgProps};
use backend_android::{with_jni_env, AndroidBackend};
use runtime_core::effect;

use jni::objects::{GlobalRef, JObject, JValue};
use jni::sys::{jfloat, jint};
use jni::JNIEnv;
use usvg::tiny_skia_path::{PathSegment, Point as SvgPoint};
use usvg::{Color as SvgColor, FillRule, LineCap, LineJoin, Transform};

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub(crate) static OPS: &dyn SvgOps = &AndroidSvgOps;

// Per-view intrinsic-size side table — keyed by GlobalRef raw
// pointer. Same leak caveat as the iOS impl (no Drop hook on the
// Java side); same "doesn't matter in practice" verdict.
thread_local! {
    static INTRINSIC_SIZES: RefCell<HashMap<usize, (f32, f32)>> =
        RefCell::new(HashMap::new());
}

fn global_ref_key(view: &GlobalRef) -> usize {
    view.as_obj().as_raw() as usize
}

fn store_intrinsic_size(view: &GlobalRef, size: (f32, f32)) {
    let key = global_ref_key(view);
    INTRINSIC_SIZES.with(|m| {
        m.borrow_mut().insert(key, size);
    });
}

fn read_intrinsic_size(view: &GlobalRef) -> Option<(f32, f32)> {
    let key = global_ref_key(view);
    INTRINSIC_SIZES.with(|m| m.borrow().get(&key).copied())
}

/// Register the SVG handler against an `AndroidBackend`. One-line call from
/// app bootstrap.
pub fn register(backend: &mut AndroidBackend) {
    backend.register_external::<SvgProps, _>(|props, b| build_svg(props, b));
}

// Self-register at backend construction (no app-side `register` call needed).
// See [[project_inventory_self_registration]].
inventory::submit! {
    backend_android::AndroidExternalRegistrar(register)
}

fn build_svg(props: &Rc<SvgProps>, b: &mut AndroidBackend) -> GlobalRef {
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
        // ScaleType.FIT_CENTER — preserves aspect ratio inside the
        // Taffy-assigned frame. PictureDrawable also scales to its
        // bounds, but the ImageView's scaleType decides how those
        // bounds map to the View's frame. FIT_CENTER matches iOS's
        // `scaleAspectFit` default.
        let scale_type_class = env
            .find_class("android/widget/ImageView$ScaleType")
            .expect("find_class ImageView$ScaleType");
        let fit_center = env
            .get_static_field(
                &scale_type_class,
                "FIT_CENTER",
                "Landroid/widget/ImageView$ScaleType;",
            )
            .expect("FIT_CENTER static field")
            .l()
            .expect("FIT_CENTER as object");
        let _ = env.call_method(
            &local,
            "setScaleType",
            "(Landroid/widget/ImageView$ScaleType;)V",
            &[JValue::Object(&fit_center)],
        );
        env.new_global_ref(local).expect("new_global_ref")
    });

    let view_for_effect = view.clone();
    let props_clone = props.clone();
    effect!({
        let markup = (props_clone.markup)();
        match usvg::Tree::from_str(&markup, &usvg::Options::default()) {
            Ok(tree) => {
                let size = tree.size();
                let intrinsic = (size.width(), size.height());
                store_intrinsic_size(&view_for_effect, intrinsic);
                paint_into_view(&view_for_effect, &tree, intrinsic);
                if let Some(cb) = &props_clone.on_load {
                    cb();
                }
            }
            Err(e) => {
                if let Some(cb) = &props_clone.on_error {
                    cb(format!("{e}"));
                }
            }
        }
    });

    view
}

/// Record the parsed tree into a fresh `Picture`, wrap it in a
/// `PictureDrawable`, and assign that drawable to the ImageView.
/// The drawable handles all subsequent layout-driven re-scaling
/// — we don't have to listen for size changes.
fn paint_into_view(view: &GlobalRef, tree: &usvg::Tree, intrinsic: (f32, f32)) {
    with_jni_env(|env| {
        // Round up so the Picture's natural size includes the SVG's
        // sub-pixel extents. PictureDrawable scales by
        // dst_bounds.w / picture.w; using ceil avoids a tiny down-
        // scale that would discard half a pixel of the rightmost
        // stroke.
        let pic_w = intrinsic.0.ceil() as i32;
        let pic_h = intrinsic.1.ceil() as i32;
        if pic_w <= 0 || pic_h <= 0 {
            return;
        }

        // Picture + recording canvas.
        let picture_class = match env.find_class("android/graphics/Picture") {
            Ok(c) => c,
            Err(_) => return,
        };
        let picture = match env.new_object(&picture_class, "()V", &[]) {
            Ok(p) => p,
            Err(_) => return,
        };
        let canvas = match env
            .call_method(
                &picture,
                "beginRecording",
                "(II)Landroid/graphics/Canvas;",
                &[JValue::Int(pic_w), JValue::Int(pic_h)],
            )
            .and_then(|v| v.l())
        {
            Ok(c) => c,
            Err(_) => return,
        };

        // Walk the tree into the recording canvas.
        let mut painter = match AndroidSvgPainter::new(env, &canvas) {
            Some(p) => p,
            None => return,
        };
        render_tree(&mut painter, tree);
        // Drop the painter early so the env mutable borrow is
        // released before we issue more calls. (Letting it survive
        // through `endRecording` would also work — the borrow is
        // already mutable — but explicit drop reads clearer.)
        drop(painter);

        let _ = env.call_method(&picture, "endRecording", "()V", &[]);

        // PictureDrawable(picture). Wraps the recorded Picture as a
        // Drawable that scales-to-bounds on each draw pass.
        let drawable_class = match env.find_class("android/graphics/drawable/PictureDrawable") {
            Ok(c) => c,
            Err(_) => return,
        };
        let drawable = match env.new_object(
            &drawable_class,
            "(Landroid/graphics/Picture;)V",
            &[JValue::Object(&picture)],
        ) {
            Ok(d) => d,
            Err(_) => return,
        };

        // ImageView.setImageDrawable replaces any previous drawable
        // and triggers a redraw via invalidate().
        let _ = env.call_method(
            view.as_obj(),
            "setImageDrawable",
            "(Landroid/graphics/drawable/Drawable;)V",
            &[JValue::Object(&drawable)],
        );

        // setLayerType(LAYER_TYPE_SOFTWARE, null) — PictureDrawable
        // doesn't render correctly on hardware-accelerated layers
        // (Android documents this in the `Picture` class header:
        // "drawing a picture to a hardware-accelerated canvas has
        // potential side effects"). Forcing software rendering for
        // this single view costs negligible performance for static
        // SVGs and is the documented workaround.
        const LAYER_TYPE_SOFTWARE: jint = 1;
        let _ = env.call_method(
            view.as_obj(),
            "setLayerType",
            "(ILandroid/graphics/Paint;)V",
            &[JValue::Int(LAYER_TYPE_SOFTWARE), JValue::Object(&JObject::null())],
        );
    });
}

// ============================================================================
// SvgPainter impl
// ============================================================================

// Two lifetimes:
// - `'p`: painter's own scope, controls how long `env` is borrowed
//   for. Always shorter than the JNI local frame.
// - `'env`: the JNI local frame the env was attached to. JObjects /
//   JClasses with this lifetime stay valid for the same frame.
//
// Mashing both into a single `'env` reads ergonomically but means the
// painter's mutable borrow on env lives as long as env itself, which
// blocks any code outside the painter (the surrounding
// `with_jni_env` closure) from using env again afterwards.
struct AndroidSvgPainter<'p, 'env> {
    env: &'p mut JNIEnv<'env>,
    canvas: &'p JObject<'env>,
    /// Cached `Paint.Style.FILL` and `STROKE` references. Looked up
    /// once at painter construction so each fill/stroke doesn't
    /// re-walk the static field tables.
    fill_style: GlobalRef,
    stroke_style: GlobalRef,
    /// Reusable Paint object. Reset before every fill/stroke. JNI
    /// object allocation is the expensive part of per-path painting;
    /// keeping one Paint and reconfiguring it is the standard
    /// Android perf pattern (cf. `Canvas.drawText` examples).
    paint: GlobalRef,
}

impl<'p, 'env> AndroidSvgPainter<'p, 'env> {
    fn new(env: &'p mut JNIEnv<'env>, canvas: &'p JObject<'env>) -> Option<Self> {
        let paint = make_paint(env)?;
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
        let paint_global = env.new_global_ref(&paint).ok()?;
        Some(Self {
            env,
            canvas,
            fill_style,
            stroke_style,
            paint: paint_global,
        })
    }

    fn reset_paint(&mut self) {
        // `paint.reset()` clears antialias / style / color / shader /
        // stroke width / dash effect to defaults. Re-enable antialias
        // (the framework default is false; we always want it on).
        let _ = self
            .env
            .call_method(self.paint.as_obj(), "reset", "()V", &[]);
        let _ = self.env.call_method(
            self.paint.as_obj(),
            "setAntiAlias",
            "(Z)V",
            &[JValue::Bool(1)],
        );
    }

    fn set_paint_color_argb(&mut self, argb: i32) {
        let _ = self.env.call_method(
            self.paint.as_obj(),
            "setColor",
            "(I)V",
            &[JValue::Int(argb)],
        );
    }

    fn set_paint_style_fill(&mut self) {
        let style = self.fill_style.clone();
        let _ = self.env.call_method(
            self.paint.as_obj(),
            "setStyle",
            "(Landroid/graphics/Paint$Style;)V",
            &[JValue::Object(style.as_obj())],
        );
    }

    fn set_paint_style_stroke(&mut self) {
        let style = self.stroke_style.clone();
        let _ = self.env.call_method(
            self.paint.as_obj(),
            "setStyle",
            "(Landroid/graphics/Paint$Style;)V",
            &[JValue::Object(style.as_obj())],
        );
    }

    fn draw_path_with_paint(&mut self, path: &JObject<'env>) {
        let _ = self.env.call_method(
            self.canvas,
            "drawPath",
            "(Landroid/graphics/Path;Landroid/graphics/Paint;)V",
            &[
                JValue::Object(path),
                JValue::Object(self.paint.as_obj()),
            ],
        );
    }
}

impl<'p, 'env> SvgPainter for AndroidSvgPainter<'p, 'env> {
    type Path = GlobalRef;

    fn build_path<I: Iterator<Item = PathSegment>>(&mut self, segments: I) -> Self::Path {
        let path_class = self
            .env
            .find_class("android/graphics/Path")
            .expect("find_class android/graphics/Path");
        let path = self
            .env
            .new_object(&path_class, "()V", &[])
            .expect("new Path()");

        for seg in segments {
            match seg {
                PathSegment::MoveTo(p) => {
                    let _ = self.env.call_method(
                        &path,
                        "moveTo",
                        "(FF)V",
                        &[JValue::Float(p.x), JValue::Float(p.y)],
                    );
                }
                PathSegment::LineTo(p) => {
                    let _ = self.env.call_method(
                        &path,
                        "lineTo",
                        "(FF)V",
                        &[JValue::Float(p.x), JValue::Float(p.y)],
                    );
                }
                PathSegment::QuadTo(c, p) => {
                    let _ = self.env.call_method(
                        &path,
                        "quadTo",
                        "(FFFF)V",
                        &[
                            JValue::Float(c.x),
                            JValue::Float(c.y),
                            JValue::Float(p.x),
                            JValue::Float(p.y),
                        ],
                    );
                }
                PathSegment::CubicTo(c1, c2, p) => {
                    let _ = self.env.call_method(
                        &path,
                        "cubicTo",
                        "(FFFFFF)V",
                        &[
                            JValue::Float(c1.x),
                            JValue::Float(c1.y),
                            JValue::Float(c2.x),
                            JValue::Float(c2.y),
                            JValue::Float(p.x),
                            JValue::Float(p.y),
                        ],
                    );
                }
                PathSegment::Close => {
                    let _ = self.env.call_method(&path, "close", "()V", &[]);
                }
            }
        }

        self.env
            .new_global_ref(&path)
            .expect("new_global_ref(Path)")
    }

    fn fill_solid(
        &mut self,
        path: &Self::Path,
        color: SvgColor,
        opacity: f32,
        rule: FillRule,
    ) {
        apply_fill_rule(self.env, path.as_obj(), rule);
        self.reset_paint();
        self.set_paint_style_fill();
        self.set_paint_color_argb(svg_color_to_argb(color, opacity));
        // `local` is a borrowed `JObject` view of the `GlobalRef` path,
        // re-tagged with the env lifetime that `draw_path_with_paint`
        // wants. `JObject` has no `Drop` (only `GlobalRef`/`AutoLocal`/
        // `WeakRef` free in jni 0.21), so it just falls out of scope — no
        // `mem::forget` is needed to avoid double-freeing the GlobalRef.
        let local = unsafe { JObject::from_raw(path.as_obj().as_raw()) };
        self.draw_path_with_paint(&local);
    }

    fn fill_linear_gradient(
        &mut self,
        path: &Self::Path,
        gradient: &usvg::LinearGradient,
        opacity: f32,
        rule: FillRule,
    ) {
        apply_fill_rule(self.env, path.as_obj(), rule);
        self.reset_paint();
        self.set_paint_style_fill();

        let t = gradient.transform();
        let start = map_point(t, SvgPoint { x: gradient.x1(), y: gradient.y1() });
        let end = map_point(t, SvgPoint { x: gradient.x2(), y: gradient.y2() });
        let shader = build_linear_shader(self.env, gradient.stops(), opacity, start, end);
        if let Some(sh) = shader {
            let _ = self.env.call_method(
                self.paint.as_obj(),
                "setShader",
                "(Landroid/graphics/Shader;)Landroid/graphics/Shader;",
                &[JValue::Object(&sh)],
            );
        }
        let local = unsafe { JObject::from_raw(path.as_obj().as_raw()) };
        // No `mem::forget`: `JObject` is a borrowed handle with no `Drop`
        // (see the fill-path note above).
        self.draw_path_with_paint(&local);
    }

    fn fill_radial_gradient(
        &mut self,
        path: &Self::Path,
        gradient: &usvg::RadialGradient,
        opacity: f32,
        rule: FillRule,
    ) {
        apply_fill_rule(self.env, path.as_obj(), rule);
        self.reset_paint();
        self.set_paint_style_fill();

        let t = gradient.transform();
        let center = map_point(t, SvgPoint { x: gradient.cx(), y: gradient.cy() });
        // Average scale magnitude for the radius — same approximation
        // as the iOS impl, exact for non-skewed transforms.
        let scale_avg = ((t.sx * t.sx + t.ky * t.ky).sqrt()
            + (t.kx * t.kx + t.sy * t.sy).sqrt())
            * 0.5;
        let radius = gradient.r().get() * scale_avg;
        let shader = build_radial_shader(self.env, gradient.stops(), opacity, center, radius);
        if let Some(sh) = shader {
            let _ = self.env.call_method(
                self.paint.as_obj(),
                "setShader",
                "(Landroid/graphics/Shader;)Landroid/graphics/Shader;",
                &[JValue::Object(&sh)],
            );
        }
        let local = unsafe { JObject::from_raw(path.as_obj().as_raw()) };
        // No `mem::forget`: `JObject` is a borrowed handle with no `Drop`
        // (see the fill-path note above).
        self.draw_path_with_paint(&local);
    }

    fn stroke_solid(
        &mut self,
        path: &Self::Path,
        color: SvgColor,
        opacity: f32,
        params: StrokeParams,
    ) {
        self.reset_paint();
        self.set_paint_style_stroke();
        self.set_paint_color_argb(svg_color_to_argb(color, opacity));
        let _ = self.env.call_method(
            self.paint.as_obj(),
            "setStrokeWidth",
            "(F)V",
            &[JValue::Float(params.width)],
        );
        let _ = self.env.call_method(
            self.paint.as_obj(),
            "setStrokeMiter",
            "(F)V",
            &[JValue::Float(params.miter_limit)],
        );
        set_stroke_cap(self.env, self.paint.as_obj(), params.linecap);
        set_stroke_join(self.env, self.paint.as_obj(), params.linejoin);
        if let Some(dash) = params.dasharray {
            apply_dash_effect(self.env, self.paint.as_obj(), dash, params.dashoffset);
        }
        let local = unsafe { JObject::from_raw(path.as_obj().as_raw()) };
        // No `mem::forget`: `JObject` is a borrowed handle with no `Drop`
        // (see the fill-path note above).
        self.draw_path_with_paint(&local);
    }

    fn with_transform<R>(
        &mut self,
        transform: Transform,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        // canvas.save() returns the save count, used by `restoreToCount`.
        // We pair with `restore()` (pops the most recent save) which
        // is equivalent for the single-save case.
        let _ = self.env.call_method(self.canvas, "save", "()I", &[]);
        let matrix = make_matrix(self.env, transform);
        if let Some(m) = matrix {
            let _ = self.env.call_method(
                self.canvas,
                "concat",
                "(Landroid/graphics/Matrix;)V",
                &[JValue::Object(&m)],
            );
        }
        let result = f(self);
        let _ = self.env.call_method(self.canvas, "restore", "()V", &[]);
        result
    }

    fn extend_path<I: Iterator<Item = PathSegment>>(
        &mut self,
        dest: &mut Self::Path,
        segments: I,
        t: Transform,
    ) {
        // Apply `t` per-segment ourselves rather than building a new
        // path + transforming it. Android's `Path.transform(Matrix)`
        // would also work (transforms the whole path in place) but
        // requires a temporary Path object per extend call — more JNI
        // round-trips than transforming the points ourselves.
        let path_ref = dest.as_obj();
        for seg in segments {
            match seg {
                PathSegment::MoveTo(p) => {
                    let q = map_point(t, p);
                    let _ = self.env.call_method(
                        path_ref,
                        "moveTo",
                        "(FF)V",
                        &[JValue::Float(q.x), JValue::Float(q.y)],
                    );
                }
                PathSegment::LineTo(p) => {
                    let q = map_point(t, p);
                    let _ = self.env.call_method(
                        path_ref,
                        "lineTo",
                        "(FF)V",
                        &[JValue::Float(q.x), JValue::Float(q.y)],
                    );
                }
                PathSegment::QuadTo(c, p) => {
                    let qc = map_point(t, c);
                    let qp = map_point(t, p);
                    let _ = self.env.call_method(
                        path_ref,
                        "quadTo",
                        "(FFFF)V",
                        &[
                            JValue::Float(qc.x),
                            JValue::Float(qc.y),
                            JValue::Float(qp.x),
                            JValue::Float(qp.y),
                        ],
                    );
                }
                PathSegment::CubicTo(c1, c2, p) => {
                    let qc1 = map_point(t, c1);
                    let qc2 = map_point(t, c2);
                    let qp = map_point(t, p);
                    let _ = self.env.call_method(
                        path_ref,
                        "cubicTo",
                        "(FFFFFF)V",
                        &[
                            JValue::Float(qc1.x),
                            JValue::Float(qc1.y),
                            JValue::Float(qc2.x),
                            JValue::Float(qc2.y),
                            JValue::Float(qp.x),
                            JValue::Float(qp.y),
                        ],
                    );
                }
                PathSegment::Close => {
                    let _ = self.env.call_method(path_ref, "close", "()V", &[]);
                }
            }
        }
    }

    fn draw_image(&mut self, kind: &usvg::ImageKind, dst_rect: SvgRect) {
        let bytes: &[u8] = match kind {
            usvg::ImageKind::JPEG(b) | usvg::ImageKind::PNG(b) | usvg::ImageKind::GIF(b) => {
                b.as_slice()
            }
            // ImageKind::SVG is handled by the walker before reaching us.
            usvg::ImageKind::SVG(_) => return,
        };
        let bitmap = match decode_bytes_to_bitmap(self.env, bytes) {
            Some(b) => b,
            None => return,
        };
        // Canvas.drawBitmap(Bitmap, src_rect, dst_rect, Paint)
        //   src = null (whole image), dst = our target RectF.
        let rectf = match make_rectf(self.env, dst_rect) {
            Some(r) => r,
            None => return,
        };
        self.reset_paint();
        // Filter bitmap on scaling — looks much better than nearest
        // for the typical "raster icon that we're stretching to view
        // size" case. setFilterBitmap is a Paint property.
        let _ = self.env.call_method(
            self.paint.as_obj(),
            "setFilterBitmap",
            "(Z)V",
            &[JValue::Bool(1)],
        );
        let _ = self.env.call_method(
            self.canvas,
            "drawBitmap",
            "(Landroid/graphics/Bitmap;Landroid/graphics/Rect;Landroid/graphics/RectF;Landroid/graphics/Paint;)V",
            &[
                JValue::Object(&bitmap),
                JValue::Object(&JObject::null()),
                JValue::Object(&rectf),
                JValue::Object(self.paint.as_obj()),
            ],
        );
    }

    fn with_clip<R>(
        &mut self,
        clip: &Self::Path,
        rule: FillRule,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        apply_fill_rule(self.env, clip.as_obj(), rule);
        let _ = self.env.call_method(self.canvas, "save", "()I", &[]);
        let _ = self.env.call_method(
            self.canvas,
            "clipPath",
            "(Landroid/graphics/Path;)Z",
            &[JValue::Object(clip.as_obj())],
        );
        let result = f(self);
        let _ = self.env.call_method(self.canvas, "restore", "()V", &[]);
        result
    }

    fn with_opacity<R>(&mut self, alpha: f32, f: impl FnOnce(&mut Self) -> R) -> R {
        // `saveLayerAlpha(RectF bounds, int alpha)` — bounds=null
        // means "the whole current clip". The layer composites at
        // `alpha/255` on restore, so children render at full alpha
        // into the layer and the layer fades on composite. Same
        // semantic as iOS's `TransparencyLayer + SetAlpha`.
        let alpha_int = (alpha.clamp(0.0, 1.0) * 255.0).round() as jint;
        let _ = self.env.call_method(
            self.canvas,
            "saveLayerAlpha",
            "(Landroid/graphics/RectF;I)I",
            &[JValue::Object(&JObject::null()), JValue::Int(alpha_int)],
        );
        let result = f(self);
        let _ = self.env.call_method(self.canvas, "restore", "()V", &[]);
        result
    }

    fn with_mask<R>(
        &mut self,
        kind: MaskKind,
        dst_rect: SvgRect,
        render_mask: impl FnOnce(&mut Self),
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        // Strategy on Android:
        //
        // 1. saveLayer (outer L1) — collects the masked-content's
        //    pixels in an offscreen.
        // 2. Run `f` — draws content into L1.
        // 3. saveLayer (inner L2) with a Paint whose Xfermode is
        //    `PorterDuff.Mode.DST_IN`. DST_IN means "destination
        //    pixels are kept only where the source has alpha" — so
        //    when the inner layer composites onto the outer, content
        //    survives only where the mask covers it.
        // 4. Run `render_mask` — draws into L2.
        // 5. If `kind == Luminance`, we need to convert the mask's
        //    RGB into alpha before compositing. Apply a
        //    ColorMatrixColorFilter on the L2 paint that multiplies
        //    luminance into alpha. (The matrix `[0,0,0,0,0,
        //    0,0,0,0,0, 0,0,0,0,0, 0.2126,0.7152,0.0722,0,0]` zeros
        //    out RGB, leaving only a luminance-derived alpha.)
        // 6. Restore L2 — composites mask alpha into L1 via DST_IN.
        // 7. Restore L1 — composites the masked result onto the
        //    parent canvas.
        //
        // `dst_rect` could be used to scope the saveLayer's bounds
        // for memory efficiency, but passing `null` lets Android
        // size the layer to the current clip — simpler and avoids
        // re-computing transforms when the parent CTM is non-trivial.
        let _ = dst_rect; // bounds=null path, see above.

        // Outer layer L1.
        let _ = self.env.call_method(
            self.canvas,
            "saveLayer",
            "(Landroid/graphics/RectF;Landroid/graphics/Paint;)I",
            &[
                JValue::Object(&JObject::null()),
                JValue::Object(&JObject::null()),
            ],
        );
        // Content into L1.
        let result = f(self);

        // Build the Xfermode + (if luminance) ColorFilter Paint for L2.
        if let Some(mask_paint) = build_mask_paint(self.env, kind) {
            let _ = self.env.call_method(
                self.canvas,
                "saveLayer",
                "(Landroid/graphics/RectF;Landroid/graphics/Paint;)I",
                &[
                    JValue::Object(&JObject::null()),
                    JValue::Object(&mask_paint),
                ],
            );
            // Mask content into L2.
            render_mask(self);
            let _ = self.env.call_method(self.canvas, "restore", "()V", &[]);
        }
        // Restore L1.
        let _ = self.env.call_method(self.canvas, "restore", "()V", &[]);
        result
    }
}

// ============================================================================
// JNI helpers
// ============================================================================

fn make_paint<'env>(env: &mut JNIEnv<'env>) -> Option<JObject<'env>> {
    let paint_class = env.find_class("android/graphics/Paint").ok()?;
    let paint = env.new_object(&paint_class, "()V", &[]).ok()?;
    // setAntiAlias(true) — framework default is false. SVG renders
    // with antialiasing in every other engine; matching that.
    let _ = env.call_method(&paint, "setAntiAlias", "(Z)V", &[JValue::Bool(1)]);
    Some(paint)
}

fn svg_color_to_argb(c: SvgColor, opacity: f32) -> i32 {
    let a = (opacity.clamp(0.0, 1.0) * 255.0).round() as i32;
    let r = c.red as i32;
    let g = c.green as i32;
    let b = c.blue as i32;
    (a << 24) | (r << 16) | (g << 8) | b
}

fn apply_fill_rule(env: &mut JNIEnv, path: &JObject, rule: FillRule) {
    // android.graphics.Path.FillType: WINDING (= NonZero), EVEN_ODD.
    let class = match env.find_class("android/graphics/Path$FillType") {
        Ok(c) => c,
        Err(_) => return,
    };
    let field = match rule {
        FillRule::NonZero => "WINDING",
        FillRule::EvenOdd => "EVEN_ODD",
    };
    let value = match env
        .get_static_field(&class, field, "Landroid/graphics/Path$FillType;")
        .and_then(|v| v.l())
    {
        Ok(v) => v,
        Err(_) => return,
    };
    let _ = env.call_method(
        path,
        "setFillType",
        "(Landroid/graphics/Path$FillType;)V",
        &[JValue::Object(&value)],
    );
}

fn set_stroke_cap(env: &mut JNIEnv, paint: &JObject, cap: LineCap) {
    let class = match env.find_class("android/graphics/Paint$Cap") {
        Ok(c) => c,
        Err(_) => return,
    };
    let field = match cap {
        LineCap::Butt => "BUTT",
        LineCap::Round => "ROUND",
        LineCap::Square => "SQUARE",
    };
    if let Ok(value) = env
        .get_static_field(&class, field, "Landroid/graphics/Paint$Cap;")
        .and_then(|v| v.l())
    {
        let _ = env.call_method(
            paint,
            "setStrokeCap",
            "(Landroid/graphics/Paint$Cap;)V",
            &[JValue::Object(&value)],
        );
    }
}

fn set_stroke_join(env: &mut JNIEnv, paint: &JObject, join: LineJoin) {
    let class = match env.find_class("android/graphics/Paint$Join") {
        Ok(c) => c,
        Err(_) => return,
    };
    let field = match join {
        // Android has no MiterClip equivalent — substitute Miter,
        // same visual at typical stroke widths since the miter limit
        // clamps the spike.
        LineJoin::Miter | LineJoin::MiterClip => "MITER",
        LineJoin::Round => "ROUND",
        LineJoin::Bevel => "BEVEL",
    };
    if let Ok(value) = env
        .get_static_field(&class, field, "Landroid/graphics/Paint$Join;")
        .and_then(|v| v.l())
    {
        let _ = env.call_method(
            paint,
            "setStrokeJoin",
            "(Landroid/graphics/Paint$Join;)V",
            &[JValue::Object(&value)],
        );
    }
}

fn apply_dash_effect(env: &mut JNIEnv, paint: &JObject, dash: &[f32], offset: f32) {
    if dash.is_empty() {
        return;
    }
    // android.graphics.DashPathEffect(float[] intervals, float phase)
    let arr = match env.new_float_array(dash.len() as jni::sys::jsize) {
        Ok(a) => a,
        Err(_) => return,
    };
    if env
        .set_float_array_region(
            &arr,
            0,
            // safe transmute: jfloat is f32
            unsafe { std::slice::from_raw_parts(dash.as_ptr() as *const jfloat, dash.len()) },
        )
        .is_err()
    {
        return;
    }
    let class = match env.find_class("android/graphics/DashPathEffect") {
        Ok(c) => c,
        Err(_) => return,
    };
    let effect = match env.new_object(
        &class,
        "([FF)V",
        &[
            JValue::Object(&JObject::from(arr)),
            JValue::Float(offset),
        ],
    ) {
        Ok(e) => e,
        Err(_) => return,
    };
    let _ = env.call_method(
        paint,
        "setPathEffect",
        "(Landroid/graphics/PathEffect;)Landroid/graphics/PathEffect;",
        &[JValue::Object(&effect)],
    );
}

fn make_matrix<'env>(env: &mut JNIEnv<'env>, t: Transform) -> Option<JObject<'env>> {
    let class = env.find_class("android/graphics/Matrix").ok()?;
    let matrix = env.new_object(&class, "()V", &[]).ok()?;
    // setValues(float[9]) — row-major as Android documents:
    //   [MSCALE_X, MSKEW_X,  MTRANS_X,
    //    MSKEW_Y,  MSCALE_Y, MTRANS_Y,
    //    MPERSP_0, MPERSP_1, MPERSP_2]
    // tiny_skia is also column-major-column-vector, so sx→MSCALE_X,
    // kx→MSKEW_X, tx→MTRANS_X, ky→MSKEW_Y, sy→MSCALE_Y, ty→MTRANS_Y.
    let vals: [jfloat; 9] = [
        t.sx as jfloat,
        t.kx as jfloat,
        t.tx as jfloat,
        t.ky as jfloat,
        t.sy as jfloat,
        t.ty as jfloat,
        0.0,
        0.0,
        1.0,
    ];
    let arr = env.new_float_array(9).ok()?;
    env.set_float_array_region(&arr, 0, &vals).ok()?;
    env.call_method(
        &matrix,
        "setValues",
        "([F)V",
        &[JValue::Object(&JObject::from(arr))],
    )
    .ok()?;
    Some(matrix)
}

/// Build a `LinearGradient` shader from a usvg stop list. Caller is
/// responsible for `setShader` on the destination Paint.
fn build_linear_shader<'env>(
    env: &mut JNIEnv<'env>,
    stops: &[usvg::Stop],
    opacity: f32,
    start: SvgPoint,
    end: SvgPoint,
) -> Option<JObject<'env>> {
    if stops.is_empty() {
        return None;
    }
    let (colors_arr, positions_arr) = stop_arrays(env, stops, opacity)?;
    let class = env.find_class("android/graphics/LinearGradient").ok()?;
    // Constructor: LinearGradient(float x0, float y0, float x1, float y1,
    //                              int[] colors, float[] positions,
    //                              Shader.TileMode tile);
    let tile_class = env.find_class("android/graphics/Shader$TileMode").ok()?;
    let clamp = env
        .get_static_field(&tile_class, "CLAMP", "Landroid/graphics/Shader$TileMode;")
        .ok()?
        .l()
        .ok()?;
    env.new_object(
        &class,
        "(FFFF[I[FLandroid/graphics/Shader$TileMode;)V",
        &[
            JValue::Float(start.x),
            JValue::Float(start.y),
            JValue::Float(end.x),
            JValue::Float(end.y),
            JValue::Object(&JObject::from(colors_arr)),
            JValue::Object(&JObject::from(positions_arr)),
            JValue::Object(&clamp),
        ],
    )
    .ok()
}

fn build_radial_shader<'env>(
    env: &mut JNIEnv<'env>,
    stops: &[usvg::Stop],
    opacity: f32,
    center: SvgPoint,
    radius: f32,
) -> Option<JObject<'env>> {
    if stops.is_empty() || radius <= 0.0 {
        return None;
    }
    let (colors_arr, positions_arr) = stop_arrays(env, stops, opacity)?;
    let class = env.find_class("android/graphics/RadialGradient").ok()?;
    let tile_class = env.find_class("android/graphics/Shader$TileMode").ok()?;
    let clamp = env
        .get_static_field(&tile_class, "CLAMP", "Landroid/graphics/Shader$TileMode;")
        .ok()?
        .l()
        .ok()?;
    // RadialGradient(float centerX, float centerY, float radius,
    //                int[] colors, float[] stops, Shader.TileMode tileMode)
    // Android's RadialGradient doesn't have a focal-point variant
    // until API 33 (RadialGradient.setFocal). For broader compatibility
    // we ignore the SVG focal (fx, fy) and center the gradient — most
    // SVG radial gradients have fx==cx and fy==cy anyway.
    env.new_object(
        &class,
        "(FFF[I[FLandroid/graphics/Shader$TileMode;)V",
        &[
            JValue::Float(center.x),
            JValue::Float(center.y),
            JValue::Float(radius),
            JValue::Object(&JObject::from(colors_arr)),
            JValue::Object(&JObject::from(positions_arr)),
            JValue::Object(&clamp),
        ],
    )
    .ok()
}

/// Decode raw PNG/JPEG/GIF bytes into an `android.graphics.Bitmap`.
/// Returns `None` on malformed input.
fn decode_bytes_to_bitmap<'env>(
    env: &mut JNIEnv<'env>,
    bytes: &[u8],
) -> Option<JObject<'env>> {
    let java_bytes = env.byte_array_from_slice(bytes).ok()?;
    let factory = env.find_class("android/graphics/BitmapFactory").ok()?;
    let bitmap = env
        .call_static_method(
            &factory,
            "decodeByteArray",
            "([BII)Landroid/graphics/Bitmap;",
            &[
                JValue::Object(&JObject::from(java_bytes)),
                JValue::Int(0),
                JValue::Int(bytes.len() as jint),
            ],
        )
        .ok()?
        .l()
        .ok()?;
    if bitmap.is_null() {
        return None;
    }
    Some(bitmap)
}

/// `android.graphics.RectF` constructed from our internal `SvgRect`.
fn make_rectf<'env>(env: &mut JNIEnv<'env>, r: SvgRect) -> Option<JObject<'env>> {
    let class = env.find_class("android/graphics/RectF").ok()?;
    env.new_object(
        &class,
        "(FFFF)V",
        &[
            JValue::Float(r.x),
            JValue::Float(r.y),
            JValue::Float(r.x + r.width),
            JValue::Float(r.y + r.height),
        ],
    )
    .ok()
}

/// Build the Paint object used for the inner saveLayer when
/// compositing a mask. Configures DST_IN xfermode and (for luminance
/// masks) a ColorMatrixColorFilter that re-channels RGB → A using
/// the BT.709 luminance coefficients. The returned Paint is a fresh
/// JObject — caller is responsible for keeping it alive while the
/// saveLayer is active.
fn build_mask_paint<'env>(env: &mut JNIEnv<'env>, kind: MaskKind) -> Option<JObject<'env>> {
    let paint = make_paint(env)?;
    // PorterDuffXfermode(PorterDuff.Mode.DST_IN). Construct the mode
    // via `PorterDuff.Mode.valueOf("DST_IN")` — simpler than walking
    // enum ordinals and works across all API levels.
    let mode_class = env.find_class("android/graphics/PorterDuff$Mode").ok()?;
    let dst_in = env
        .get_static_field(&mode_class, "DST_IN", "Landroid/graphics/PorterDuff$Mode;")
        .ok()?
        .l()
        .ok()?;
    let xfermode_class = env.find_class("android/graphics/PorterDuffXfermode").ok()?;
    let xfermode = env
        .new_object(
            &xfermode_class,
            "(Landroid/graphics/PorterDuff$Mode;)V",
            &[JValue::Object(&dst_in)],
        )
        .ok()?;
    let _ = env.call_method(
        &paint,
        "setXfermode",
        "(Landroid/graphics/Xfermode;)Landroid/graphics/Xfermode;",
        &[JValue::Object(&xfermode)],
    );

    if matches!(kind, MaskKind::Luminance) {
        // 4×5 column-major-ish color matrix that zeros out R/G/B
        // and puts BT.709 luminance into the alpha channel. The 5th
        // column is constant-add per channel (we use 0).
        // Layout (row-major):
        //   R' = 0*R + 0*G + 0*B + 0*A + 0
        //   G' = ...
        //   B' = ...
        //   A' = 0.2126*R + 0.7152*G + 0.0722*B + 0*A + 0
        let coeffs: [jfloat; 20] = [
            0.0, 0.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 0.0, 0.0,
            0.2126, 0.7152, 0.0722, 0.0, 0.0,
        ];
        let arr = env.new_float_array(20).ok()?;
        env.set_float_array_region(&arr, 0, &coeffs).ok()?;
        let cm_class = env.find_class("android/graphics/ColorMatrix").ok()?;
        let color_matrix = env
            .new_object(&cm_class, "([F)V", &[JValue::Object(&JObject::from(arr))])
            .ok()?;
        let filter_class = env
            .find_class("android/graphics/ColorMatrixColorFilter")
            .ok()?;
        let filter = env
            .new_object(
                &filter_class,
                "(Landroid/graphics/ColorMatrix;)V",
                &[JValue::Object(&color_matrix)],
            )
            .ok()?;
        let _ = env.call_method(
            &paint,
            "setColorFilter",
            "(Landroid/graphics/ColorFilter;)Landroid/graphics/ColorFilter;",
            &[JValue::Object(&filter)],
        );
    }

    Some(paint)
}

fn stop_arrays<'env>(
    env: &mut JNIEnv<'env>,
    stops: &[usvg::Stop],
    opacity: f32,
) -> Option<(jni::objects::JIntArray<'env>, jni::objects::JFloatArray<'env>)> {
    let colors: Vec<jint> = stops
        .iter()
        .map(|s| svg_color_to_argb(s.color(), s.opacity().get() * opacity))
        .collect();
    let positions: Vec<jfloat> =
        stops.iter().map(|s| s.offset().get() as jfloat).collect();
    let colors_arr = env.new_int_array(colors.len() as jni::sys::jsize).ok()?;
    env.set_int_array_region(&colors_arr, 0, &colors).ok()?;
    let positions_arr = env
        .new_float_array(positions.len() as jni::sys::jsize)
        .ok()?;
    env.set_float_array_region(&positions_arr, 0, &positions)
        .ok()?;
    Some((colors_arr, positions_arr))
}

// ============================================================================
// Imperative ops
// ============================================================================

struct AndroidSvgOps;

impl SvgOps for AndroidSvgOps {
    fn intrinsic_size(&self, node: &dyn Any) -> Option<(f32, f32)> {
        let view = node.downcast_ref::<GlobalRef>()?;
        read_intrinsic_size(view)
    }
}
