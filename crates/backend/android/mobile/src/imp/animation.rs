//! Animator builders. Each returns the animator as a `GlobalRef` so
//! the per-node cache in [`super::NodeAnim`] can hold it across
//! `apply_style` calls and cancel it on value change.
//!
//! The simpler animators use `ObjectAnimator` directly (it finds the
//! setter by property name via reflection). For properties that don't
//! map cleanly to a single setter — per-side padding, combined stroke
//! width+color, four-corner radii — we route through a tiny Kotlin
//! helper class `io.idealyst.runtime.Animators` that owns a
//! `ValueAnimator` and re-invokes the multi-arg setter on each tick.

use runtime_core::{Easing, Transition};
use jni::objects::{GlobalRef, JObject, JValue};
use jni::JNIEnv;

/// Cancel a previously-running animator, dropping the JVM global.
pub(crate) fn cancel_animator(env: &mut JNIEnv, anim: Option<GlobalRef>) {
    if let Some(a) = anim {
        let _ = env.call_method(a.as_obj(), "cancel", "()V", &[]);
    }
}

/// `ObjectAnimator.ofArgb(target, propertyName, from, to)` — animates
/// an `int`-valued ARGB property via the JVM's built-in
/// `ArgbEvaluator`. Used for `View.backgroundColor` and
/// `TextView.textColor`.
pub(crate) fn start_argb_animator(
    env: &mut JNIEnv,
    target: &GlobalRef,
    property: &str,
    from: i32,
    to: i32,
    transition: Transition,
) -> Option<GlobalRef> {
    let class = env.find_class("android/animation/ObjectAnimator").ok()?;
    let prop = env.new_string(property).ok()?;
    let (from, to) = normalize_argb_fade(from, to);
    let values = env.new_int_array(2).ok()?;
    env.set_int_array_region(&values, 0, &[from, to]).ok()?;
    let anim = env
        .call_static_method(
            &class,
            "ofArgb",
            "(Ljava/lang/Object;Ljava/lang/String;[I)Landroid/animation/ObjectAnimator;",
            &[
                JValue::Object(&target.as_obj()),
                JValue::Object(&JObject::from(prop)),
                JValue::Object(&JObject::from(values)),
            ],
        )
        .ok()?
        .l()
        .ok()?;
    configure_and_start(env, &anim, transition)?;
    env.new_global_ref(&anim).ok()
}

/// Normalize a fully-transparent endpoint to carry its PARTNER's RGB so the
/// JVM `ArgbEvaluator` (straight, non-premultiplied ARGB interpolation) fades
/// only the alpha rather than through gray. `transparent` packs to `0x00000000`
/// (RGB = black), so interpolating it to an opaque light color passes through
/// semi-opaque BLACK at the midpoint — the dark-gray flash on press / active /
/// focus state fades, where the resting `background` is `transparent`. Swapping
/// the partner RGB into the transparent endpoint is invisible (alpha stays 0)
/// and keeps the fade on the target hue — the same outcome as the macOS
/// premultiplied-alpha tween (`transitions::lerp`).
fn normalize_argb_fade(from: i32, to: i32) -> (i32, i32) {
    // Arithmetic `>> 24` sign-extends for an i32 with the alpha high bit set;
    // `& 0xFF` masks that off to recover the alpha byte. `& 0x00FF_FFFF` keeps
    // RGB with alpha 0.
    let from_opaque = (from >> 24) & 0xFF != 0;
    let to_opaque = (to >> 24) & 0xFF != 0;
    let from2 = if from_opaque { from } else { to & 0x00FF_FFFF };
    let to2 = if to_opaque { to } else { from2 & 0x00FF_FFFF };
    (from2, to2)
}

/// Specialized ARGB animator for `GradientDrawable.color`. The
/// JVM-side `setColor(int)` is the matching mutator.
pub(crate) fn start_drawable_argb_animator(
    env: &mut JNIEnv,
    drawable: &GlobalRef,
    property: &str,
    from: i32,
    to: i32,
    transition: Transition,
) -> Option<GlobalRef> {
    // Same machinery as the View case; GradientDrawable exposes a
    // setColor(int) so ObjectAnimator finds it by name.
    start_argb_animator(env, drawable, property, from, to, transition)
}

