//! Browser tests for the web connectivity backend: `current()` against
//! `navigator.onLine` / `navigator.connection`, and the `watch` guard's
//! listener lifecycle on `window`'s `online` / `offline` events.
//!
//! Run with `cargo test -p connectivity --target wasm32-unknown-unknown`
//! (the workspace runner supplies web-glue's JS; `wasm-pack test` cannot).

#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::rc::Rc;

use connectivity::{current, watch, Connectivity, Transport};
use wasm_bindgen_test::*;
use web_glue::JsValue;

wasm_bindgen_test_configure!(run_in_browser);

fn run(body: &str) {
    let f = JsValue::global()
        .get("Function")
        .unwrap()
        .construct(&[&JsValue::from_str(body)])
        .unwrap();
    f.call(&JsValue::undefined(), &[]).unwrap();
}

/// Shadow `navigator.<prop>` with a JS expression until the guard drops.
struct NavOverride(&'static str);

impl NavOverride {
    fn install(prop: &'static str, js_expr: &str) -> NavOverride {
        run(&format!(
            "Object.defineProperty(navigator, '{prop}', {{ value: {js_expr}, configurable: true }});"
        ));
        NavOverride(prop)
    }
}

impl Drop for NavOverride {
    fn drop(&mut self) {
        run(&format!("delete navigator.{};", self.0));
    }
}

fn dispatch(ty: &str) {
    run(&format!("window.dispatchEvent(new Event('{ty}'));"));
}

#[wasm_bindgen_test]
fn offline_flag_is_offline() {
    let _off = NavOverride::install("onLine", "false");
    assert_eq!(current(), Connectivity::OFFLINE);
}

#[wasm_bindgen_test]
fn transport_hint_comes_from_navigator_connection() {
    let _on = NavOverride::install("onLine", "true");
    {
        let _c = NavOverride::install("connection", "({ type: 'wifi', effectiveType: '4g' })");
        assert_eq!(current(), Connectivity { online: true, transport: Transport::Wifi });
    }
    {
        // No `type` (Chrome desktop): a cellular-ish effectiveType wins.
        let _c = NavOverride::install("connection", "({ effectiveType: '3g' })");
        assert_eq!(current().transport, Transport::Cellular);
    }
    {
        // Safari / Firefox: no NetworkInformation at all.
        let _c = NavOverride::install("connection", "undefined");
        assert_eq!(current(), Connectivity { online: true, transport: Transport::Other });
    }
}

/// `watch` delivers a fresh snapshot on `online` / `offline`, and dropping
/// the guard REMOVES the listeners: a later event neither reaches the
/// callback nor throws into the page from a dead closure.
#[wasm_bindgen_test]
fn watch_delivers_and_its_guard_detaches() {
    let seen: Rc<RefCell<Vec<bool>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    let sub = watch(move |c| sink.borrow_mut().push(c.online));

    {
        let _off = NavOverride::install("onLine", "false");
        dispatch("offline");
    }
    {
        let _on = NavOverride::install("onLine", "true");
        dispatch("online");
    }
    assert_eq!(*seen.borrow(), vec![false, true]);

    drop(sub);
    // A listener left attached with a dropped closure would throw here
    // (web-glue: "called after its Rust owner dropped it"); a listener's
    // throw is reported as a window `error` event, which this records.
    run("globalThis.__errs = []; globalThis.__spy = (e) => __errs.push(e.message); \
         window.addEventListener('error', __spy);");
    dispatch("offline");
    dispatch("online");
    run("window.removeEventListener('error', __spy);");
    let errs = JsValue::global().get("__errs").unwrap().get("length").unwrap().as_f64();
    assert_eq!(errs, Some(0.0), "an event after the guard dropped reached a dead listener");
    assert_eq!(*seen.borrow(), vec![false, true]);
}
