//! Browser tests: the `localStorage` store answers the synchronous
//! accessors, namespaced exactly like the async API, and `clear()` wipes
//! only its own namespace.
//!
//! Run with `cargo test -p storage --target wasm32-unknown-unknown` (the
//! workspace runner supplies web-glue's JS; `wasm-pack test` cannot).

#![cfg(target_arch = "wasm32")]

use storage::platform_storage;
use wasm_bindgen_test::*;
use web_glue::JsValue;

wasm_bindgen_test_configure!(run_in_browser);

fn local_storage() -> JsValue {
    JsValue::global().get("localStorage").expect("localStorage")
}

/// `localStorage.getItem(key)` read straight from the page, bypassing the
/// SDK.
fn raw_get(key: &str) -> Option<String> {
    local_storage()
        .call_method("getItem", &[&JsValue::from_str(key)])
        .expect("getItem")
        .as_string()
}

fn raw_set(key: &str, value: &str) {
    local_storage()
        .call_method("setItem", &[&JsValue::from_str(key), &JsValue::from_str(value)])
        .expect("setItem");
}

#[wasm_bindgen_test]
fn local_storage_answers_now_under_its_namespace() {
    let store = platform_storage("now-test");
    store.set_now("theme", "dark").unwrap();
    assert_eq!(store.get_now("theme"), Ok(Some("dark".to_string())));

    // Same key the async API would write: `<namespace>:<key>`.
    assert_eq!(raw_get("now-test:theme").as_deref(), Some("dark"));

    store.remove_now("theme").unwrap();
    assert_eq!(store.get_now("theme"), Ok(None));
    assert_eq!(raw_get("now-test:theme"), None);
}

/// Non-ASCII values round-trip byte-exact through both string directions.
#[wasm_bindgen_test]
fn local_storage_round_trips_non_ascii() {
    let store = platform_storage("utf8-test");
    let value = "やあ — ünïcödé 🎉";
    store.set_now("k", value).unwrap();
    assert_eq!(store.get_now("k"), Ok(Some(value.to_string())));
    assert_eq!(raw_get("utf8-test:k").as_deref(), Some(value));
    store.remove_now("k").unwrap();
}

/// `clear()` walks `localStorage` by index and removes only keys under its
/// own `<namespace>:` prefix.
#[wasm_bindgen_test]
async fn clear_removes_only_its_own_namespace() {
    let mine = platform_storage("clear-mine");
    mine.set("a", "1").await.unwrap();
    mine.set("b", "2").await.unwrap();
    raw_set("clear-other:a", "keep");
    raw_set("unprefixed", "keep");

    mine.clear().await.unwrap();

    assert_eq!(mine.get("a").await, Ok(None));
    assert_eq!(mine.get("b").await, Ok(None));
    assert_eq!(raw_get("clear-other:a").as_deref(), Some("keep"));
    assert_eq!(raw_get("unprefixed").as_deref(), Some("keep"));

    local_storage().call_method("removeItem", &[&JsValue::from_str("clear-other:a")]).unwrap();
    local_storage().call_method("removeItem", &[&JsValue::from_str("unprefixed")]).unwrap();
}
