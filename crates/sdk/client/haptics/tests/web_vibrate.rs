//! Browser tests for the web haptics backend (`navigator.vibrate`).
//!
//! Run with `cargo test -p haptics --target wasm32-unknown-unknown` (the
//! workspace runner supplies web-glue's JS; `wasm-pack test` cannot).

#![cfg(target_arch = "wasm32")]

use haptics::{impact, is_supported, notify, selection, ImpactStyle, NotificationFeedback};
use wasm_bindgen_test::*;
use web_glue::JsValue;

wasm_bindgen_test_configure!(run_in_browser);

/// Replace `navigator.vibrate` with `value` (an own property shadowing the
/// prototype's), returning a guard that removes it again.
struct VibrateOverride;

impl VibrateOverride {
    fn install(value: &JsValue) -> VibrateOverride {
        let desc = JsValue::global().get("Object").unwrap().construct(&[]).unwrap();
        desc.set("value", value).unwrap();
        desc.set("configurable", &JsValue::from_bool(true)).unwrap();
        desc.set("writable", &JsValue::from_bool(true)).unwrap();
        let navigator = JsValue::global().get("navigator").unwrap();
        JsValue::global()
            .get("Object")
            .unwrap()
            .call_method("defineProperty", &[&navigator, &JsValue::from_str("vibrate"), &desc])
            .unwrap();
        VibrateOverride
    }
}

impl Drop for VibrateOverride {
    fn drop(&mut self) {
        let navigator = JsValue::global().get("navigator").unwrap();
        let _ = JsValue::global()
            .get("Reflect")
            .unwrap()
            .call_method("deleteProperty", &[&navigator, &JsValue::from_str("vibrate")]);
    }
}

/// A JS function that appends each call's argument, as JSON, to the
/// `|`-separated string `globalThis.__vibrateCalls`.
fn recording_vibrate() -> JsValue {
    JsValue::global()
        .get("Function")
        .unwrap()
        .construct(&[
            &JsValue::from_str("p"),
            &JsValue::from_str(
                "const c = globalThis.__vibrateCalls; \
                 globalThis.__vibrateCalls = (c ? c + '|' : '') + JSON.stringify(p); return true;",
            ),
        ])
        .unwrap()
}

fn take_calls() -> Vec<String> {
    let calls = JsValue::global().get("__vibrateCalls").unwrap().as_string().unwrap_or_default();
    JsValue::global().set("__vibrateCalls", &JsValue::undefined()).unwrap();
    calls.split('|').filter(|s| !s.is_empty()).map(str::to_owned).collect()
}

/// Regression: Safari and desktop Firefox have NO `navigator.vibrate`.
/// The web-sys port called it unconditionally, so the missing method threw
/// a TypeError through the wasm frames instead of being the documented
/// no-op. Every effect must now return quietly.
#[wasm_bindgen_test]
fn regression_missing_vibration_api_is_a_quiet_no_op() {
    let _no_vibrate = VibrateOverride::install(&JsValue::undefined());
    impact(ImpactStyle::Heavy);
    notify(NotificationFeedback::Error);
    selection();
    // A window context still reports supported (the predicate is coarse by
    // design — see `is_supported`'s docs).
    assert!(is_supported());
}

/// Single pulses use the duration overload; notifications pass the whole
/// on/off pattern as an array.
#[wasm_bindgen_test]
fn effects_reach_navigator_vibrate_with_their_patterns() {
    let _rec = VibrateOverride::install(&recording_vibrate());
    impact(ImpactStyle::Medium);
    selection();
    notify(NotificationFeedback::Warning);
    notify(NotificationFeedback::Success);
    assert_eq!(take_calls(), vec!["20", "5", "[20,60,20]", "[15]"]);
}