/// `ObjectAnimator.ofFloat(target, propertyName, from, to)` for
/// scalar properties (alpha, scale, etc.).
pub(crate) fn start_float_animator(
    env: &mut JNIEnv,
    target: &GlobalRef,
    property: &str,
    from: f32,
    to: f32,
    transition: Transition,
) -> Option<GlobalRef> {
    let class = env.find_class("android/animation/ObjectAnimator").ok()?;
    let prop = env.new_string(property).ok()?;
    let values = env.new_float_array(2).ok()?;
    env.set_float_array_region(&values, 0, &[from, to]).ok()?;
    let anim = env
        .call_static_method(
            &class,
            "ofFloat",
            "(Ljava/lang/Object;Ljava/lang/String;[F)Landroid/animation/ObjectAnimator;",
            &[
                JValue::Object(&target.as_obj()),
                JValue::Object(&JObject::from(prop)),
                JValue::Object(&JObject::from(values)),
            ],
        )
        .ok()?
        .l()
        .ok()?;
    configure_and_start(env, &anim, transition)?;
    env.new_global_ref(&anim).ok()
}

/// Per-side padding animator. There's no `paddingLeft` etc. setter
/// that ObjectAnimator can find by reflection, so we go through a
/// Kotlin-side bridge that owns a `ValueAnimator` + listener and
/// invokes `setPadding(...)` with the interpolated value, preserving
/// the other three sides.
pub(crate) fn start_padding_animator(
    env: &mut JNIEnv,
    view: &GlobalRef,
    side: i32, // 0..3 = L,T,R,B
    from: i32,
    to: i32,
    transition: Transition,
) -> Option<GlobalRef> {
    let class = env.find_class("io/idealyst/runtime/Animators").ok()?;
    let interpolator = build_interpolator(env, transition.easing)?;
    let anim = env
        .call_static_method(
            &class,
            "animatePaddingSide",
            "(Landroid/view/View;IIIJLandroid/view/animation/Interpolator;)Landroid/animation/ValueAnimator;",
            &[
                JValue::Object(&view.as_obj()),
                JValue::Int(side),
                JValue::Int(from),
                JValue::Int(to),
                JValue::Long(transition.duration_ms as i64),
                JValue::Object(&interpolator),
            ],
        )
        .ok()?
        .l()
        .ok()?;
    env.new_global_ref(&anim).ok()
}

/// Stroke animator: `GradientDrawable.setStroke` takes
/// Per-side border animator. Interpolates four widths + four colors
/// in lockstep and re-invokes `RustBorderDrawable.update(...)` on
/// each tick. Width arrays + color arrays are in
/// (top, right, bottom, left) order — same convention the per-side
/// drawable uses. All eight axes interpolate at the same fraction;
/// the framework's per-side `border_*_transition` props get
/// collapsed to a single timing at the caller (matches the iOS path,
/// which doesn't animate borders at all).
pub(crate) fn start_border_animator(
    env: &mut JNIEnv,
    drawable: &GlobalRef,
    from_w: [i32; 4],
    to_w: [i32; 4],
    from_c: [i32; 4],
    to_c: [i32; 4],
    transition: Transition,
) -> Option<GlobalRef> {
    let class = env.find_class("io/idealyst/runtime/Animators").ok()?;
    let interpolator = build_interpolator(env, transition.easing)?;
    let from_w_arr = env.new_int_array(4).ok()?;
    env.set_int_array_region(&from_w_arr, 0, &from_w).ok()?;
    let to_w_arr = env.new_int_array(4).ok()?;
    env.set_int_array_region(&to_w_arr, 0, &to_w).ok()?;
    let from_c_arr = env.new_int_array(4).ok()?;
    env.set_int_array_region(&from_c_arr, 0, &from_c).ok()?;
    let to_c_arr = env.new_int_array(4).ok()?;
    env.set_int_array_region(&to_c_arr, 0, &to_c).ok()?;
    let anim = env
        .call_static_method(
            &class,
            "animateBorder",
            "(Lio/idealyst/runtime/RustBorderDrawable;[I[I[I[IJLandroid/view/animation/Interpolator;)Landroid/animation/ValueAnimator;",
            &[
                JValue::Object(&drawable.as_obj()),
                JValue::Object(&from_w_arr),
                JValue::Object(&to_w_arr),
                JValue::Object(&from_c_arr),
                JValue::Object(&to_c_arr),
                JValue::Long(transition.duration_ms as i64),
                JValue::Object(&interpolator),
            ],
        )
        .ok()?
        .l()
        .ok()?;
    env.new_global_ref(&anim).ok()
}

