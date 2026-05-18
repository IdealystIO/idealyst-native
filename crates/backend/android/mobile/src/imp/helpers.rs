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
/// all `Option<Tokenized<Length>>` after the tokenization refactor;
/// native has no token system to point to so we resolve every token
/// to its fallback (`.value()`) at apply-time.
pub(crate) fn px_or(value: Option<&framework_core::Tokenized<framework_core::Length>>, default: f32) -> f32 {
    match value.map(|t| t.value()) {
        Some(framework_core::Length::Px(v)) => *v,
        // Percent/Auto don't have a well-defined value here without a
        // layout pass; treat as default.
        _ => default,
    }
}

/// Read a View's screen-relative bounding rect, in physical pixels.
/// Origin is the top-left of the device screen, including the
/// status bar (the same coordinate space `PopupWindow.showAtLocation`
/// uses).
///
/// Returns the zero rect if the view has no width/height yet (not
/// laid out) — which gives overlay positioning code a sensible
/// fallback (it'll center on the viewport instead of anchoring to
/// nowhere).
///
/// Synchronous JNI calls. Cheap enough to call once per overlay
/// open; not suitable for per-frame use. (`getLocationOnScreen`
/// internally walks the view ancestry.)
pub(crate) fn view_screen_rect(
    node: &jni::objects::GlobalRef,
) -> framework_core::primitives::overlay::ViewportRect {
    crate::imp::with_env(|env| {
        // int[2] for getLocationOnScreen's output param.
        let Ok(loc) = env.new_int_array(2) else {
            return framework_core::primitives::overlay::ViewportRect::default();
        };
        // `JIntArray` derefs to `JObject` via `AsRef`, but the jni
        // 0.21 API wants `&JObject` in `JValue::Object`. Pull the
        // underlying object reference out explicitly.
        let loc_obj: &JObject = loc.as_ref();
        if env
            .call_method(
                node.as_obj(),
                "getLocationOnScreen",
                "([I)V",
                &[JValue::Object(loc_obj)],
            )
            .is_err()
        {
            return framework_core::primitives::overlay::ViewportRect::default();
        }
        let mut buf = [0i32; 2];
        if env.get_int_array_region(&loc, 0, &mut buf).is_err() {
            return framework_core::primitives::overlay::ViewportRect::default();
        }
        let width = env
            .call_method(node.as_obj(), "getWidth", "()I", &[])
            .and_then(|v| v.i())
            .unwrap_or(0);
        let height = env
            .call_method(node.as_obj(), "getHeight", "()I", &[])
            .and_then(|v| v.i())
            .unwrap_or(0);
        framework_core::primitives::overlay::ViewportRect {
            x: buf[0] as f32,
            y: buf[1] as f32,
            width: width as f32,
            height: height as f32,
        }
    })
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
