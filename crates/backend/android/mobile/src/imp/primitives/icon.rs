//! `Element::Icon` — renders vector paths via Android's Path/Canvas
//! system inside an ImageView with a custom Drawable.
//!
//! Strategy:
//! - Parse SVG path `d` strings into `android.graphics.Path` objects
//! - Wrap in a custom drawable that strokes the paths
//! - Display in an `ImageView` (standalone icon) or set as compound
//!   drawable on a `Button`/`TextView` (button icon)
//!
//! Stroke animation uses `ObjectAnimator` targeting a custom property
//! that maps to `DashPathEffect` manipulation.

use backend_android_core::helpers::apply_default_layout_params;
use crate::imp::{with_env, AndroidBackend};
use runtime_core::primitives::icon::IconData;
use runtime_core::Color;
use jni::objects::{GlobalRef, JObject, JValue};

/// Create an ImageView displaying the icon paths as a stroked drawable.
pub(crate) fn create(b: &AndroidBackend, data: &IconData, color: Option<&Color>) -> GlobalRef {
    with_env(|env| {
        // Create ImageView.
        let iv_class = env.find_class("android/widget/ImageView").unwrap();
        let image_view = env
            .new_object(
                &iv_class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .unwrap();

        // Build the drawable from icon path data. Pass the display
        // density so the drawable's intrinsic size is the real render
        // size in device pixels (24dp) — the Taffy measure_fn reads that
        // intrinsic size, so it must be dp-correct, not viewBox units.
        let density = get_density(env, &b.context);
        let drawable = build_icon_drawable(env, data, color, density);
        env.call_method(
            &image_view,
            "setImageDrawable",
            "(Landroid/graphics/drawable/Drawable;)V",
            &[JValue::Object(&drawable)],
        )
        .unwrap();

        // ScaleType.FIT_CENTER = 6
        let scale_type_class = env
            .find_class("android/widget/ImageView$ScaleType")
            .unwrap();
        let fit_center = env
            .get_static_field(&scale_type_class, "FIT_CENTER", "Landroid/widget/ImageView$ScaleType;")
            .and_then(|v| v.l());
        if let Ok(st) = fit_center {
            let _ = env.call_method(
                &image_view,
                "setScaleType",
                "(Landroid/widget/ImageView$ScaleType;)V",
                &[JValue::Object(&st)],
            );
        }

        // Margin-capable default LayoutParams (FrameLayout.LayoutParams /
        // MarginLayoutParams) so the layout pass can position the icon via
        // `leftMargin`/`topMargin` from its Taffy frame. We deliberately do
        // NOT overwrite this with a fixed `ViewGroup.LayoutParams(24,24)`:
        // that base class has no margin fields, so a centered Taffy frame
        // (e.g. (6,6,24,24) inside a 36×36 menu button) couldn't be applied
        // and the icon pinned to the parent's top-left corner. The 24dp
        // size now comes from the Taffy measure_fn (`install_external_measure_fn`,
        // which reads the drawable's intrinsic size) + the apply-frames
        // width/height, exactly like every other view.
        apply_default_layout_params(env, &image_view);

        env.new_global_ref(image_view).unwrap()
    })
}

/// Update the icon's stroke color via the drawable's paint.
pub(crate) fn update_color(node: &GlobalRef, color: &Color) {
    with_env(|env| {
        let drawable: Result<JObject, _> = env
            .call_method(node.as_obj(), "getDrawable", "()Landroid/graphics/drawable/Drawable;", &[])
            .and_then(|v| v.l());
        if let Ok(d) = drawable {
            let argb = parse_color_to_argb(&color.0);
            // setTint(int color) on Drawable
            let _ = env.call_method(&d, "setTint", "(I)V", &[JValue::Int(argb)]);
            let _ = env.call_method(node.as_obj(), "invalidate", "()V", &[]);
        }
    });
}

/// Set stroke progress (0.0–1.0) by adjusting the drawable's trim.
pub(crate) fn update_stroke(node: &GlobalRef, progress: f32) {
    with_env(|env| {
        let drawable: Result<JObject, _> = env
            .call_method(node.as_obj(), "getDrawable", "()Landroid/graphics/drawable/Drawable;", &[])
            .and_then(|v| v.l());
        if let Ok(d) = drawable {
            // Set level (0–10000) to represent stroke progress.
            // ShapeDrawable/custom drawables can use this.
            //
            // NB: `Drawable.setLevel(int)` returns `boolean` (true if
            // the level change altered the drawable's appearance) —
            // its JNI signature is `(I)Z`, NOT `(I)V`. Using `(I)V`
            // makes `GetMethodID` fail to resolve and raises a
            // pending `NoSuchMethodError` that aborts the ART runtime
            // shortly after — visible as a SIGABRT during the
            // sidebar build path's icon stroke animator startup. The
            // returned bool is ignored (Android re-draws on level
            // change regardless via the subsequent `invalidate()`).
            let level = (progress.clamp(0.0, 1.0) * 10000.0) as i32;
            let _ = env.call_method(&d, "setLevel", "(I)Z", &[JValue::Int(level)]);
            let _ = env.call_method(node.as_obj(), "invalidate", "()V", &[]);
        }
    });
}

/// Animate stroke from→to using ObjectAnimator on the drawable's level.
pub(crate) fn animate_stroke(
    node: &GlobalRef,
    from: f32,
    to: f32,
    duration_ms: u32,
    _easing: runtime_core::Easing,
    infinite: bool,
    autoreverses: bool,
) {
    with_env(|env| {
        let drawable: Result<JObject, _> = env
            .call_method(node.as_obj(), "getDrawable", "()Landroid/graphics/drawable/Drawable;", &[])
            .and_then(|v| v.l());
        let Ok(d) = drawable else { return };

        let from_level = (from.clamp(0.0, 1.0) * 10000.0) as i32;
        let to_level = (to.clamp(0.0, 1.0) * 10000.0) as i32;

        // ObjectAnimator.ofInt(target, "level", from, to)
        let animator_class = env.find_class("android/animation/ObjectAnimator").unwrap();
        let prop_name = env.new_string("level").unwrap();
        let values = env.new_int_array(2).unwrap();
        let _ = env.set_int_array_region(&values, 0, &[from_level, to_level]);

        let animator = env
            .call_static_method(
                &animator_class,
                "ofInt",
                "(Ljava/lang/Object;Ljava/lang/String;[I)Landroid/animation/ObjectAnimator;",
                &[
                    JValue::Object(&d),
                    JValue::Object(&prop_name),
                    JValue::Object(unsafe { &JObject::from_raw(values.into_raw()) }),
                ],
            )
            .and_then(|v| v.l());

        if let Ok(anim) = animator {
            let _ = env.call_method(
                &anim,
                "setDuration",
                "(J)Landroid/animation/ObjectAnimator;",
                &[JValue::Long(duration_ms as i64)],
            );
            if infinite {
                // ValueAnimator.INFINITE = -1
                let _ = env.call_method(
                    &anim, "setRepeatCount", "(I)V", &[JValue::Int(-1)],
                );
                // REVERSE = 2, RESTART = 1
                let mode = if autoreverses { 2 } else { 1 };
                let _ = env.call_method(
                    &anim, "setRepeatMode", "(I)V", &[JValue::Int(mode)],
                );
            }
            let _ = env.call_method(&anim, "start", "()V", &[]);
        }
    });
}

/// Build a ShapeDrawable-based icon from path data.
/// Uses android.graphics.drawable.ShapeDrawable with a custom PathShape.
/// Default icon edge length in dp (matches iOS `icon::DEFAULT_SIZE`).
/// The drawable's intrinsic size is this many device pixels so the
/// Taffy measure_fn sizes the icon to 24dp; the SVG paths are scaled
/// from the viewBox to the drawable's bounds at draw time.
const DEFAULT_ICON_DP: f32 = 24.0;

fn build_icon_drawable<'a>(
    env: &mut jni::JNIEnv<'a>,
    data: &IconData,
    color: Option<&Color>,
    density: f32,
) -> JObject<'a> {
    let (vw, vh) = data.view_box;

    // Create a Path combining all icon paths.
    let path_class = env.find_class("android/graphics/Path").unwrap();
    let path = env.new_object(&path_class, "()V", &[]).unwrap();

    // Parse SVG path data using android.util.PathParser (API 21+).
    for path_d in data.paths {
        let d_str = env.new_string(path_d).unwrap();
        let parser_class = env.find_class("androidx/core/graphics/PathParser").unwrap_or_else(|_| {
            // Fallback to framework PathParser (hidden API but available).
            env.find_class("android/util/PathParser").unwrap()
        });
        let parsed = env.call_static_method(
            &parser_class,
            "createPathFromPathData",
            "(Ljava/lang/String;)Landroid/graphics/Path;",
            &[JValue::Object(&d_str)],
        );
        if let Ok(p) = parsed.and_then(|v| v.l()) {
            // path.addPath(parsed)
            let _ = env.call_method(
                &path,
                "addPath",
                "(Landroid/graphics/Path;)V",
                &[JValue::Object(&p)],
            );
        }
    }

    // Create ShapeDrawable with a PathShape.
    let path_shape_class = env.find_class("android/graphics/drawable/shapes/PathShape").unwrap();
    let path_shape = env
        .new_object(
            &path_shape_class,
            "(Landroid/graphics/Path;FF)V",
            &[
                JValue::Object(&path),
                JValue::Float(vw as f32),
                JValue::Float(vh as f32),
            ],
        )
        .unwrap();

    let shape_drawable_class = env
        .find_class("android/graphics/drawable/ShapeDrawable")
        .unwrap();
    let drawable = env
        .new_object(
            &shape_drawable_class,
            "(Landroid/graphics/drawable/shapes/Shape;)V",
            &[JValue::Object(&path_shape)],
        )
        .unwrap();

    // Configure paint for stroke rendering.
    let paint: JObject = env
        .call_method(
            &drawable,
            "getPaint",
            "()Landroid/graphics/Paint;",
            &[],
        )
        .and_then(|v| v.l())
        .unwrap();

    // Paint style: FILL for filled/silhouette icons, STROKE for outlined
    // (the default). The stroke width/cap/join set below are inert when
    // the paint is in FILL mode, so we leave them unconditional.
    let style_class = env.find_class("android/graphics/Paint$Style").unwrap();
    let style_field = if data.filled { "FILL" } else { "STROKE" };
    let paint_style = env
        .get_static_field(&style_class, style_field, "Landroid/graphics/Paint$Style;")
        .and_then(|v| v.l())
        .unwrap();
    let _ = env.call_method(
        &paint,
        "setStyle",
        "(Landroid/graphics/Paint$Style;)V",
        &[JValue::Object(&paint_style)],
    );

    // Stroke width (2 units in viewBox space — will be scaled by drawable).
    let _ = env.call_method(&paint, "setStrokeWidth", "(F)V", &[JValue::Float(2.0)]);

    // Round cap = 1, round join = 1
    let cap_class = env.find_class("android/graphics/Paint$Cap").unwrap();
    let round_cap = env
        .get_static_field(&cap_class, "ROUND", "Landroid/graphics/Paint$Cap;")
        .and_then(|v| v.l())
        .unwrap();
    let _ = env.call_method(
        &paint,
        "setStrokeCap",
        "(Landroid/graphics/Paint$Cap;)V",
        &[JValue::Object(&round_cap)],
    );

    let join_class = env.find_class("android/graphics/Paint$Join").unwrap();
    let round_join = env
        .get_static_field(&join_class, "ROUND", "Landroid/graphics/Paint$Join;")
        .and_then(|v| v.l())
        .unwrap();
    let _ = env.call_method(
        &paint,
        "setStrokeJoin",
        "(Landroid/graphics/Paint$Join;)V",
        &[JValue::Object(&round_join)],
    );

    // Anti-alias.
    let _ = env.call_method(&paint, "setAntiAlias", "(Z)V", &[JValue::Bool(1)]);

    // Color: default to label color (text appearance), or explicit.
    let argb = match color {
        Some(c) => parse_color_to_argb(&c.0),
        None => 0xFF000000u32 as i32, // Black fallback; theme tinting overrides.
    };
    let _ = env.call_method(&paint, "setColor", "(I)V", &[JValue::Int(argb)]);

    // Intrinsic size = the icon's render size (24dp) in device pixels.
    // The Taffy measure_fn reads this via `View.measure`, so it has to be
    // the real dp-scaled size, NOT the viewBox units (which would make
    // the icon ~7dp). The paths are scaled from the viewBox to the
    // drawable's bounds at draw time, so the viewBox only drives the path
    // geometry, not the display size.
    let intrinsic_px = (DEFAULT_ICON_DP * density).round() as i32;
    let _ = env.call_method(
        &drawable,
        "setIntrinsicWidth",
        "(I)V",
        &[JValue::Int(intrinsic_px)],
    );
    let _ = env.call_method(
        &drawable,
        "setIntrinsicHeight",
        "(I)V",
        &[JValue::Int(intrinsic_px)],
    );

    drawable
}

fn get_density(env: &mut jni::JNIEnv, context: &GlobalRef) -> f32 {
    let resources = env
        .call_method(context.as_obj(), "getResources", "()Landroid/content/res/Resources;", &[])
        .and_then(|v| v.l());
    let metrics = resources.and_then(|res| {
        env.call_method(&res, "getDisplayMetrics", "()Landroid/util/DisplayMetrics;", &[])
            .and_then(|v| v.l())
    });
    metrics
        .and_then(|m| env.get_field(&m, "density", "F").and_then(|v| v.f()))
        .unwrap_or(2.0)
}

/// Parse a CSS color string to Android's `0xAARRGGBB` packed int.
/// Routes through the canonical parser; falls back to opaque black
/// on unknown input. Previously this had a divergent 8-digit hex
/// interpretation (read `#rrggbbaa` as `#aarrggbb`) that produced
/// the wrong color for any icon styled with CSS-spec 8-digit hex.
fn parse_color_to_argb(color: &str) -> i32 {
    runtime_core::color::parse_or(color, runtime_core::color::Rgba::BLACK).to_argb_u32() as i32
}
