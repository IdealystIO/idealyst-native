//! Browser tests for the web location backend (`navigator.geolocation`).
//!
//! A headless page has no location fix and no way to answer the prompt, so
//! `navigator.geolocation` and `navigator.permissions` are shadowed per
//! test with JS stand-ins that call back like the real API does; what's
//! under test is the SDK's binding — the callbacks, the position decode,
//! the error mapping and the watch lifecycle.
//!
//! Run with `cargo test -p location --target wasm32-unknown-unknown` (the
//! workspace runner supplies web-glue's JS; `wasm-pack test` cannot).

#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::rc::Rc;

use location::{current, watch, LocationError, Position};
use wasm_bindgen_test::*;
use web_glue::{JsFuture, JsValue};

wasm_bindgen_test_configure!(run_in_browser);

fn eval(body: &str) -> JsValue {
    let f = JsValue::global()
        .get("Function")
        .unwrap()
        .construct(&[&JsValue::from_str(body)])
        .unwrap();
    f.call(&JsValue::undefined(), &[]).unwrap()
}

/// Shadow `navigator.<prop>` with a JS expression until the guard drops.
struct NavOverride(&'static str);

impl NavOverride {
    fn install(prop: &'static str, js_expr: &str) -> NavOverride {
        eval(&format!(
            "Object.defineProperty(navigator, '{prop}', {{ value: {js_expr}, configurable: true }});"
        ));
        NavOverride(prop)
    }
}

impl Drop for NavOverride {
    fn drop(&mut self) {
        eval(&format!("delete navigator.{};", self.0));
    }
}

/// `current()` asks the permissions SDK first; grant geolocation.
fn grant_geolocation() -> NavOverride {
    NavOverride::install(
        "permissions",
        "{ query: () => Promise.resolve(Object.create(PermissionStatus.prototype, \
             { state: { value: 'granted' } })) }",
    )
}

/// A position shaped like a real `GeolocationPosition` (same fields; the
/// real class can't be constructed from script).
const FIX: &str = "{ coords: { latitude: 51.5, longitude: -0.12, accuracy: 8, \
                   altitude: null, heading: NaN, speed: 0 }, timestamp: 1700000000000 }";

fn fix() -> Position {
    Position {
        latitude: 51.5,
        longitude: -0.12,
        accuracy_m: 8.0,
        altitude: None,
        heading: Some(f64::NAN),
        speed: Some(0.0),
        timestamp_ms: 1_700_000_000_000.0,
    }
}

fn assert_fix(p: &Position) {
    let want = fix();
    assert_eq!(
        (p.latitude, p.longitude, p.accuracy_m, p.altitude, p.speed, p.timestamp_ms),
        (want.latitude, want.longitude, want.accuracy_m, want.altitude, want.speed, want.timestamp_ms)
    );
    // Present-but-NaN (the spec's "not moving") stays present.
    assert!(p.heading.is_some_and(f64::is_nan), "{:?}", p.heading);
}

async fn next_task() {
    let p = eval("return new Promise((r) => setTimeout(r, 0));");
    let _ = JsFuture::new(&p).await;
}

#[wasm_bindgen_test]
async fn current_decodes_the_fix() {
    let _perm = grant_geolocation();
    let _geo = NavOverride::install(
        "geolocation",
        &format!("{{ getCurrentPosition: (ok, err, opts) => {{ \
                     globalThis.__opts = JSON.stringify(opts); setTimeout(() => ok({FIX}), 0); }} }}"),
    );
    let p = current().await.expect("a fix");
    assert_fix(&p);
    let opts = JsValue::global().get("__opts").unwrap().as_string().unwrap();
    assert_eq!(opts, r#"{"enableHighAccuracy":true,"timeout":30000}"#);
}

#[wasm_bindgen_test]
async fn current_maps_position_errors() {
    let _perm = grant_geolocation();
    {
        let _geo = NavOverride::install(
            "geolocation",
            "{ getCurrentPosition: (ok, err) => err({ code: 1, message: 'denied' }) }",
        );
        assert_eq!(current().await, Err(LocationError::NotAuthorized));
    }
    {
        let _geo = NavOverride::install(
            "geolocation",
            "{ getCurrentPosition: (ok, err) => err({ code: 3, message: 'Timeout expired' }) }",
        );
        assert_eq!(current().await, Err(LocationError::Unavailable("Timeout expired".into())));
    }
    {
        let _geo = NavOverride::install(
            "geolocation",
            "{ getCurrentPosition: () => { throw new Error('boom'); } }",
        );
        assert!(matches!(current().await, Err(LocationError::Unavailable(_))));
    }
    {
        let _geo = NavOverride::install("geolocation", "undefined");
        assert_eq!(current().await, Err(LocationError::NotSupported));
    }
}

/// Regression: the web-sys port accepted a watch update only if it was
/// `instanceof Position` — the class's pre-2020 name, which no current
/// browser defines — so `watch` never delivered a single position. A real
/// browser hands a `GeolocationPosition`; the stand-in has its shape.
#[wasm_bindgen_test]
async fn regression_watch_delivers_positions() {
    assert!(
        JsValue::global().get("Position").unwrap().is_undefined(),
        "this browser still defines the legacy `Position` class"
    );
    let _geo = NavOverride::install(
        "geolocation",
        &format!("{{ watchPosition: (ok) => {{ globalThis.__watchOk = ok; return 42; }}, \
                     clearWatch: (id) => {{ globalThis.__cleared = id; }} }}"),
    );
    let seen: Rc<RefCell<Vec<Position>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    let guard = watch(move |p| sink.borrow_mut().push(p));

    eval(&format!("globalThis.__watchOk({FIX}); globalThis.__watchOk({FIX});"));
    next_task().await;
    assert_eq!(seen.borrow().len(), 2);
    assert_fix(&seen.borrow()[0]);

    // Dropping the guard clears the native watch by its id.
    drop(guard);
    assert_eq!(JsValue::global().get("__cleared").unwrap().as_f64(), Some(42.0));
}

/// Without geolocation the guard is still returned and drops cleanly.
#[wasm_bindgen_test]
fn watch_without_geolocation_is_inert() {
    let _geo = NavOverride::install("geolocation", "undefined");
    let guard = watch(|_| panic!("no geolocation, no updates"));
    drop(guard);
}
