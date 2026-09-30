//! The DOM surface this demo needs, declared the way a framework crate
//! would declare its own: one `import!` per operation, JS inline. Compare
//! the call shape with web-sys — one import call per DOM operation, strings
//! passed as `(ptr, len)` and decoded once on the JS side.

// Some of these are exercised only by the `selftest` module.
#![cfg_attr(not(feature = "selftest"), allow(dead_code))]

use web_glue::{import, js_module, string, JsValue};

// A crate-shipped JS module (the `__idealyst_glue` custom-section carrier).
// `rows_module()` is its anchor: calling it is what guarantees the record
// is linked (see `web_glue::js_module!`).
js_module!(pub fn rows_module = "own-glue-demo/rows", include_str!("rows.js"));

import! {
    fn js_document_body() -> u32 = "() => G.add(document.body)";
    fn js_create_element(p: usize, l: usize) -> u32 =
        "(p, l) => G.add(document.createElement(G.str(p, l)))";
    fn js_set_attribute(el: u32, np: usize, nl: usize, vp: usize, vl: usize) =
        "(e, np, nl, vp, vl) => { G.get(e).setAttribute(G.str(np, nl), G.str(vp, vl)); }";
    fn js_set_text(el: u32, p: usize, l: usize) =
        "(e, p, l) => { G.get(e).textContent = G.str(p, l); }";
    fn js_text(el: u32, out: usize) = "(e, o) => G.retStr(G.get(e).textContent, o)";
    fn js_append(parent: u32, child: u32) = "(p, c) => { G.get(p).appendChild(G.get(c)); }";
    fn js_clear(el: u32) = "(e) => { G.get(e).replaceChildren(); }";
    fn js_add_listener(el: u32, p: usize, l: usize, f: u32) =
        "(e, p, l, f) => { G.get(e).addEventListener(G.str(p, l), G.get(f)); }";
    fn js_remove_listener(el: u32, p: usize, l: usize, f: u32) =
        "(e, p, l, f) => { G.get(e).removeEventListener(G.str(p, l), G.get(f)); }";
    fn js_sleep(ms: f64) -> u32 = "(ms) => G.add(new Promise((r) => setTimeout(() => r(ms), ms)))";
    #[catch]
    fn js_throw_range_error(p: usize, l: usize) = "(p, l) => { throw new RangeError(G.str(p, l)); }";
    fn js_set_global(p: usize, l: usize, v: u32) = "(p, l, v) => { globalThis[G.str(p, l)] = G.get(v); }";
    fn js_row_count(el: u32) -> u32 = "(e) => G.m('own-glue-demo/rows').rowCount(G.get(e))";
}

pub fn body() -> JsValue {
    unsafe { JsValue::from_raw(js_document_body()) }
}

pub fn create(tag: &str) -> JsValue {
    let (p, l) = string::abi(tag);
    unsafe { JsValue::from_raw(js_create_element(p, l)) }
}

pub fn set_attr(el: &JsValue, name: &str, value: &str) {
    let (np, nl) = string::abi(name);
    let (vp, vl) = string::abi(value);
    unsafe { js_set_attribute(el.raw(), np, nl, vp, vl) }
}

pub fn set_text(el: &JsValue, text: &str) {
    let (p, l) = string::abi(text);
    unsafe { js_set_text(el.raw(), p, l) }
}

pub fn text(el: &JsValue) -> String {
    string::receive(|out| unsafe { js_text(el.raw(), out) })
}

pub fn append(parent: &JsValue, child: &JsValue) {
    unsafe { js_append(parent.raw(), child.raw()) }
}

pub fn clear(el: &JsValue) {
    unsafe { js_clear(el.raw()) }
}

pub fn add_listener(el: &JsValue, ty: &str, f: &JsValue) {
    let (p, l) = string::abi(ty);
    unsafe { js_add_listener(el.raw(), p, l, f.raw()) }
}

pub fn remove_listener(el: &JsValue, ty: &str, f: &JsValue) {
    let (p, l) = string::abi(ty);
    unsafe { js_remove_listener(el.raw(), p, l, f.raw()) }
}

/// A Promise that resolves to `ms` after `ms` milliseconds.
pub fn sleep(ms: f64) -> JsValue {
    unsafe { JsValue::from_raw(js_sleep(ms)) }
}

pub fn throw_range_error(msg: &str) -> Result<(), web_glue::JsError> {
    let (p, l) = string::abi(msg);
    unsafe { js_throw_range_error(p, l) }
}

pub fn set_global(name: &str, v: &JsValue) {
    let (p, l) = string::abi(name);
    unsafe { js_set_global(p, l, v.raw()) }
}

/// Through the crate's JS module.
pub fn row_count(el: &JsValue) -> u32 {
    rows_module();
    unsafe { js_row_count(el.raw()) }
}

/// `<tag id=id>` appended to `parent`.
pub fn child(parent: &JsValue, tag: &str, id: &str) -> JsValue {
    let el = create(tag);
    set_attr(&el, "id", id);
    append(parent, &el);
    el
}