/// `(width, color)` together so we route through a Kotlin helper
/// that owns a ValueAnimator and re-invokes `setStroke` on each
/// tick using a separate `ArgbEvaluator` for the color and a linear
/// int interpolation for the width.
pub(crate) fn start_stroke_animator(
    env: &mut JNIEnv,
    drawable: &GlobalRef,
    from_w: i32,
    to_w: i32,
    from_c: i32,
    to_c: i32,
    transition: Transition,
) -> Option<GlobalRef> {
    let class = env.find_class("io/idealyst/runtime/Animators").ok()?;
    let interpolator = build_interpolator(env, transition.easing)?;
    let anim = env
        .call_static_method(
            &class,
            "animateStroke",
            "(Landroid/graphics/drawable/GradientDrawable;IIIIJLandroid/view/animation/Interpolator;)Landroid/animation/ValueAnimator;",
            &[
                JValue::Object(&drawable.as_obj()),
                JValue::Int(from_w),
                JValue::Int(to_w),
                JValue::Int(from_c),
                JValue::Int(to_c),
                JValue::Long(transition.duration_ms as i64),
                JValue::Object(&interpolator),
            ],
        )
        .ok()?
        .l()
        .ok()?;
    env.new_global_ref(&anim).ok()
}

/// Corner-radii animator. Interpolates all four corners independently
/// and re-invokes `setCornerRadii` on each tick.
pub(crate) fn start_radii_animator(
    env: &mut JNIEnv,
    drawable: &GlobalRef,
    from: [f32; 4],
    to: [f32; 4],
    transition: Transition,
) -> Option<GlobalRef> {
    let class = env.find_class("io/idealyst/runtime/Animators").ok()?;
    let interpolator = build_interpolator(env, transition.easing)?;
    let from_arr = env.new_float_array(4).ok()?;
    env.set_float_array_region(&from_arr, 0, &from).ok()?;
    let to_arr = env.new_float_array(4).ok()?;
    env.set_float_array_region(&to_arr, 0, &to).ok()?;
    let anim = env
        .call_static_method(
            &class,
            "animateCornerRadii",
            "(Landroid/graphics/drawable/GradientDrawable;[F[FJLandroid/view/animation/Interpolator;)Landroid/animation/ValueAnimator;",
            &[
                JValue::Object(&drawable.as_obj()),
                JValue::Object(&JObject::from(from_arr)),
                JValue::Object(&JObject::from(to_arr)),
                JValue::Long(transition.duration_ms as i64),
                JValue::Object(&interpolator),
            ],
        )
        .ok()?
        .l()
        .ok()?;
    env.new_global_ref(&anim).ok()
}

/// Common configuration shared by all `ObjectAnimator` constructions:
/// duration, interpolator, start. Returns Some(()) on success.
fn configure_and_start(
    env: &mut JNIEnv,
    anim: &JObject,
    transition: Transition,
) -> Option<()> {
    let interp = build_interpolator(env, transition.easing)?;
    let _ = env.call_method(
        anim,
        "setDuration",
        "(J)Landroid/animation/ValueAnimator;",
        &[JValue::Long(transition.duration_ms as i64)],
    );
    let _ = env.call_method(
        anim,
        "setInterpolator",
        "(Landroid/animation/TimeInterpolator;)V",
        &[JValue::Object(&interp)],
    );
    let _ = env.call_method(anim, "start", "()V", &[]);
    Some(())
}

