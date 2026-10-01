//! Plain property accessors (`dom_api.rs`'s `prop_*!` / `set_*!` /
//! `call0!` macros) in a real browser.
//!
//! Each accessor is its own glue import with the property name baked into
//! its JS. They used to share one generic import per kind that took the
//! name as `(ptr, len)` and decoded it with `TextDecoder` on every call —
//! on the benchmark's theme toggle (`sheet`, `cssRules`, `style` ×2,
//! `documentElement`) that decode was the whole gap to the web-sys build.
//! These tests pin every accessor kind's behaviour (values, `null` →
//! `None`, setters, the `call0!` method form, handle ownership) and that
//! no accessor decodes a name.

#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};
use web_glue::dom::*;
use web_glue::{JsCast, JsValue};

wasm_bindgen_test_configure!(run_in_browser);

/// `new Function(body)` — a JS helper for the test's own probes.
fn js_fn(args: &str, body: &str) -> JsValue {
    let mut ctor_args: Vec<JsValue> = args
        .split(',')
        .filter(|a| !a.is_empty())
        .map(JsValue::from_str)
        .collect();
    ctor_args.push(JsValue::from_str(body));
    let refs: Vec<&JsValue> = ctor_args.iter().collect();
    JsValue::global()
        .get("Function")
        .unwrap()
        .construct(&refs)
        .expect("new Function")
}

fn doc() -> Document {
    window().unwrap().document().unwrap()
}

/// A `<style>` attached to `<head>` with one `:root` rule, the shape
/// backend-web's theme tokens live in.
fn style_sheet_with_root_rule() -> (HtmlStyleElement, CssStyleSheet) {
    let d = doc();
    let el: HtmlStyleElement = d.create_element("style").unwrap().dyn_into().unwrap();
    // Detached: a `<style>` has no sheet until it is in a document.
    assert!(el.sheet().is_none(), "a detached <style> has no sheet");
    d.head().unwrap().append_child(&el).unwrap();
    let sheet: CssStyleSheet = el
        .sheet()
        .expect("attached <style> has a sheet")
        .dyn_into()
        .unwrap();
    sheet
        .insert_rule_with_index(":root { --web-props-probe: red; }", 0)
        .unwrap();
    (el, sheet)
}

#[wasm_bindgen_test]
fn property_getters_read_values_and_null_is_none() {
    let d = doc();
    // obj / Option<obj>
    let root = d.document_element().expect("documentElement");
    assert_eq!(root.tag_name(), "HTML");
    let body = d.body().expect("body");
    let div: HtmlElement = d.create_element("div").unwrap().dyn_into().unwrap();
    assert!(
        div.parent_node().is_none(),
        "parentNode of a detached node is None"
    );
    body.append_child(&div).unwrap();
    let parent: JsValue = div.parent_node().unwrap().into();
    let body_v: &JsValue = body.as_ref();
    assert!(parent.strict_eq(body_v));
    // String / Option<String>
    assert_eq!(div.node_name(), "DIV");
    assert_eq!(d.ready_state(), "complete");
    // u32 / i32 / bool
    assert_eq!(div.node_type(), 1);
    assert_eq!(div.tab_index(), -1);
    assert!(div.is_connected());
    assert!(!div.hidden());
    div.remove();
    assert!(!div.is_connected());
}

#[wasm_bindgen_test]
fn property_setters_write_through() {
    let d = doc();
    let input: HtmlInputElement = d.create_element("input").unwrap().dyn_into().unwrap();
    input.set_value("héllo ✓"); // &str, non-ASCII
    assert_eq!(input.value(), "héllo ✓");
    input.set_disabled(true); // bool
    assert!(input.disabled());
    input.set_max_length(7); // i32
    assert_eq!(input.get_attribute("maxlength").as_deref(), Some("7"));
    let text = d.create_text_node("abc");
    text.set_node_value(None); // obj (null)
    assert_eq!(text.text_content().as_deref(), Some(""));
    text.set_node_value(Some("xyz")); // &str
    assert_eq!(text.text_content().as_deref(), Some("xyz"));
}

#[wasm_bindgen_test]
fn theme_toggle_accessor_chain_updates_the_rule() {
    // The exact chain backend-web's `impl_install_theme_variables` walks.
    let (el, sheet) = style_sheet_with_root_rule();
    let rules = sheet.css_rules().unwrap();
    assert_eq!(rules.length(), 1);
    let rule: CssStyleRule = rules.get(0).unwrap().dyn_into().unwrap();
    assert_eq!(rule.selector_text(), ":root");
    let decl = rule.style();
    decl.set_property("--web-props-probe", "blue").unwrap();
    assert_eq!(
        decl.get_property_value("--web-props-probe").unwrap().trim(),
        "blue"
    );
    el.remove();
}

#[wasm_bindgen_test]
fn accessors_release_every_handle_they_mint() {
    let (el, sheet) = style_sheet_with_root_rule();
    let d = doc();
    let base = JsValue::live_count();
    let js_base = JsValue::js_live_count();
    for _ in 0..5000 {
        let rules = sheet.css_rules().unwrap();
        let rule: CssStyleRule = rules.get(0).unwrap().dyn_into().unwrap();
        let _decl = rule.style();
        let _root = d.document_element().unwrap();
        let _sheet = el.sheet().unwrap();
        let _none = d.create_element("p").unwrap().parent_node();
    }
    assert_eq!(JsValue::live_count(), base, "Rust-side handles leaked");
    assert_eq!(JsValue::js_live_count(), js_base, "JS slab slots leaked");
    el.remove();
}

/// The accessors used to decode their property name per call (one
/// generic import per kind); 2000 reads + writes decoded 2000 names (this
/// test failed with `decodes == 2000`). Now the name is in the snippet.
#[wasm_bindgen_test]
fn regression_property_access_decodes_no_name_per_call() {
    let (el, sheet) = style_sheet_with_root_rule();
    let d = doc();
    let input: HtmlInputElement = d.create_element("input").unwrap().dyn_into().unwrap();
    let install = js_fn(
        "",
        "const P = TextDecoder.prototype; const orig = P.decode; globalThis.__propDecodes = 0; \
         P.decode = function (...a) { globalThis.__propDecodes++; return orig.apply(this, a); }; \
         return () => { P.decode = orig; return globalThis.__propDecodes; };",
    );
    let restore = install.call(&JsValue::NULL, &[]).unwrap();
    let mut seen = 0u32;
    for i in 0..250 {
        seen += el.sheet().is_some() as u32; // Option<obj>
        seen += sheet.css_rules().unwrap().length(); // obj + u32
        seen += d.document_element().is_some() as u32; // Option<obj>
        seen += input.disabled() as u32; // bool
        input.set_disabled(i % 2 == 0); // bool setter
        input.set_max_length(i); // num setter
        seen += (input.node_type() == 1) as u32; // u32
    }
    let decodes = restore.call(&JsValue::NULL, &[]).unwrap().as_f64().unwrap();
    // `disabled` reads true on the odd iterations (set on the even ones).
    assert_eq!(seen, 250 * 4 + 125);
    assert_eq!(
        decodes, 0.0,
        "2000 property accesses decoded {decodes} strings"
    );
    el.remove();
}
