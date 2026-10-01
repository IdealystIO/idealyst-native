//! JS → Rust strings (`G.retStr` → `string::receive`) in a real browser:
//! every shape of string round-trips, the buffer Rust adopts is exactly
//! the string's size, memory growth in the middle of a return is survived,
//! and an ASCII return never goes through `TextEncoder`.

#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};
use web_glue::{string, JsValue};

wasm_bindgen_test_configure!(run_in_browser);

/// `new Function(args, body)`.
fn js_fn(args: &str, body: &str) -> JsValue {
    let mut a: Vec<JsValue> = args.split(',').filter(|a| !a.is_empty()).map(JsValue::from_str).collect();
    a.push(JsValue::from_str(body));
    let refs: Vec<&JsValue> = a.iter().collect();
    JsValue::global().get("Function").unwrap().construct(&refs).expect("new Function")
}

/// A JS string built in JS (so it never crossed Rust → JS as UTF-8),
/// read back through `retStr`.
fn js_string(expr: &str) -> String {
    let v = js_fn("", &format!("return {expr};")).call(&JsValue::NULL, &[]).unwrap();
    v.as_string().expect("a string")
}

#[wasm_bindgen_test]
fn every_string_shape_round_trips() {
    let cases = [
        "",
        "a",
        "translate(12px, 4px)",
        "é",
        "abcé",
        "éabc",
        "日本語",
        "a😀b",
        "😀",
        "mixed ascii, ü, 中, 😀 and back to ascii",
    ];
    for s in cases {
        let back = JsValue::from_str(s).as_string().unwrap();
        assert_eq!(back, s);
        assert_eq!(back.capacity(), back.len(), "adopted buffer must be exactly sized for {s:?}");
    }
    // Long strings: the ASCII loop end to end, and a non-ASCII tail after a
    // long ASCII prefix (the realloc path's worst-case sizing).
    let long_ascii = js_string("'x'.repeat(200000)");
    assert_eq!(long_ascii, "x".repeat(200_000));
    let long_tail = js_string("'y'.repeat(100000) + '日本😀'.repeat(5000)");
    assert_eq!(long_tail, format!("{}{}", "y".repeat(100_000), "日本😀".repeat(5000)));
    assert_eq!(long_tail.capacity(), long_tail.len());
}

#[wasm_bindgen_test]
fn a_lone_surrogate_becomes_the_replacement_character() {
    // What TextEncoder does with unpaired UTF-16, so the bytes Rust adopts
    // are always valid UTF-8.
    assert_eq!(js_string("'a\\uD800b'"), "a\u{FFFD}b");
    assert_eq!(js_string("'\\uDC00'"), "\u{FFFD}");
}

#[wasm_bindgen_test]
fn memory_growth_during_a_string_return_is_survived() {
    // Growth inside `__glue_alloc`: the ASCII loop must write through a
    // view taken after it.
    string::debug_grow_on_next_alloc();
    assert_eq!(js_string("'grown before the ascii copy'"), "grown before the ascii copy");
    // Growth inside `__glue_realloc`: the non-ASCII tail's `encodeInto`
    // must write through a view taken after it.
    string::debug_grow_on_next_realloc();
    assert_eq!(js_string("'ascii prefix, then ü日😀'"), "ascii prefix, then ü日😀");
}

/// Regression: every JS → Rust string used to be `TextEncoder.encode`d
/// into a temporary array and copied in (~176 ns for a short CSS value
/// against ~21 ns for the direct copy; the theme toggle reads strings back
/// on every swap). An ASCII return must not touch `TextEncoder` at all,
/// and a non-ASCII one must encode in place (`encodeInto`), never through
/// `encode`.
#[wasm_bindgen_test]
fn regression_string_return_encodes_in_place() {
    let ascii = JsValue::from_str("font-family");
    let wide = JsValue::from_str("Ünïcødé font");
    let install = js_fn(
        "",
        "const P = TextEncoder.prototype; const e = P.encode, i = P.encodeInto; \
         const n = { encode: 0, encodeInto: 0 }; \
         P.encode = function (...a) { n.encode++; return e.apply(this, a); }; \
         P.encodeInto = function (...a) { n.encodeInto++; return i.apply(this, a); }; \
         return () => { P.encode = e; P.encodeInto = i; return n.encode * 1000 + n.encodeInto; };",
    );
    let restore = install.call(&JsValue::NULL, &[]).unwrap();
    for _ in 0..100 {
        assert_eq!(ascii.as_string().unwrap(), "font-family");
    }
    let wide_back = wide.as_string().unwrap();
    let counts = restore.call(&JsValue::NULL, &[]).unwrap().as_f64().unwrap() as u32;
    assert_eq!(wide_back, "Ünïcødé font");
    assert_eq!(counts / 1000, 0, "TextEncoder.encode ran {} times", counts / 1000);
    assert_eq!(counts % 1000, 1, "one encodeInto, for the one non-ASCII string");
}
