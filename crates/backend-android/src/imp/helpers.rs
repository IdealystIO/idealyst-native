//! Small JNI utilities used across primitives and the style path.
//! Free functions (no inherent impl) — each takes the `JNIEnv` and
//! whatever JVM object handle it operates on.

use jni::objects::{JObject, JValue};
use jni::JNIEnv;

/// Stamp a default `LayoutParams(MATCH_PARENT, WRAP_CONTENT)` on a
/// freshly-created View. Mirrors the web backend's default of
/// `display: flex; flex-direction: column` with children stretched on
/// the cross-axis — without this, Android's parent LayoutParams
/// default to `WRAP_CONTENT` and rows shrink to their content width.
///
/// Explicit `width` / `height` from the stylesheet later overrides
/// these defaults via the LayoutParams field mutation in
/// [`super::style::apply_rules`].
pub(crate) fn apply_default_layout_params(env: &mut JNIEnv, view: &JObject) {
    let lp_class = env.find_class("android/view/ViewGroup$LayoutParams").unwrap();
    // -1 = MATCH_PARENT, -2 = WRAP_CONTENT.
    let Ok(lp) = env.new_object(
        &lp_class,
        "(II)V",
        &[JValue::Int(-1), JValue::Int(-2)],
    ) else {
        return;
    };
    let _ = env.call_method(
        view,
        "setLayoutParams",
        "(Landroid/view/ViewGroup$LayoutParams;)V",
        &[JValue::Object(&lp)],
    );
}

pub(crate) fn set_text(env: &mut JNIEnv, view: &JObject, content: &str) {
    let java_str = env.new_string(content).unwrap();
    env.call_method(
        view,
        "setText",
        "(Ljava/lang/CharSequence;)V",
        &[JValue::Object(&JObject::from(java_str))],
    )
    .unwrap();
}

/// Parse a CSS-style color string (`#rgb`, `#rrggbb`, `#aarrggbb`, or
/// `transparent`) into the Android `int` form: `0xAARRGGBB`.
pub(crate) fn parse_color(input: &str) -> Option<i32> {
    let trimmed = input.trim();
    if trimmed.eq_ignore_ascii_case("transparent") {
        return Some(0);
    }
    if !trimmed.starts_with('#') {
        return None;
    }
    let hex = &trimmed[1..];
    // Helper: parse hex with full alpha if not provided.
    let parse = |s: &str, alpha: u32| -> Option<i32> {
        let rgb = u32::from_str_radix(s, 16).ok()?;
        Some(((alpha << 24) | rgb) as i32)
    };
    match hex.len() {
        3 => {
            let r = u32::from_str_radix(&hex[0..1], 16).ok()?;
            let g = u32::from_str_radix(&hex[1..2], 16).ok()?;
            let b = u32::from_str_radix(&hex[2..3], 16).ok()?;
            let expand = |v: u32| (v << 4) | v;
            let packed = (expand(r) << 16) | (expand(g) << 8) | expand(b);
            Some((0xFF000000u32 | packed) as i32)
        }
        6 => parse(hex, 0xFF),
        8 => {
            // Android wants AARRGGBB; assume CSS-like input is already
            // in the same order for the 8-digit form.
            Some(u32::from_str_radix(hex, 16).ok()? as i32)
        }
        _ => None,
    }
}

/// Pull the first `Length::Px` value from a per-side group, falling
/// back to `default` when absent. The framework's per-side fields are
/// all `Option<Length>`; for padding we collapse them with a
/// saturating max so a single-side override doesn't zero the other
/// sides.
pub(crate) fn px_or(value: Option<framework_core::Length>, default: f32) -> f32 {
    match value {
        Some(framework_core::Length::Px(v)) => v,
        // Percent/Auto don't have a well-defined value here without a
        // layout pass; treat as default.
        _ => default,
    }
}

pub(crate) fn dp_to_px(env: &mut JNIEnv, view: &JObject, dp: f32) -> i32 {
    // density = view.getResources().getDisplayMetrics().density
    let res = env
        .call_method(view, "getResources", "()Landroid/content/res/Resources;", &[])
        .unwrap()
        .l()
        .unwrap();
    let metrics = env
        .call_method(
            &res,
            "getDisplayMetrics",
            "()Landroid/util/DisplayMetrics;",
            &[],
        )
        .unwrap()
        .l()
        .unwrap();
    let density = env.get_field(&metrics, "density", "F").unwrap().f().unwrap();
    (dp * density).round() as i32
}
