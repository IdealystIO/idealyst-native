//! Browser tests for the web backend, against a fake `navigator.wakeLock`.
//!
//! Run with `cargo test -p keep-awake --target wasm32-unknown-unknown` (the
//! workspace runner supplies web-glue's JS; `wasm-pack test` cannot).

#![cfg(target_arch = "wasm32")]

use keep_awake::{is_supported, keep_screen_on};
use runtime_shared::app_state::{report, AppStateChange};
use wasm_bindgen_test::*;
use web_glue::{JsFuture, JsValue};

wasm_bindgen_test_configure!(run_in_browser);

fn eval(body: &str) -> JsValue {
    let f = JsValue::global().get("Function").unwrap().construct(&[&JsValue::from_str(body)]).unwrap();
    f.call(&JsValue::undefined(), &[]).unwrap()
}

/// Let queued promise reactions and tasks run. One timer: Safari throttles
/// them in a window that is not in front.
async fn settle() {
    JsFuture::new(&eval("return new Promise(r => setTimeout(r, 0));")).await.unwrap();
}

/// A fake that counts requests and the sentinels still held, and refuses
/// while `__refuse`.
fn install_fake() {
    eval(
        "globalThis.__req = 0; globalThis.__held = 0; globalThis.__refuse = false; \
         Object.defineProperty(navigator, 'wakeLock', { configurable: true, value: { \
           request: async (type) => { __req++; \
             if (type !== 'screen') throw new Error('type ' + type); \
             if (__refuse) throw new DOMException('no', 'NotAllowedError'); \
             __held++; const s = { released: false, \
               release: async () => { if (!s.released) { s.released = true; __held--; } } }; \
             globalThis.__last = s; return s; } } });",
    );
}

fn num(name: &str) -> f64 {
    JsValue::global().get(name).unwrap().as_f64().unwrap()
}

#[wasm_bindgen_test]
async fn the_wake_lock_follows_the_holds() {
    install_fake();
    assert!(is_supported());

    let a = keep_screen_on();
    let b = keep_screen_on();
    settle().await;
    assert_eq!((num("__req"), num("__held")), (1.0, 1.0), "one lock for two holds");
    drop(a);
    settle().await;
    assert_eq!(num("__held"), 1.0);
    drop(b);
    settle().await;
    assert_eq!(num("__held"), 0.0, "released");

    // Dropped while the request is out.
    drop(keep_screen_on());
    settle().await;
    assert_eq!(num("__held"), 0.0, "a late grant is given back");

    // The browser takes the lock when the page hides.
    let c = keep_screen_on();
    settle().await;
    assert_eq!(num("__held"), 1.0);
    eval("__last.release();");
    report(AppStateChange::Visible(false));
    settle().await;
    let before = num("__req");
    assert_eq!(num("__held"), 0.0);
    report(AppStateChange::Visible(true));
    settle().await;
    assert_eq!((num("__req"), num("__held")), (before + 1.0, 1.0), "asked again when shown");
    drop(c);
    settle().await;

    // Refused before the first gesture, granted after it.
    report(AppStateChange::UserActivated(false));
    eval("__refuse = true;");
    let d = keep_screen_on();
    settle().await;
    assert_eq!(num("__held"), 0.0, "refused, and nothing thrown");
    eval("__refuse = false;");
    report(AppStateChange::UserActivated(true));
    settle().await;
    assert_eq!(num("__held"), 1.0, "asked again");
    drop(d);
    settle().await;
    assert_eq!(num("__held"), 0.0);

    // A browser without the API.
    eval("Object.defineProperty(navigator, 'wakeLock', { configurable: true, value: undefined });");
    assert!(!is_supported());
    drop(keep_screen_on());
    settle().await;
    eval("delete navigator.wakeLock;");
}
