//! Browser tests for the live-address half of the SDK: `current_url`,
//! `origin` and `replace_url` against a real `window.location` /
//! `history`.
//!
//! Browser-only, because the whole mechanism is: run with
//! `cargo test -p deep-link --target wasm32-unknown-unknown` (the
//! workspace runner supplies web-glue's JS; `wasm-pack test` cannot).

#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::*;
use web_glue::JsValue;

wasm_bindgen_test_configure!(run_in_browser);

/// `window[a][b]` as a string, read straight from the page.
fn page_str(a: &str, b: &str) -> String {
    JsValue::global().get(a).unwrap().get(b).unwrap().as_string().unwrap()
}

fn history() -> JsValue {
    JsValue::global().get("history").unwrap()
}

fn history_length() -> f64 {
    history().get("length").unwrap().as_f64().unwrap()
}

fn location_path_and_search() -> (String, String) {
    (page_str("location", "pathname"), page_str("location", "search"))
}

#[wasm_bindgen_test]
fn replace_url_rewrites_the_address_and_current_url_follows_it() {
    let before = history_length();

    deep_link::replace_url("/projects/42?tab=a%20b&flag");

    // The real address bar moved…
    assert_eq!(
        location_path_and_search(),
        ("/projects/42".to_string(), "?tab=a%20b&flag".to_string())
    );
    // …without adding a history entry (replace, never push).
    let after = history_length();
    assert_eq!(before, after, "replace_url must not push a history entry");

    // And the live read reports the new address, decoded.
    let url = deep_link::current_url().expect("a browser always has an address");
    assert_eq!(url.path, "/projects/42");
    assert_eq!(
        url.query_pairs(),
        vec![
            ("tab".to_string(), "a b".to_string()),
            ("flag".to_string(), String::new()),
        ]
    );
}

#[wasm_bindgen_test]
fn replace_url_keeps_the_entrys_history_state() {
    // A navigator (or anyone) parked state on the current entry.
    history()
        .call_method("replaceState", &[&JsValue::from_str("nav-entry-7"), &JsValue::from_str("")])
        .unwrap();

    deep_link::replace_url("/elsewhere");

    assert_eq!(
        history().get("state").unwrap().as_string().as_deref(),
        Some("nav-entry-7"),
        "rewriting the address must not wipe the entry's state"
    );
}

#[wasm_bindgen_test]
fn origin_is_the_pages_origin_without_a_trailing_slash() {
    let expected = page_str("location", "origin");
    let origin = deep_link::origin().expect("the test page is served over http");
    assert_eq!(origin, expected);
    assert!(!origin.ends_with('/'));
    // It is what an absolute link is built from.
    let url = deep_link::DeepLink::parse(&format!("{origin}/share/abc")).unwrap();
    assert_eq!(url.path, "/share/abc");
}

#[wasm_bindgen_test]
fn replace_url_does_not_dispatch_to_link_handlers() {
    use std::cell::Cell;
    use std::rc::Rc;
    let fired = Rc::new(Cell::new(0u32));
    let f = Rc::clone(&fired);
    let _sub = deep_link::on_link(move |_| f.set(f.get() + 1));
    deep_link::replace_url("/quiet");
    assert_eq!(fired.get(), 0);
}

/// `history.replaceState` throws a `SecurityError` for a URL on another
/// origin. That throw must stay inside the binding (`#[catch]`) — the
/// address is left alone and the caller never sees an exception.
#[wasm_bindgen_test]
fn replace_url_to_another_origin_is_ignored() {
    deep_link::replace_url("/before-cross-origin");
    deep_link::replace_url("https://example.invalid/elsewhere");
    assert_eq!(page_str("location", "pathname"), "/before-cross-origin");
}
