//! Browser test: the `localStorage` store answers the synchronous
//! accessors, namespaced exactly like the async API.
//!
//! Run with `wasm-pack test --headless --chrome --package storage`.

#![cfg(target_arch = "wasm32")]

use storage::platform_storage;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
fn local_storage_answers_now_under_its_namespace() {
    let store = platform_storage("now-test");
    store.set_now("theme", "dark").unwrap();
    assert_eq!(store.get_now("theme"), Ok(Some("dark".to_string())));

    // Same key the async API would write: `<namespace>:<key>`.
    let raw = web_sys::window()
        .unwrap()
        .local_storage()
        .unwrap()
        .unwrap()
        .get_item("now-test:theme")
        .unwrap();
    assert_eq!(raw.as_deref(), Some("dark"));

    store.remove_now("theme").unwrap();
    assert_eq!(store.get_now("theme"), Ok(None));
}
