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
/// these defaults via the LayoutParams field mutation in the
/// leaf crate's `imp::style::apply_rules`.
pub fn apply_default_layout_params(env: &mut JNIEnv, view: &JObject) {
    // `MarginLayoutParams`, not the bare `ViewGroup.LayoutParams`,
    // because `ScrollView.measureChildWithMargins` (and every other
    // margin-aware parent) casts its child's LP to MarginLayoutParams
    // and throws `ClassCastException` otherwise. Margin-aware is a
    // strict superset of the bare LP shape, so this is safe to use
    // as the default for every view.
    let lp_class = env
        .find_class("android/view/ViewGroup$MarginLayoutParams")
        .unwrap();
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

pub fn set_text(env: &mut JNIEnv, view: &JObject, content: &str) {
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
pub fn parse_color(input: &str) -> Option<i32> {
    let trimmed = input.trim();
    if trimmed.eq_ignore_ascii_case("transparent") {
        return Some(0);
    }
    // CSS `rgba(r, g, b, a)` / `rgb(r, g, b)`. Channels are 0..=255
    // integers; alpha is 0..=1 float (or, leniently, 0..=255 — we
    // pick whichever interpretation makes sense from the value). Used
    // by the welcome example's gradient stops; same shape as the iOS
    // parser so authors can write one color string and have it work
    // on all backends.
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("rgba(") || lower.starts_with("rgb(") {
        let inner = trimmed
            .trim_start_matches(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
            .trim_end_matches(')');
        let parts: Vec<&str> = inner.split(',').collect();
        if parts.len() < 3 {
            return None;
        }
        let r: f32 = parts[0].trim().parse().ok()?;
        let g: f32 = parts[1].trim().parse().ok()?;
        let b: f32 = parts[2].trim().parse().ok()?;
        let a: f32 = if parts.len() >= 4 {
            parts[3].trim().parse().ok()?
        } else {
            1.0
        };
        // Alpha: if >1.0 we treat it as a 0..=255 byte (lenient
        // handling of the rare `rgba(r, g, b, 255)` form); otherwise
        // map 0..=1 to 0..=255.
        let a_byte = if a > 1.0 {
            a.clamp(0.0, 255.0).round() as u32
        } else {
            (a.clamp(0.0, 1.0) * 255.0).round() as u32
        };
        let r_byte = r.clamp(0.0, 255.0).round() as u32;
        let g_byte = g.clamp(0.0, 255.0).round() as u32;
        let b_byte = b.clamp(0.0, 255.0).round() as u32;
        return Some(
            (((a_byte & 0xff) << 24)
                | ((r_byte & 0xff) << 16)
                | ((g_byte & 0xff) << 8)
                | (b_byte & 0xff)) as i32,
        );
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
/// all `Option<Tokenized<Length>>` after the tokenization refactor;
/// native has no token system to point to so we resolve every token
/// to its current value (`.resolve()`) at apply-time. The `.resolve()`
/// call subscribes the enclosing apply-style Effect to the token's
/// signal, so token swaps re-fire the apply.
pub fn px_or(value: Option<&framework_core::Tokenized<framework_core::Length>>, default: f32) -> f32 {
    match value.map(|t| t.resolve()) {
        Some(framework_core::Length::Px(v)) => v,
        // Percent/Auto don't have a well-defined value here without a
        // layout pass; treat as default.
        _ => default,
    }
}

pub fn dp_to_px(env: &mut JNIEnv, view: &JObject, dp: f32) -> i32 {
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
