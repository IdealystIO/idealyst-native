//! Small JNI utilities used across primitives and the style path.
//! Free functions (no inherent impl) — each takes the `JNIEnv` and
//! whatever JVM object handle it operates on.
//!
//! Panic policy: every public helper here graceful-degrades on JNI
//! failure (logs via `log::error!`, returns / no-ops). These functions
//! are reachable from `extern "system"` JNI exports in
//! `backend-android-mobile`; unwinding across that boundary is
//! undefined behavior. Pre-fix the helpers used `.unwrap()` liberally,
//! which would have aborted the JVM process on any pending JNI
//! exception or class-load failure.

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
///
/// JNI failures (class not found, pending exception) are logged and
/// the view is left with its constructor-default LayoutParams. The
/// affected view will still mount; it may render at the wrong size
/// until the layout pass corrects it.
///
/// Caller must invoke this on the Android main/UI thread — it mutates a
/// `View`. Off-main mutation raises `CalledFromWrongThreadException`.
pub fn apply_default_layout_params(env: &mut JNIEnv, view: &JObject) {
    // `MarginLayoutParams`, not the bare `ViewGroup.LayoutParams`,
    // because `ScrollView.measureChildWithMargins` (and every other
    // margin-aware parent) casts its child's LP to MarginLayoutParams
    // and throws `ClassCastException` otherwise. Margin-aware is a
    // strict superset of the bare LP shape, so this is safe to use
    // as the default for every view.
    let lp_class = match env.find_class("android/view/ViewGroup$MarginLayoutParams") {
        Ok(c) => c,
        Err(e) => {
            let _ = env.exception_clear();
            log::error!(
                "[backend-android-core] apply_default_layout_params: \
                 find_class(MarginLayoutParams) failed: {e}; leaving view with default LP"
            );
            return;
        }
    };
    // -1 = MATCH_PARENT, -2 = WRAP_CONTENT.
    let Ok(lp) = env.new_object(
        &lp_class,
        "(II)V",
        &[JValue::Int(-1), JValue::Int(-2)],
    ) else {
        let _ = env.exception_clear();
        return;
    };
    let _ = env.call_method(
        view,
        "setLayoutParams",
        "(Landroid/view/ViewGroup$LayoutParams;)V",
        &[JValue::Object(&lp)],
    );
    // call_method swallows the Result; clear any pending exception
    // so the next JNI call from a caller doesn't trip on stale state.
    let _ = env.exception_clear();
}

/// Set a `TextView`-shaped widget's text. JNI failures (pending
/// exception, OOM allocating the Java String, Java-side method throw)
/// are logged and the call is a no-op — the text widget stays at its
/// previous content. Never panics across the JNI boundary.
///
/// Caller must invoke this on the Android main/UI thread — it mutates a
/// `View`.
pub fn set_text(env: &mut JNIEnv, view: &JObject, content: &str) {
    let java_str = match env.new_string(content) {
        Ok(s) => s,
        Err(e) => {
            let _ = env.exception_clear();
            log::error!(
                "[backend-android-core] set_text: new_string failed: {e}; skipping setText"
            );
            return;
        }
    };
    if let Err(e) = env.call_method(
        view,
        "setText",
        "(Ljava/lang/CharSequence;)V",
        &[JValue::Object(&JObject::from(java_str))],
    ) {
        let _ = env.exception_clear();
        log::error!("[backend-android-core] set_text: setText threw: {e}");
    }
}

/// Parse a CSS-style color string into the Android `int` form
/// (`0xAARRGGBB`). Parsing logic lives in `runtime_core::color`;
/// `Rgba::to_argb_u32()` handles the CSS→Android byte-order
/// reshuffle so 8-digit `#rrggbbaa` reads as the CSS-spec alpha-
/// last form (not the legacy `#aarrggbb` interpretation that
/// produced dark squares on fade-out stops).
pub fn parse_color(input: &str) -> Option<i32> {
    runtime_core::color::parse(input).ok().map(|c| c.to_argb_u32() as i32)
}

/// Pull the first `Length::Px` value from a per-side group, falling
/// back to `default` when absent. The framework's per-side fields are
/// all `Option<Tokenized<Length>>` after the tokenization refactor;
/// native has no token system to point to so we resolve every token
/// to its current value (`.resolve()`) at apply-time. The `.resolve()`
/// call subscribes the enclosing apply-style Effect to the token's
/// signal, so token swaps re-fire the apply.
pub fn px_or(value: Option<&runtime_core::Tokenized<runtime_core::Length>>, default: f32) -> f32 {
    match value.map(|t| t.resolve()) {
        Some(runtime_core::Length::Px(v)) => v,
        // Percent/Auto don't have a well-defined value here without a
        // layout pass; treat as default.
        _ => default,
    }
}

/// Convert a `dp` value to device pixels by reading the view's
/// display-metric density. JNI failures (detached view, missing
/// Resources reference, mid-frame race) are logged and the function
/// falls back to `dp.round()` — the unconverted value. The affected
/// layout will be slightly off (1 dp = 1 px) until the next apply
/// pass, but the app keeps running. Never panics across the JNI
/// boundary.
pub fn dp_to_px(env: &mut JNIEnv, view: &JObject, dp: f32) -> i32 {
    // density = view.getResources().getDisplayMetrics().density
    let fallback = dp.round() as i32;
    macro_rules! fail {
        ($what:expr, $err:expr) => {{
            let _ = env.exception_clear();
            log::error!(
                "[backend-android-core] dp_to_px: {} failed: {}; \
                 falling back to dp.round() = {}",
                $what,
                $err,
                fallback,
            );
            return fallback;
        }};
    }

    let res_jval = match env.call_method(
        view,
        "getResources",
        "()Landroid/content/res/Resources;",
        &[],
    ) {
        Ok(v) => v,
        Err(e) => fail!("getResources()", e),
    };
    let res = match res_jval.l() {
        Ok(o) => o,
        Err(e) => fail!("getResources().l()", e),
    };
    let metrics_jval = match env.call_method(
        &res,
        "getDisplayMetrics",
        "()Landroid/util/DisplayMetrics;",
        &[],
    ) {
        Ok(v) => v,
        Err(e) => fail!("getDisplayMetrics()", e),
    };
    let metrics = match metrics_jval.l() {
        Ok(o) => o,
        Err(e) => fail!("getDisplayMetrics().l()", e),
    };
    let density_jval = match env.get_field(&metrics, "density", "F") {
        Ok(v) => v,
        Err(e) => fail!("get_field(density)", e),
    };
    let density = match density_jval.f() {
        Ok(d) => d,
        Err(e) => fail!("density.f()", e),
    };
    (dp * density).round() as i32
}