/// Map a framework `Easing` to a JVM `Interpolator` instance.
/// `Ease` and `EaseInOut` are intentionally distinct: `Ease` gets
/// the CSS-default cubic-bezier(0.25, 0.1, 0.25, 1.0) via
/// `PathInterpolator`, while `EaseInOut` uses the symmetric
/// `AccelerateDecelerateInterpolator` (which is closer to CSS
/// `ease-in-out` than to `ease`).
pub(crate) fn build_interpolator<'local>(
    env: &mut JNIEnv<'local>,
    easing: Easing,
) -> Option<JObject<'local>> {
    // Helper: instantiate `class` with its `()V` constructor, returning
    // the JNI *local* ref.
    fn new_instance<'a>(env: &mut JNIEnv<'a>, class_name: &str) -> Option<JObject<'a>> {
        let class = env.find_class(class_name).ok()?;
        env.new_object(&class, "()V", &[]).ok()
    }
    let interp_local: JObject<'local> = match easing {
        Easing::Linear => new_instance(env, "android/view/animation/LinearInterpolator")?,
        Easing::EaseIn => new_instance(env, "android/view/animation/AccelerateInterpolator")?,
        Easing::EaseOut => new_instance(env, "android/view/animation/DecelerateInterpolator")?,
        Easing::EaseInOut => {
            new_instance(env, "android/view/animation/AccelerateDecelerateInterpolator")?
        }
        Easing::Ease => build_cubic_bezier(env, 0.25, 0.1, 0.25, 1.0)?,
        Easing::CubicBezier(a, b, c, d) => build_cubic_bezier(env, a, b, c, d)?,
    };
    // Return the *local* ref. Every caller (`start_*_animator`) consumes
    // the interpolator immediately within the same JNI frame — it is
    // passed straight into the `Animators.*` static call before the frame
    // pops — so a local is sufficient and the JVM reclaims it with the
    // frame. The previous code promoted to a GlobalRef and `mem::forget`'d
    // it, leaking one global ref per animation start; the JVM global-ref
    // table caps near 51k and would eventually abort an animation-heavy,
    // long-lived screen (and it violated the no-`forget` rule).
    Some(interp_local)
}

/// `PathInterpolator`-via-reflection for cubic-bezier. Available on
/// API 21+ (we assume modern Android — the build targets it).
fn build_cubic_bezier<'a>(
    env: &mut JNIEnv<'a>,
    a: f32,
    b: f32,
    c: f32,
    d: f32,
) -> Option<JObject<'a>> {
    let class = env.find_class("android/view/animation/PathInterpolator").ok()?;
    env.new_object(
        &class,
        "(FFFF)V",
        &[
            JValue::Float(a),
            JValue::Float(b),
            JValue::Float(c),
            JValue::Float(d),
        ],
    )
    .ok()
}

#[cfg(test)]
mod argb_fade_tests {
    use super::normalize_argb_fade;

    // 0x00000000 = transparent (RGB black). 0xFFEEF0F7 = opaque #eef0f7.
    const TRANSPARENT: i32 = 0x0000_0000u32 as i32;
    const SURFACE: i32 = 0xFFEE_F0F7u32 as i32;

    #[test]
    fn transparent_from_takes_partner_rgb_alpha_zero() {
        // Regression: a transparent→opaque fade must NOT animate through gray.
        // The transparent `from` gets the target RGB at alpha 0, so ArgbEvaluator
        // ramps only alpha and the midpoint is the light hue, not dark gray.
        let (from, to) = normalize_argb_fade(TRANSPARENT, SURFACE);
        assert_eq!(from, 0x00EE_F0F7u32 as i32, "from = target RGB, alpha 0");
        assert_eq!(to, SURFACE, "to unchanged");
    }

    #[test]
    fn transparent_to_takes_partner_rgb_on_fade_out() {
        // Fade OUT (opaque→transparent): the transparent `to` gets the source RGB.
        let (from, to) = normalize_argb_fade(SURFACE, TRANSPARENT);
        assert_eq!(from, SURFACE, "from unchanged");
        assert_eq!(to, 0x00EE_F0F7u32 as i32, "to = source RGB, alpha 0");
    }

    #[test]
    fn opaque_to_opaque_unchanged() {
        let a = 0xFF11_2233u32 as i32;
        let b = 0xFF99_8877u32 as i32;
        assert_eq!(normalize_argb_fade(a, b), (a, b));
    }
}
