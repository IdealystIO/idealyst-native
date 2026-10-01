//! The DOM operations the framework performs, on the [`crate::dom`] classes,
//! with web-sys's names and signatures (`Node::append_child ->
//! Result<Node, _>`, `Element::set_attribute -> Result<(), _>`,
//! `Window::document -> Option<Document>`, …) so ported call sites change
//! paths, not shapes. Errors are [`JsError`].
//!
//! Every operation is its own snippet: hot ones (creation, attributes,
//! text, tree edits, inline style, class lists, measurement) hand-written,
//! and plain property reads and writes through the `prop_*!` / `set_*!`
//! macros, which bake the property name into the snippet — no operation
//! decodes a name string per call.

use crate::cast::JsCast;
use crate::dom::*;
use crate::js::{Array, Function, Object};
use crate::{string, JsError, JsValue};

fn opt<T: JsCast>(idx: u32) -> Option<T> {
    (idx != 0 && idx != 1).then(|| T::unchecked_from_js(unsafe { JsValue::from_raw(idx) }))
}

fn owned<T: JsCast>(idx: u32) -> T {
    T::unchecked_from_js(unsafe { JsValue::from_raw(idx) })
}

fn h(v: &impl JsCast) -> u32 {
    v.as_ref().raw()
}

crate::import! {
    // ---- Window / Document -------------------------------------------------
    fn js_document(w: u32) -> u32 = "(w) => G.add(G.get(w).document)";
    #[catch]
    fn js_create_element(d: u32, p: usize, l: usize) -> u32 =
        "(d, p, l) => G.add(G.get(d).createElement(G.str(p, l)))";
    #[catch]
    fn js_create_element_ns(d: u32, np: usize, nl: usize, has_ns: u32, p: usize, l: usize) -> u32 =
        "(d, np, nl, h, p, l) => G.add(G.get(d).createElementNS(h ? G.str(np, nl) : null, G.str(p, l)))";
    fn js_create_text_node(d: u32, p: usize, l: usize) -> u32 =
        "(d, p, l) => G.add(G.get(d).createTextNode(G.str(p, l)))";
    fn js_create_fragment(d: u32) -> u32 = "(d) => G.add(G.get(d).createDocumentFragment())";
    fn js_get_element_by_id(d: u32, p: usize, l: usize) -> u32 =
        "(d, p, l) => G.add(G.get(d).getElementById(G.str(p, l)))";
    #[catch]
    fn js_computed_style(w: u32, e: u32) -> u32 = "(w, e) => G.add(G.get(w).getComputedStyle(G.get(e)))";
    #[catch]
    fn js_window_open(w: u32, up: usize, ul: usize, tp: usize, tl: usize, fp: usize, fl: usize) -> u32 =
        "(w, up, ul, tp, tl, fp, fl) => G.add(G.get(w).open(G.str(up, ul), G.str(tp, tl), G.str(fp, fl)))";
    #[catch]
    fn js_history_state(hi: u32, st: u32, up: usize, ul: usize, replace: u32) =
        "(h, s, up, ul, r) => { const x = G.get(h); \
           if (r) x.replaceState(G.get(s), '', G.str(up, ul)); else x.pushState(G.get(s), '', G.str(up, ul)); }";

    // ---- Node --------------------------------------------------------------
    #[catch]
    fn js_append_child(p: u32, c: u32) -> u32 = "(p, c) => G.add(G.get(p).appendChild(G.get(c)))";
    #[catch]
    fn js_insert_before(p: u32, c: u32, r: u32) -> u32 =
        "(p, c, r) => G.add(G.get(p).insertBefore(G.get(c), G.get(r)))";
    #[catch]
    fn js_remove_child(p: u32, c: u32) -> u32 = "(p, c) => G.add(G.get(p).removeChild(G.get(c)))";
    #[catch]
    fn js_replace_child(p: u32, n: u32, o: u32) -> u32 =
        "(p, n, o) => G.add(G.get(p).replaceChild(G.get(n), G.get(o)))";
    fn js_set_text_content(n: u32, has: u32, p: usize, l: usize) =
        "(n, h, p, l) => { G.get(n).textContent = h ? G.str(p, l) : null; }";
    fn js_contains(a: u32, b: u32) -> u32 = "(a, b) => G.get(a).contains(G.get(b)) ? 1 : 0";
    fn js_same(a: u32, b: u32) -> u32 = "(a, b) => G.get(a) === G.get(b) ? 1 : 0";
    #[catch]
    fn js_clone_node(n: u32, deep: u32) -> u32 = "(n, d) => G.add(G.get(n).cloneNode(d !== 0))";
    fn js_list_item(l: u32, i: u32) -> u32 = "(l, i) => G.add(G.get(l).item(i >>> 0))";
    #[catch]
    fn js_dispatch(t: u32, e: u32) -> u32 = "(t, e) => G.get(t).dispatchEvent(G.get(e)) ? 1 : 0";

    // ---- Element -----------------------------------------------------------
    #[catch]
    fn js_set_attribute(e: u32, np: usize, nl: usize, vp: usize, vl: usize) =
        "(e, np, nl, vp, vl) => { G.get(e).setAttribute(G.str(np, nl), G.str(vp, vl)); }";
    #[catch]
    fn js_remove_attribute(e: u32, p: usize, l: usize) = "(e, p, l) => { G.get(e).removeAttribute(G.str(p, l)); }";
    fn js_has_attribute(e: u32, p: usize, l: usize) -> u32 = "(e, p, l) => G.get(e).hasAttribute(G.str(p, l)) ? 1 : 0";
    fn js_get_attribute(e: u32, p: usize, l: usize, out: usize) -> u32 =
        "(e, p, l, o) => { const v = G.get(e).getAttribute(G.str(p, l)); if (v == null) return 0; G.retStr(v, o); return 1; }";
    #[catch]
    fn js_query_selector(e: u32, p: usize, l: usize) -> u32 =
        "(e, p, l) => G.add(G.get(e).querySelector(G.str(p, l)))";
    #[catch]
    fn js_query_selector_all(e: u32, p: usize, l: usize) -> u32 =
        "(e, p, l) => G.add(G.get(e).querySelectorAll(G.str(p, l)))";
    #[catch]
    fn js_closest(e: u32, p: usize, l: usize) -> u32 = "(e, p, l) => G.add(G.get(e).closest(G.str(p, l)))";
    #[catch]
    fn js_matches(e: u32, p: usize, l: usize) -> u32 = "(e, p, l) => G.get(e).matches(G.str(p, l)) ? 1 : 0";
    fn js_rect(e: u32) -> u32 = "(e) => G.add(G.get(e).getBoundingClientRect())";
    #[catch]
    fn js_pointer_capture(e: u32, id: i32, set: u32) =
        "(e, i, s) => { if (s) G.get(e).setPointerCapture(i); else G.get(e).releasePointerCapture(i); }";
    fn js_has_pointer_capture(e: u32, id: i32) -> u32 = "(e, i) => G.get(e).hasPointerCapture(i) ? 1 : 0";
    fn js_remove(e: u32) = "(e) => { G.get(e).remove(); }";
    #[catch]
    fn js_focus(e: u32) = "(e) => { G.get(e).focus(); }";
    #[catch]
    fn js_focus_prevent_scroll(e: u32) = "(e) => { G.get(e).focus({ preventScroll: true }); }";
    #[catch]
    fn js_blur(e: u32) = "(e) => { G.get(e).blur(); }";
    fn js_click(e: u32) = "(e) => { G.get(e).click(); }";
    fn js_select(e: u32) = "(e) => { G.get(e).select(); }";
    #[catch]
    fn js_set_range_text(e: u32, p: usize, l: usize, s: u32, en: u32) =
        "(e, p, l, s, n) => { G.get(e).setRangeText(G.str(p, l), s, n); }";
    fn js_selection(e: u32, which: u32) -> f64 =
        "(e, w) => { const v = w ? G.get(e).selectionEnd : G.get(e).selectionStart; return v == null ? -1 : v; }";
    #[catch]
    fn js_set_selection_range(e: u32, s: u32, en: u32) = "(e, s, n) => { G.get(e).setSelectionRange(s, n); }";

    // ---- classList / style --------------------------------------------------
    #[catch]
    fn js_token(l: u32, p: usize, len: usize, op: u32) -> u32 =
        "(l, p, n, op) => { const x = G.get(l), t = G.str(p, n); \
           if (op === 0) { x.add(t); return 1; } if (op === 1) { x.remove(t); return 1; } \
           if (op === 2) return x.contains(t) ? 1 : 0; return x.toggle(t) ? 1 : 0; }";
    #[catch]
    fn js_set_property(s: u32, np: usize, nl: usize, vp: usize, vl: usize) =
        "(s, np, nl, vp, vl) => { G.get(s).setProperty(G.str(np, nl), G.str(vp, vl)); }";
    #[catch]
    fn js_set_property_prio(s: u32, np: usize, nl: usize, vp: usize, vl: usize, pp: usize, pl: usize) =
        "(s, np, nl, vp, vl, pp, pl) => { G.get(s).setProperty(G.str(np, nl), G.str(vp, vl), G.str(pp, pl)); }";
    #[catch]
    fn js_remove_property(s: u32, p: usize, l: usize, out: usize) =
        "(s, p, l, o) => G.retStr(G.get(s).removeProperty(G.str(p, l)), o)";
    #[catch]
    fn js_get_property_value(s: u32, p: usize, l: usize, out: usize) =
        "(s, p, l, o) => G.retStr(G.get(s).getPropertyValue(G.str(p, l)), o)";

    // ---- CSSOM -----------------------------------------------------------------
    #[catch]
    fn js_insert_rule(s: u32, p: usize, l: usize, i: u32) -> u32 =
        "(s, p, l, i) => G.get(s).insertRule(G.str(p, l), i) >>> 0";
    #[catch]
    fn js_delete_rule(s: u32, i: u32) = "(s, i) => { G.get(s).deleteRule(i); }";

    // ---- ResizeObserver ------------------------------------------------------
    #[catch]
    fn js_resize_observer(f: u32) -> u32 = "(f) => G.add(new ResizeObserver(G.get(f)))";
    fn js_observe(o: u32, e: u32, op: u32) =
        "(o, e, op) => { const x = G.get(o); if (op === 0) x.observe(G.get(e)); \
           else if (op === 1) x.unobserve(G.get(e)); else x.disconnect(); }";

    // ---- Blob / URL -------------------------------------------------------------
    #[catch]
    fn js_blob_new(parts: u32, tp: usize, tl: usize) -> u32 =
        "(a, p, l) => G.add(new Blob(G.get(a), { type: G.str(p, l) }))";
    #[catch]
    fn js_object_url(b: u32, out: usize) = "(b, o) => G.retStr(URL.createObjectURL(G.get(b)), o)";
    fn js_revoke_url(p: usize, l: usize) = "(p, l) => { URL.revokeObjectURL(G.str(p, l)); }";

    // ---- events -----------------------------------------------------------------
    #[catch]
    fn js_new_event(cp: usize, cl: usize, tp: usize, tl: usize, init: u32) -> u32 =
        "(cp, cl, tp, tl, i) => G.add(new globalThis[G.str(cp, cl)](G.str(tp, tl), G.get(i)))";
}

// ---- plain property access ----------------------------------------------
//
// Each accessor is its OWN import with the property name baked into its
// JS (`(o) => G.add(G.get(o)["cssRules"])`), the shape wasm-bindgen
// emits as `__wbg_cssRules_<hash>`. They were one generic import per
// kind taking the name as `(ptr, len)`, which cost a `TextDecoder`
// decode of the name on EVERY access: measured on the benchmark's theme
// toggle (Chrome 154, 30 k toggles, CPU profile), the five name decodes
// (`sheet`, `cssRules`, `style` ×2, `documentElement`) were ~0.8 µs of
// a ~7.7 µs toggle, which was the whole gap to the pre-port web-sys
// build (~7.1 µs; boundary crossings were 31 against web-sys's 34).
//
// Same-JS, same-signature imports from different call sites get the
// same import name, so LLD merges them; an accessor nothing calls is
// dropped with its JS. The key must be a literal: `stringify!` of it is
// the (quoted) JS property name.

macro_rules! prop_num {
    ($o:expr, $k:literal) => {{
        crate::import! { fn get(o: u32) -> f64 = concat!("(o) => +G.get(o)[", stringify!($k), "]"); }
        unsafe { get(h($o)) }
    }};
}
macro_rules! prop_bool {
    ($o:expr, $k:literal) => {{
        crate::import! { fn get(o: u32) -> u32 = concat!("(o) => G.get(o)[", stringify!($k), "] ? 1 : 0"); }
        unsafe { get(h($o)) != 0 }
    }};
}
macro_rules! prop_str_opt {
    ($o:expr, $k:literal) => {{
        crate::import! {
            fn get(o: u32, out: usize) -> u32 = concat!(
                "(o, r) => { const v = G.get(o)[", stringify!($k),
                "]; if (v == null) return 0; G.retStr(String(v), r); return 1; }"
            );
        }
        let mut has = 0;
        let s = string::receive(|out| has = unsafe { get(h($o), out) });
        (has != 0).then_some(s)
    }};
}
macro_rules! prop_str {
    ($o:expr, $k:literal) => {
        prop_str_opt!($o, $k).unwrap_or_default()
    };
}
/// `Option<T>`: `null` / `undefined` are `None`; `T` is inferred.
macro_rules! prop_obj {
    ($o:expr, $k:literal) => {{
        crate::import! { fn get(o: u32) -> u32 = concat!("(o) => G.add(G.get(o)[", stringify!($k), "])"); }
        opt(unsafe { get(h($o)) })
    }};
}
macro_rules! set_num {
    ($o:expr, $k:literal, $v:expr) => {{
        crate::import! { fn set(o: u32, n: f64) = concat!("(o, n) => { G.get(o)[", stringify!($k), "] = n; }"); }
        let v: f64 = $v;
        unsafe { set(h($o), v) }
    }};
}
macro_rules! set_bool {
    ($o:expr, $k:literal, $v:expr) => {{
        crate::import! { fn set(o: u32, b: u32) = concat!("(o, b) => { G.get(o)[", stringify!($k), "] = b !== 0; }"); }
        let v: bool = $v;
        unsafe { set(h($o), v as u32) }
    }};
}
macro_rules! set_str {
    ($o:expr, $k:literal, $v:expr) => {{
        crate::import! {
            fn set(o: u32, p: usize, l: usize) =
                concat!("(o, p, l) => { G.get(o)[", stringify!($k), "] = G.str(p, l); }");
        }
        let v: &str = $v;
        let (p, l) = string::abi(v);
        unsafe { set(h($o), p, l) }
    }};
}
macro_rules! set_obj {
    ($o:expr, $k:literal, $v:expr) => {{
        crate::import! { fn set(o: u32, v: u32) = concat!("(o, v) => { G.get(o)[", stringify!($k), "] = G.get(v); }"); }
        // Inline, not a `let`: the argument may borrow a temporary.
        unsafe { set(h($o), JsValue::raw($v)) }
    }};
}
/// `o[name]()`; a throw is the `Err`.
macro_rules! call0 {
    ($o:expr, $k:literal) => {{
        crate::import! {
            #[catch]
            fn call(o: u32) -> u32 = concat!("(o) => G.add(G.get(o)[", stringify!($k), "]())");
        }
        unsafe { call(h($o)) }.map(|i| unsafe { JsValue::from_raw(i) })
    }};
}

/// Declares plain property accessors: `getter: kind = "jsName"` and
/// `setter <= kind = "jsName"`.
macro_rules! props {
    ($ty:ty { $($body:tt)* }) => { impl $ty { props!(@items $($body)*); } };
    (@items) => {};
    (@items $name:ident : i32 = $js:literal ; $($rest:tt)*) => {
        pub fn $name(&self) -> i32 { prop_num!(self, $js) as i32 }
        props!(@items $($rest)*);
    };
    (@items $name:ident : u32 = $js:literal ; $($rest:tt)*) => {
        pub fn $name(&self) -> u32 { prop_num!(self, $js) as u32 }
        props!(@items $($rest)*);
    };
    (@items $name:ident : f64 = $js:literal ; $($rest:tt)*) => {
        pub fn $name(&self) -> f64 { prop_num!(self, $js) }
        props!(@items $($rest)*);
    };
    (@items $name:ident : bool = $js:literal ; $($rest:tt)*) => {
        pub fn $name(&self) -> bool { prop_bool!(self, $js) }
        props!(@items $($rest)*);
    };
    (@items $name:ident : String = $js:literal ; $($rest:tt)*) => {
        pub fn $name(&self) -> String { prop_str!(self, $js) }
        props!(@items $($rest)*);
    };
    (@items $name:ident : Option<String> = $js:literal ; $($rest:tt)*) => {
        pub fn $name(&self) -> Option<String> { prop_str_opt!(self, $js) }
        props!(@items $($rest)*);
    };
    (@items $name:ident : Option<$t:ident> = $js:literal ; $($rest:tt)*) => {
        pub fn $name(&self) -> Option<$t> { prop_obj!(self, $js) }
        props!(@items $($rest)*);
    };
    (@items $name:ident : obj $t:ident = $js:literal ; $($rest:tt)*) => {
        pub fn $name(&self) -> $t { prop_obj!(self, $js).expect(concat!($js, " is null")) }
        props!(@items $($rest)*);
    };
    (@items $name:ident <= i32 = $js:literal ; $($rest:tt)*) => {
        pub fn $name(&self, v: i32) { set_num!(self, $js, v as f64) }
        props!(@items $($rest)*);
    };
    (@items $name:ident <= u32 = $js:literal ; $($rest:tt)*) => {
        pub fn $name(&self, v: u32) { set_num!(self, $js, v as f64) }
        props!(@items $($rest)*);
    };
    (@items $name:ident <= f64 = $js:literal ; $($rest:tt)*) => {
        pub fn $name(&self, v: f64) { set_num!(self, $js, v) }
        props!(@items $($rest)*);
    };
    (@items $name:ident <= bool = $js:literal ; $($rest:tt)*) => {
        pub fn $name(&self, v: bool) { set_bool!(self, $js, v) }
        props!(@items $($rest)*);
    };
    (@items $name:ident <= &str = $js:literal ; $($rest:tt)*) => {
        pub fn $name(&self, v: &str) { set_str!(self, $js, v) }
        props!(@items $($rest)*);
    };
}

// ---- Window ----------------------------------------------------------------

props!(Window {
    device_pixel_ratio: f64 = "devicePixelRatio";
    scroll_x_f64: f64 = "scrollX";
    scroll_y_f64: f64 = "scrollY";
});

impl Window {
    pub fn document(&self) -> Option<Document> {
        opt(unsafe { js_document(h(self)) })
    }
    /// `innerWidth` (web-sys shape: a number in a `JsValue`).
    pub fn inner_width(&self) -> Result<JsValue, JsError> {
        Ok(JsValue::from_f64(prop_num!(self, "innerWidth")))
    }
    pub fn inner_height(&self) -> Result<JsValue, JsError> {
        Ok(JsValue::from_f64(prop_num!(self, "innerHeight")))
    }
    pub fn history(&self) -> Result<History, JsError> {
        Ok(prop_obj!(self, "history").expect("window.history"))
    }
    pub fn location(&self) -> Location {
        prop_obj!(self, "location").expect("window.location")
    }
    pub fn navigator(&self) -> Navigator {
        prop_obj!(self, "navigator").expect("window.navigator")
    }
    pub fn performance(&self) -> Option<Performance> {
        prop_obj!(self, "performance")
    }
    /// `setTimeout(f, ms)` with a JS function (web-sys shape).
    pub fn set_timeout_with_callback_and_timeout_and_arguments_0(
        &self,
        f: &Function,
        ms: i32,
    ) -> Result<i32, JsError> {
        Ok(call(self, "setTimeout", &[f.as_js(), &ms.into()])?.as_f64().unwrap_or(0.0) as i32)
    }
    pub fn get_computed_style(&self, el: &Element) -> Result<Option<CssStyleDeclaration>, JsError> {
        unsafe { js_computed_style(h(self), h(el)) }.map(opt)
    }
    pub fn open_with_url_and_target_and_features(
        &self,
        url: &str,
        target: &str,
        features: &str,
    ) -> Result<Option<Window>, JsError> {
        let (up, ul) = string::abi(url);
        let (tp, tl) = string::abi(target);
        let (fp, fl) = string::abi(features);
        unsafe { js_window_open(h(self), up, ul, tp, tl, fp, fl) }.map(opt)
    }
    pub fn open_with_url_and_target(&self, url: &str, target: &str) -> Result<Option<Window>, JsError> {
        self.open_with_url_and_target_and_features(url, target, "")
    }
    pub fn scroll_x(&self) -> Result<f64, JsError> {
        Ok(self.scroll_x_f64())
    }
    pub fn scroll_y(&self) -> Result<f64, JsError> {
        Ok(self.scroll_y_f64())
    }
}

// ---- Document ----------------------------------------------------------------

props!(Document {
    body: Option<HtmlElement> = "body";
    head: Option<HtmlHeadElement> = "head";
    default_view: Option<Window> = "defaultView";
    document_element: Option<Element> = "documentElement";
    active_element: Option<Element> = "activeElement";
    fonts: obj FontFaceSet = "fonts";
    title: String = "title";
    set_title <= &str = "title";
    ready_state: String = "readyState";
});

impl Document {
    pub fn exit_fullscreen(&self) {
        let _ = call0!(self, "exitFullscreen");
    }
    pub fn fullscreen_element(&self) -> Option<Element> {
        prop_obj!(self, "fullscreenElement")
    }
    pub fn create_element(&self, tag: &str) -> Result<Element, JsError> {
        let (p, l) = string::abi(tag);
        unsafe { js_create_element(h(self), p, l) }.map(owned)
    }
    pub fn create_element_ns(&self, ns: Option<&str>, tag: &str) -> Result<Element, JsError> {
        let (np, nl) = string::abi(ns.unwrap_or(""));
        let (p, l) = string::abi(tag);
        unsafe { js_create_element_ns(h(self), np, nl, ns.is_some() as u32, p, l) }.map(owned)
    }
    pub fn create_text_node(&self, text: &str) -> Text {
        let (p, l) = string::abi(text);
        owned(unsafe { js_create_text_node(h(self), p, l) })
    }
    pub fn create_document_fragment(&self) -> DocumentFragment {
        owned(unsafe { js_create_fragment(h(self)) })
    }
    pub fn element_from_point(&self, x: f32, y: f32) -> Option<Element> {
        let r = call(self, "elementFromPoint", &[&x.into(), &y.into()]).ok()?;
        (!r.is_null() && !r.is_undefined()).then(|| r.unchecked_into())
    }
    pub fn get_element_by_id(&self, id: &str) -> Option<Element> {
        let (p, l) = string::abi(id);
        opt(unsafe { js_get_element_by_id(h(self), p, l) })
    }
    pub fn query_selector(&self, sel: &str) -> Result<Option<Element>, JsError> {
        let (p, l) = string::abi(sel);
        unsafe { js_query_selector(h(self), p, l) }.map(opt)
    }
    pub fn query_selector_all(&self, sel: &str) -> Result<NodeList, JsError> {
        let (p, l) = string::abi(sel);
        unsafe { js_query_selector_all(h(self), p, l) }.map(owned)
    }
}

// ---- EventTarget / Node ---------------------------------------------------------

impl EventTarget {
    pub fn dispatch_event(&self, ev: &Event) -> Result<bool, JsError> {
        unsafe { js_dispatch(h(self), h(ev)) }.map(|r| r != 0)
    }
}

props!(Node {
    parent_node: Option<Node> = "parentNode";
    parent_element: Option<Element> = "parentElement";
    first_child: Option<Node> = "firstChild";
    last_child: Option<Node> = "lastChild";
    next_sibling: Option<Node> = "nextSibling";
    previous_sibling: Option<Node> = "previousSibling";
    owner_document: Option<Document> = "ownerDocument";
    child_nodes: obj NodeList = "childNodes";
    node_name: String = "nodeName";
    node_type: u32 = "nodeType";
    is_connected: bool = "isConnected";
    text_content: Option<String> = "textContent";
    node_value: Option<String> = "nodeValue";
});

impl Node {
    pub fn append_child(&self, child: &Node) -> Result<Node, JsError> {
        unsafe { js_append_child(h(self), h(child)) }.map(owned)
    }
    pub fn insert_before(&self, child: &Node, reference: Option<&Node>) -> Result<Node, JsError> {
        let r = reference.map_or(1, |r| h(r)); // slot 1 is null
        unsafe { js_insert_before(h(self), h(child), r) }.map(owned)
    }
    pub fn remove_child(&self, child: &Node) -> Result<Node, JsError> {
        unsafe { js_remove_child(h(self), h(child)) }.map(owned)
    }
    pub fn replace_child(&self, new: &Node, old: &Node) -> Result<Node, JsError> {
        unsafe { js_replace_child(h(self), h(new), h(old)) }.map(owned)
    }
    pub fn set_text_content(&self, text: Option<&str>) {
        let (p, l) = string::abi(text.unwrap_or(""));
        unsafe { js_set_text_content(h(self), text.is_some() as u32, p, l) }
    }
    pub fn set_node_value(&self, v: Option<&str>) {
        match v {
            Some(v) => set_str!(self, "nodeValue", v),
            None => set_obj!(self, "nodeValue", &JsValue::NULL),
        }
    }
    /// `this.contains(other)`; `None` is `false`, as in the DOM.
    pub fn contains(&self, other: Option<&Node>) -> bool {
        other.is_some_and(|o| unsafe { js_contains(h(self), h(o)) != 0 })
    }
    /// Identity (`===`).
    pub fn is_same_node(&self, other: Option<&Node>) -> bool {
        other.is_some_and(|o| unsafe { js_same(h(self), h(o)) != 0 })
    }
    pub fn clone_node_with_deep(&self, deep: bool) -> Result<Node, JsError> {
        unsafe { js_clone_node(h(self), deep as u32) }.map(owned)
    }
    pub fn clone_node(&self) -> Result<Node, JsError> {
        self.clone_node_with_deep(false)
    }
}

props!(NodeList { length: u32 = "length"; });
impl NodeList {
    pub fn item(&self, i: u32) -> Option<Node> {
        opt(unsafe { js_list_item(h(self), i) })
    }
    pub fn get(&self, i: u32) -> Option<Node> {
        self.item(i)
    }
}

props!(HtmlCollection { length: u32 = "length"; });
impl HtmlCollection {
    pub fn item(&self, i: u32) -> Option<Element> {
        opt(unsafe { js_list_item(h(self), i) })
    }
    pub fn get_with_index(&self, i: u32) -> Option<Element> {
        self.item(i)
    }
}

impl Text {
    pub fn data(&self) -> String {
        prop_str!(self, "data")
    }
    pub fn set_data(&self, v: &str) {
        set_str!(self, "data", v)
    }
}

// ---- Element -----------------------------------------------------------------------

props!(Element {
    id: String = "id";
    set_id <= &str = "id";
    set_class_name <= &str = "className";
    local_name: String = "localName";
    inner_html: String = "innerHTML";
    set_inner_html <= &str = "innerHTML";
    outer_html: String = "outerHTML";
    first_element_child: Option<Element> = "firstElementChild";
    last_element_child: Option<Element> = "lastElementChild";
    next_element_sibling: Option<Element> = "nextElementSibling";
    previous_element_sibling: Option<Element> = "previousElementSibling";
    children: obj HtmlCollection = "children";
    child_element_count: u32 = "childElementCount";
    class_list: obj DomTokenList = "classList";
    scroll_top: i32 = "scrollTop";
    set_scroll_top <= i32 = "scrollTop";
    scroll_left: i32 = "scrollLeft";
    set_scroll_left <= i32 = "scrollLeft";
    scroll_width: i32 = "scrollWidth";
    scroll_height: i32 = "scrollHeight";
    client_width: i32 = "clientWidth";
    client_height: i32 = "clientHeight";
    client_top: i32 = "clientTop";
    client_left: i32 = "clientLeft";
});

impl Element {
    pub fn request_fullscreen(&self) -> Result<(), JsError> {
        call0!(self, "requestFullscreen").map(drop)
    }
    pub fn set_attribute(&self, name: &str, value: &str) -> Result<(), JsError> {
        let (np, nl) = string::abi(name);
        let (vp, vl) = string::abi(value);
        unsafe { js_set_attribute(h(self), np, nl, vp, vl) }
    }
    pub fn remove_attribute(&self, name: &str) -> Result<(), JsError> {
        let (p, l) = string::abi(name);
        unsafe { js_remove_attribute(h(self), p, l) }
    }
    pub fn has_attribute(&self, name: &str) -> bool {
        let (p, l) = string::abi(name);
        unsafe { js_has_attribute(h(self), p, l) != 0 }
    }
    pub fn get_attribute(&self, name: &str) -> Option<String> {
        let (p, l) = string::abi(name);
        let mut has = 0;
        let s = string::receive(|o| has = unsafe { js_get_attribute(h(self), p, l, o) });
        (has != 0).then_some(s)
    }
    pub fn query_selector(&self, sel: &str) -> Result<Option<Element>, JsError> {
        let (p, l) = string::abi(sel);
        unsafe { js_query_selector(h(self), p, l) }.map(opt)
    }
    pub fn query_selector_all(&self, sel: &str) -> Result<NodeList, JsError> {
        let (p, l) = string::abi(sel);
        unsafe { js_query_selector_all(h(self), p, l) }.map(owned)
    }
    /// `closest(selector)` — `Err` for an invalid selector.
    pub fn closest(&self, sel: &str) -> Result<Option<Element>, JsError> {
        let (p, l) = string::abi(sel);
        unsafe { js_closest(h(self), p, l) }.map(opt)
    }
    pub fn matches(&self, sel: &str) -> Result<bool, JsError> {
        let (p, l) = string::abi(sel);
        unsafe { js_matches(h(self), p, l) }.map(|r| r != 0)
    }
    pub fn tag_name(&self) -> String {
        prop_str!(self, "tagName")
    }
    /// The `class` attribute (an SVG element's `className.baseVal`).
    pub fn class_name(&self) -> String {
        self.get_attribute("class").unwrap_or_default()
    }
    pub fn get_bounding_client_rect(&self) -> DomRect {
        owned(unsafe { js_rect(h(self)) })
    }
    pub fn set_pointer_capture(&self, id: i32) -> Result<(), JsError> {
        unsafe { js_pointer_capture(h(self), id, 1) }
    }
    pub fn release_pointer_capture(&self, id: i32) -> Result<(), JsError> {
        unsafe { js_pointer_capture(h(self), id, 0) }
    }
    pub fn has_pointer_capture(&self, id: i32) -> bool {
        unsafe { js_has_pointer_capture(h(self), id) != 0 }
    }
    /// `remove()` — detach from the parent.
    pub fn remove(&self) {
        unsafe { js_remove(h(self)) }
    }
}

props!(DomTokenList { length: u32 = "length"; value: String = "value"; });
impl DomTokenList {
    fn op(&self, token: &str, op: u32) -> Result<bool, JsError> {
        let (p, l) = string::abi(token);
        unsafe { js_token(h(self), p, l, op) }.map(|r| r != 0)
    }
    pub fn add_1(&self, t: &str) -> Result<(), JsError> {
        self.op(t, 0).map(drop)
    }
    pub fn remove_1(&self, t: &str) -> Result<(), JsError> {
        self.op(t, 1).map(drop)
    }
    pub fn contains(&self, t: &str) -> bool {
        self.op(t, 2).unwrap_or(false)
    }
    pub fn toggle(&self, t: &str) -> Result<bool, JsError> {
        self.op(t, 3)
    }
}

props!(DomRectReadOnly {
    x: f64 = "x";
    y: f64 = "y";
    width: f64 = "width";
    height: f64 = "height";
    left: f64 = "left";
    top: f64 = "top";
    right: f64 = "right";
    bottom: f64 = "bottom";
});

// ---- HTMLElement and friends ------------------------------------------------------------

props!(HtmlElement {
    style: obj CssStyleDeclaration = "style";
    offset_left: i32 = "offsetLeft";
    offset_top: i32 = "offsetTop";
    offset_width: i32 = "offsetWidth";
    offset_height: i32 = "offsetHeight";
    offset_parent: Option<Element> = "offsetParent";
    is_content_editable: bool = "isContentEditable";
    tab_index: i32 = "tabIndex";
    set_tab_index <= i32 = "tabIndex";
    hidden: bool = "hidden";
    set_hidden <= bool = "hidden";
    title: String = "title";
    set_title <= &str = "title";
    inner_text: String = "innerText";
    set_inner_text <= &str = "innerText";
    set_draggable <= bool = "draggable";
    dir: String = "dir";
    set_dir <= &str = "dir";
});

impl HtmlElement {
    pub fn focus(&self) -> Result<(), JsError> {
        unsafe { js_focus(h(self)) }
    }
    /// `focus({ preventScroll: true })`.
    pub fn focus_prevent_scroll(&self) -> Result<(), JsError> {
        unsafe { js_focus_prevent_scroll(h(self)) }
    }
    pub fn blur(&self) -> Result<(), JsError> {
        unsafe { js_blur(h(self)) }
    }
    pub fn click(&self) {
        unsafe { js_click(h(self)) }
    }
}

props!(SvgElement { style: obj CssStyleDeclaration = "style"; });

/// Shared by `<input>` and `<textarea>`.
macro_rules! text_control {
    ($ty:ty) => {
        props!($ty {
            value: String = "value";
            set_value <= &str = "value";
            disabled: bool = "disabled";
            set_disabled <= bool = "disabled";
            read_only: bool = "readOnly";
            set_read_only <= bool = "readOnly";
            placeholder: String = "placeholder";
            set_placeholder <= &str = "placeholder";
            set_autocomplete <= &str = "autocomplete";
            set_max_length <= i32 = "maxLength";
            set_spellcheck <= bool = "spellcheck";
            set_input_mode <= &str = "inputMode";
            name: String = "name";
            set_name <= &str = "name";
        });
        impl $ty {
            pub fn select(&self) {
                unsafe { js_select(h(self)) }
            }
            pub fn selection_start(&self) -> Result<Option<u32>, JsError> {
                let v = unsafe { js_selection(h(self), 0) };
                Ok((v >= 0.0).then_some(v as u32))
            }
            pub fn selection_end(&self) -> Result<Option<u32>, JsError> {
                let v = unsafe { js_selection(h(self), 1) };
                Ok((v >= 0.0).then_some(v as u32))
            }
            pub fn set_selection_range(&self, start: u32, end: u32) -> Result<(), JsError> {
                unsafe { js_set_selection_range(h(self), start, end) }
            }
            pub fn set_range_text_with_start_and_end(&self, text: &str, start: u32, end: u32) -> Result<(), JsError> {
                let (p, l) = string::abi(text);
                unsafe { js_set_range_text(h(self), p, l, start, end) }
            }
        }
    };
}
text_control!(HtmlInputElement);
text_control!(HtmlTextAreaElement);

props!(HtmlInputElement {
    type_: String = "type";
    set_type <= &str = "type";
    checked: bool = "checked";
    set_checked <= bool = "checked";
    min: String = "min";
    set_min <= &str = "min";
    max: String = "max";
    set_max <= &str = "max";
    step: String = "step";
    set_step <= &str = "step";
});

props!(HtmlTextAreaElement {
    rows: u32 = "rows";
    set_rows <= u32 = "rows";
    wrap: String = "wrap";
    set_wrap <= &str = "wrap";
});

props!(HtmlStyleElement { sheet: Option<StyleSheet> = "sheet"; });

props!(HtmlCanvasElement {
    width: u32 = "width";
    set_width <= u32 = "width";
    height: u32 = "height";
    set_height <= u32 = "height";
});

impl HtmlCanvasElement {
    /// `getContext(kind)`.
    pub fn get_context(&self, kind: &str) -> Result<Option<Object>, JsError> {
        let f: JsValue = prop_obj!(self, "getContext").unwrap_or_default();
        let r = f.call(self.as_js(), &[&JsValue::from_str(kind)])?;
        Ok((!r.is_null() && !r.is_undefined()).then(|| r.unchecked_into()))
    }
}

props!(HtmlAnchorElement {
    href: String = "href";
    set_href <= &str = "href";
    target: String = "target";
    set_target <= &str = "target";
    set_rel <= &str = "rel";
    set_download <= &str = "download";
});

props!(HtmlImageElement {
    src: String = "src";
    set_src <= &str = "src";
    set_alt <= &str = "alt";
    set_decoding <= &str = "decoding";
    set_loading <= &str = "loading";
    set_cross_origin <= &str = "crossOrigin";
    complete: bool = "complete";
    natural_width: u32 = "naturalWidth";
    natural_height: u32 = "naturalHeight";
});

props!(HtmlIFrameElement {
    src: String = "src";
    set_src <= &str = "src";
    content_window: Option<Window> = "contentWindow";
});

props!(HtmlOptionElement {
    selected: bool = "selected";
    set_selected <= bool = "selected";
});

// ---- CSSOM ----------------------------------------------------------------------------------

props!(CssStyleDeclaration {
    css_text: String = "cssText";
    set_css_text <= &str = "cssText";
    length: u32 = "length";
});

impl CssStyleDeclaration {
    pub fn set_property(&self, name: &str, value: &str) -> Result<(), JsError> {
        let (np, nl) = string::abi(name);
        let (vp, vl) = string::abi(value);
        unsafe { js_set_property(h(self), np, nl, vp, vl) }
    }
    pub fn set_property_with_priority(&self, name: &str, value: &str, prio: &str) -> Result<(), JsError> {
        let (np, nl) = string::abi(name);
        let (vp, vl) = string::abi(value);
        let (pp, pl) = string::abi(prio);
        unsafe { js_set_property_prio(h(self), np, nl, vp, vl, pp, pl) }
    }
    pub fn remove_property(&self, name: &str) -> Result<String, JsError> {
        let (p, l) = string::abi(name);
        let mut r = Ok(());
        let s = string::receive(|o| r = unsafe { js_remove_property(h(self), p, l, o) });
        r.map(|()| s)
    }
    pub fn get_property_value(&self, name: &str) -> Result<String, JsError> {
        let (p, l) = string::abi(name);
        let mut r = Ok(());
        let s = string::receive(|o| r = unsafe { js_get_property_value(h(self), p, l, o) });
        r.map(|()| s)
    }
    pub fn item(&self, i: u32) -> String {
        call_item_str(self, i)
    }
}

fn call_item_str(o: &impl JsCast, i: u32) -> String {
    let v = unsafe { JsValue::from_raw(js_list_item(h(o), i)) };
    v.as_string().unwrap_or_default()
}

impl StyleSheet {
    /// `sheet` is a `CSSStyleSheet` for every `<style>` the framework makes.
    pub fn css_style_sheet(&self) -> CssStyleSheet {
        self.clone().unchecked_into()
    }
}

impl CssStyleSheet {
    pub fn css_rules(&self) -> Result<CssRuleList, JsError> {
        Ok(prop_obj!(self, "cssRules").expect("cssRules"))
    }
    pub fn insert_rule_with_index(&self, rule: &str, index: u32) -> Result<u32, JsError> {
        let (p, l) = string::abi(rule);
        unsafe { js_insert_rule(h(self), p, l, index) }
    }
    pub fn insert_rule(&self, rule: &str) -> Result<u32, JsError> {
        self.insert_rule_with_index(rule, 0)
    }
    pub fn delete_rule(&self, index: u32) -> Result<(), JsError> {
        unsafe { js_delete_rule(h(self), index) }
    }
}

props!(CssRuleList { length: u32 = "length"; });
impl CssRuleList {
    pub fn item(&self, i: u32) -> Option<CssRule> {
        opt(unsafe { js_list_item(h(self), i) })
    }
    pub fn get(&self, i: u32) -> Option<CssRule> {
        self.item(i)
    }
}

props!(CssRule { css_text: String = "cssText"; type_: u32 = "type"; });
props!(CssStyleRule { selector_text: String = "selectorText"; style: obj CssStyleDeclaration = "style"; });
props!(CssMediaRule { css_rules: obj CssRuleList = "cssRules"; condition_text: String = "conditionText"; });

// ---- History / Location / Navigator --------------------------------------------------------

impl History {
    pub fn push_state_with_url(&self, state: &JsValue, _title: &str, url: Option<&str>) -> Result<(), JsError> {
        let u = url.unwrap_or_else(|| "");
        let (p, l) = string::abi(u);
        unsafe { js_history_state(h(self), state.raw(), p, l, 0) }
    }
    pub fn replace_state_with_url(&self, state: &JsValue, _title: &str, url: Option<&str>) -> Result<(), JsError> {
        let (p, l) = string::abi(url.unwrap_or(""));
        unsafe { js_history_state(h(self), state.raw(), p, l, 1) }
    }
    pub fn back(&self) -> Result<(), JsError> {
        call0!(self, "back").map(drop)
    }
    pub fn forward(&self) -> Result<(), JsError> {
        call0!(self, "forward").map(drop)
    }
    pub fn state(&self) -> Result<JsValue, JsError> {
        Ok(prop_obj!(self, "state").unwrap_or(JsValue::NULL))
    }
    pub fn length(&self) -> Result<u32, JsError> {
        Ok(prop_num!(self, "length") as u32)
    }
}

impl Location {
    pub fn pathname(&self) -> Result<String, JsError> {
        Ok(prop_str!(self, "pathname"))
    }
    pub fn search(&self) -> Result<String, JsError> {
        Ok(prop_str!(self, "search"))
    }
    pub fn hash(&self) -> Result<String, JsError> {
        Ok(prop_str!(self, "hash"))
    }
    pub fn href(&self) -> Result<String, JsError> {
        Ok(prop_str!(self, "href"))
    }
    pub fn origin(&self) -> Result<String, JsError> {
        Ok(prop_str!(self, "origin"))
    }
    pub fn host(&self) -> Result<String, JsError> {
        Ok(prop_str!(self, "host"))
    }
    pub fn protocol(&self) -> Result<String, JsError> {
        Ok(prop_str!(self, "protocol"))
    }
    pub fn set_href(&self, v: &str) -> Result<(), JsError> {
        set_str!(self, "href", v);
        Ok(())
    }
    pub fn reload(&self) -> Result<(), JsError> {
        call0!(self, "reload").map(drop)
    }
}

impl Navigator {
    pub fn user_agent(&self) -> Result<String, JsError> {
        Ok(prop_str!(self, "userAgent"))
    }
    pub fn platform(&self) -> Result<String, JsError> {
        Ok(prop_str!(self, "platform"))
    }
    pub fn language(&self) -> Option<String> {
        prop_str_opt!(self, "language")
    }
}

// ---- ResizeObserver -------------------------------------------------------------------------

impl ResizeObserver {
    /// `new ResizeObserver(callback)`; `callback` receives the entries
    /// array (and the observer, which glue closures ignore).
    pub fn new(callback: &Function) -> Result<ResizeObserver, JsError> {
        unsafe { js_resize_observer(h(callback)) }.map(owned)
    }
    pub fn observe(&self, target: &Element) {
        unsafe { js_observe(h(self), h(target), 0) }
    }
    pub fn unobserve(&self, target: &Element) {
        unsafe { js_observe(h(self), h(target), 1) }
    }
    pub fn disconnect(&self) {
        unsafe { js_observe(h(self), 0, 2) }
    }
}

props!(ResizeObserverEntry {
    content_rect: obj DomRectReadOnly = "contentRect";
    target: obj Element = "target";
});

// ---- fonts ---------------------------------------------------------------------------------

props!(FontFace { family: String = "family"; status: String = "status"; });
impl FontFace {
    pub fn load(&self) -> Result<crate::js::Promise, JsError> {
        call0!(self, "load").map(JsCast::unchecked_into)
    }
}
impl FontFaceSet {
    pub fn add(&self, face: &FontFace) -> Result<(), JsError> {
        self.as_js().call_method("add", &[face.as_js()]).map(drop)
    }
    pub fn ready(&self) -> Result<crate::js::Promise, JsError> {
        Ok(prop_obj!(self, "ready").expect("fonts.ready"))
    }
    pub fn values(&self) -> JsValue {
        self.as_js().call_method("values", &[]).unwrap_or_default()
    }
}

// ---- Blob / URL -------------------------------------------------------------------------------

impl Blob {
    /// `new Blob(parts, { type })`.
    pub fn new_with_parts_and_type(parts: &Array, mime: &str) -> Result<Blob, JsError> {
        let (p, l) = string::abi(mime);
        unsafe { js_blob_new(h(parts), p, l) }.map(owned)
    }
}

/// `URL` static functions.
pub struct Url;
impl Url {
    pub fn create_object_url_with_blob(blob: &Blob) -> Result<String, JsError> {
        let mut r = Ok(());
        let s = string::receive(|o| r = unsafe { js_object_url(h(blob), o) });
        r.map(|()| s)
    }
    pub fn revoke_object_url(url: &str) -> Result<(), JsError> {
        let (p, l) = string::abi(url);
        unsafe { js_revoke_url(p, l) };
        Ok(())
    }
}

// ---- MediaStream / MediaStreamTrack ------------------------------------------------------

props!(MediaStream {
    id: String = "id";
    active: bool = "active";
});

impl MediaStream {
    /// `new MediaStream()` — an empty stream.
    pub fn new() -> Result<MediaStream, JsError> {
        construct("MediaStream")
    }
    /// `new MediaStream(tracks)`.
    pub fn new_with_tracks(tracks: &Array) -> Result<MediaStream, JsError> {
        let ctor = JsValue::global().get("MediaStream")?;
        Ok(ctor.construct(&[tracks.as_js()])?.unchecked_into())
    }
    /// `getTracks()` — every track, audio and video.
    pub fn get_tracks(&self) -> Array {
        call(self, "getTracks", &[]).map(JsCast::unchecked_into).unwrap_or_default()
    }
    /// `getVideoTracks()`.
    pub fn get_video_tracks(&self) -> Array {
        call(self, "getVideoTracks", &[]).map(JsCast::unchecked_into).unwrap_or_default()
    }
    /// `getAudioTracks()`.
    pub fn get_audio_tracks(&self) -> Array {
        call(self, "getAudioTracks", &[]).map(JsCast::unchecked_into).unwrap_or_default()
    }
    /// `addTrack(track)`.
    pub fn add_track(&self, track: &MediaStreamTrack) {
        let _ = call(self, "addTrack", &[track.as_js()]);
    }
    /// `removeTrack(track)`.
    pub fn remove_track(&self, track: &MediaStreamTrack) {
        let _ = call(self, "removeTrack", &[track.as_js()]);
    }
}

/// `MediaStreamTrack.readyState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaStreamTrackState {
    /// `"live"` — the source is producing.
    Live,
    /// `"ended"` — `stop()` was called, or the source went away.
    Ended,
}

props!(MediaStreamTrack {
    id: String = "id";
    kind: String = "kind";
    label: String = "label";
    enabled: bool = "enabled";
    set_enabled <= bool = "enabled";
    muted: bool = "muted";
});

impl MediaStreamTrack {
    /// `stop()` — ends the track and releases its device.
    pub fn stop(&self) {
        let _ = call(self, "stop", &[]);
    }
    /// `readyState`.
    pub fn ready_state(&self) -> MediaStreamTrackState {
        if prop_str!(self, "readyState") == "live" {
            MediaStreamTrackState::Live
        } else {
            MediaStreamTrackState::Ended
        }
    }
}

// ---- WebSocket ---------------------------------------------------------------------------

crate::import! {
    #[catch]
    fn js_ws_new(p: usize, l: usize) -> u32 = "(p, l) => G.add(new WebSocket(G.str(p, l)))";
    #[catch]
    fn js_ws_send_bytes(w: u32, p: usize, l: usize) =
        "(w, p, l) => { G.get(w).send(G.u8().slice(p >>> 0, (p >>> 0) + (l >>> 0))); }";
    #[catch]
    fn js_ws_send_str(w: u32, p: usize, l: usize) = "(w, p, l) => { G.get(w).send(G.str(p, l)); }";
}

/// `WebSocket.binaryType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryType {
    Blob,
    Arraybuffer,
}

props!(WebSocket { ready_state_f64: f64 = "readyState"; buffered_amount: u32 = "bufferedAmount"; });

impl WebSocket {
    pub const CONNECTING: u16 = 0;
    pub const OPEN: u16 = 1;
    pub const CLOSING: u16 = 2;
    pub const CLOSED: u16 = 3;

    pub fn new(url: &str) -> Result<WebSocket, JsError> {
        let (p, l) = string::abi(url);
        unsafe { js_ws_new(p, l) }.map(owned)
    }
    pub fn set_binary_type(&self, t: BinaryType) {
        set_str!(self, "binaryType", if t == BinaryType::Arraybuffer { "arraybuffer" } else { "blob" })
    }
    pub fn ready_state(&self) -> u16 {
        self.ready_state_f64() as u16
    }
    /// Send a binary frame (a COPY of `bytes`).
    pub fn send_with_u8_array(&self, bytes: &[u8]) -> Result<(), JsError> {
        unsafe { js_ws_send_bytes(h(self), bytes.as_ptr() as usize, bytes.len()) }
    }
    pub fn send_with_str(&self, s: &str) -> Result<(), JsError> {
        let (p, l) = string::abi(s);
        unsafe { js_ws_send_str(h(self), p, l) }
    }
    pub fn close(&self) -> Result<(), JsError> {
        call0!(self, "close").map(drop)
    }
}

impl MessageEvent {
    pub fn data(&self) -> JsValue {
        prop_obj!(self, "data").unwrap_or_default()
    }
}

impl CloseEvent {
    pub fn code(&self) -> u16 {
        prop_num!(self, "code") as u16
    }
    pub fn reason(&self) -> String {
        prop_str!(self, "reason")
    }
    pub fn was_clean(&self) -> bool {
        prop_bool!(self, "wasClean")
    }
}

// ---- serialisation, canvas, XHR (the robot's DOM screenshot) ----------------------------

fn construct<T: JsCast>(class: &str) -> Result<T, JsError> {
    let ctor = JsValue::global().get(class)?;
    Ok(ctor.construct(&[])?.unchecked_into())
}

fn call(o: &impl JsCast, name: &str, args: &[&JsValue]) -> Result<JsValue, JsError> {
    o.as_ref().call_method(name, args)
}

impl Performance {
    pub fn now(&self) -> f64 {
        crate::dom::performance_now()
    }
}

impl XmlSerializer {
    pub fn new() -> Result<XmlSerializer, JsError> {
        construct("XMLSerializer")
    }
    pub fn serialize_to_string(&self, node: &Node) -> Result<String, JsError> {
        Ok(call(self, "serializeToString", &[node.as_js()])?.as_string().unwrap_or_default())
    }
}

impl HtmlImageElement {
    /// `new Image()`.
    pub fn new() -> Result<HtmlImageElement, JsError> {
        construct("Image")
    }
    pub fn set_onload(&self, f: Option<&Function>) {
        set_obj!(self, "onload", f.map_or(&JsValue::NULL, |f| f.as_js()))
    }
    pub fn set_onerror(&self, f: Option<&Function>) {
        set_obj!(self, "onerror", f.map_or(&JsValue::NULL, |f| f.as_js()))
    }
}

impl HtmlCanvasElement {
    pub fn to_data_url_with_type(&self, mime: &str) -> Result<String, JsError> {
        Ok(call(self, "toDataURL", &[&JsValue::from_str(mime)])?.as_string().unwrap_or_default())
    }
}

impl CanvasRenderingContext2d {
    pub fn scale(&self, x: f64, y: f64) -> Result<(), JsError> {
        call(self, "scale", &[&x.into(), &y.into()]).map(drop)
    }
    pub fn draw_image_with_html_image_element(&self, img: &HtmlImageElement, x: f64, y: f64) -> Result<(), JsError> {
        call(self, "drawImage", &[img.as_js(), &x.into(), &y.into()]).map(drop)
    }
}

impl XmlHttpRequest {
    pub fn new() -> Result<XmlHttpRequest, JsError> {
        construct("XMLHttpRequest")
    }
    pub fn open_with_async(&self, method: &str, url: &str, is_async: bool) -> Result<(), JsError> {
        call(self, "open", &[&method.into(), &url.into(), &is_async.into()]).map(drop)
    }
    pub fn override_mime_type(&self, mime: &str) -> Result<(), JsError> {
        call(self, "overrideMimeType", &[&mime.into()]).map(drop)
    }
    pub fn send(&self) -> Result<(), JsError> {
        call(self, "send", &[]).map(drop)
    }
    pub fn status(&self) -> Result<u16, JsError> {
        Ok(prop_num!(self, "status") as u16)
    }
    pub fn response_text(&self) -> Result<Option<String>, JsError> {
        Ok(prop_str_opt!(self, "responseText"))
    }
}

impl Window {
    pub fn btoa(&self, s: &str) -> Result<String, JsError> {
        Ok(call(self, "btoa", &[&s.into()])?.as_string().unwrap_or_default())
    }
}

// ---- constructing events (tests, synthetic dispatch) -----------------------------------------

/// `new <Class>(type, init)` — `class` names the global constructor
/// (`"Event"`, `"PointerEvent"`, …), `init` a plain options object.
pub fn new_event<T: JsCast>(class: &str, ty: &str, init: &Object) -> Result<T, JsError> {
    let (cp, cl) = string::abi(class);
    let (tp, tl) = string::abi(ty);
    unsafe { js_new_event(cp, cl, tp, tl, h(init)) }.map(owned)
}

/// Event-init dictionaries (`EventInit`, `MouseEventInit`, …): plain
/// objects with web-sys's setter names, for constructing synthetic events.
macro_rules! init_dict {
    ($name:ident { $($setter:ident : $t:ty = $js:literal),* $(,)? }) => {
        #[derive(Clone, Debug, Default)]
        pub struct $name(Object);
        impl $name {
            pub fn new() -> $name {
                $name(Object::new())
            }
            pub fn as_object(&self) -> &Object {
                &self.0
            }
            $(
                pub fn $setter(&self, v: $t) {
                    let _ = crate::js::Reflect::set(&self.0, &JsValue::from_str($js), &JsValue::from(v));
                }
            )*
        }
    };
}

init_dict!(EventInit { set_bubbles: bool = "bubbles", set_cancelable: bool = "cancelable", set_composed: bool = "composed" });
init_dict!(MouseEventInit {
    set_bubbles: bool = "bubbles", set_cancelable: bool = "cancelable", set_composed: bool = "composed",
    set_button: i16 = "button", set_buttons: u16 = "buttons",
    set_client_x: i32 = "clientX", set_client_y: i32 = "clientY",
    set_ctrl_key: bool = "ctrlKey", set_shift_key: bool = "shiftKey",
    set_alt_key: bool = "altKey", set_meta_key: bool = "metaKey",
});
init_dict!(PointerEventInit {
    set_bubbles: bool = "bubbles", set_cancelable: bool = "cancelable", set_composed: bool = "composed",
    set_button: i16 = "button", set_buttons: u16 = "buttons",
    set_client_x: i32 = "clientX", set_client_y: i32 = "clientY",
    set_ctrl_key: bool = "ctrlKey", set_shift_key: bool = "shiftKey",
    set_alt_key: bool = "altKey", set_meta_key: bool = "metaKey",
    set_pointer_id: i32 = "pointerId", set_pointer_type: &str = "pointerType",
    set_pressure: f32 = "pressure", set_is_primary: bool = "isPrimary",
});
init_dict!(KeyboardEventInit {
    set_bubbles: bool = "bubbles", set_cancelable: bool = "cancelable", set_composed: bool = "composed",
    set_key: &str = "key", set_code: &str = "code",
    set_ctrl_key: bool = "ctrlKey", set_shift_key: bool = "shiftKey",
    set_alt_key: bool = "altKey", set_meta_key: bool = "metaKey", set_repeat: bool = "repeat",
});

impl Event {
    /// `new Event(type)`.
    pub fn new(ty: &str) -> Result<Event, JsError> {
        new_event("Event", ty, &Object::new())
    }
    pub fn new_with_event_init_dict(ty: &str, init: &EventInit) -> Result<Event, JsError> {
        new_event("Event", ty, init.as_object())
    }
}
impl MouseEvent {
    pub fn new(ty: &str) -> Result<MouseEvent, JsError> {
        new_event("MouseEvent", ty, &Object::new())
    }
    pub fn new_with_mouse_event_init_dict(ty: &str, init: &MouseEventInit) -> Result<MouseEvent, JsError> {
        new_event("MouseEvent", ty, init.as_object())
    }
}
impl PointerEvent {
    pub fn new(ty: &str) -> Result<PointerEvent, JsError> {
        new_event("PointerEvent", ty, &Object::new())
    }
    pub fn new_with_event_init_dict(ty: &str, init: &PointerEventInit) -> Result<PointerEvent, JsError> {
        new_event("PointerEvent", ty, init.as_object())
    }
}
impl KeyboardEvent {
    pub fn new_with_keyboard_event_init_dict(ty: &str, init: &KeyboardEventInit) -> Result<KeyboardEvent, JsError> {
        new_event("KeyboardEvent", ty, init.as_object())
    }
}

impl Default for JsValue {
    fn default() -> JsValue {
        JsValue::UNDEFINED
    }
}

/// `console.*` — the web-sys `console::log_1`-style free functions.
pub mod console {
    use crate::{string, JsValue};

    crate::import! {
        fn js_console(level: u32, args: usize, n: usize) =
            "(v, a, n) => { const xs = G.args(a, n); \
               (v === 0 ? console.debug : v === 1 ? console.log : v === 2 ? console.info : \
                v === 3 ? console.warn : console.error)(...xs); }";
    }

    fn emit(level: u32, args: &[&JsValue]) {
        let raw: Vec<u32> = args.iter().map(|v| v.raw()).collect();
        unsafe { js_console(level, raw.as_ptr() as usize, raw.len()) }
    }

    pub fn debug_1(a: &JsValue) { emit(0, &[a]) }
    pub fn log_1(a: &JsValue) { emit(1, &[a]) }
    pub fn log_2(a: &JsValue, b: &JsValue) { emit(1, &[a, b]) }
    pub fn info_1(a: &JsValue) { emit(2, &[a]) }
    pub fn warn_1(a: &JsValue) { emit(3, &[a]) }
    pub fn warn_2(a: &JsValue, b: &JsValue) { emit(3, &[a, b]) }
    pub fn error_1(a: &JsValue) { emit(4, &[a]) }
    pub fn error_2(a: &JsValue, b: &JsValue) { emit(4, &[a, b]) }
    pub fn error_3(a: &JsValue, b: &JsValue, c: &JsValue) { emit(4, &[a, b, c]) }

    crate::import! {
        fn js_group(p: usize, l: usize, collapsed: u32) =
            "(p, l, c) => { (c ? console.groupCollapsed : console.group)(G.str(p, l)); }";
        fn js_group_end() = "() => { console.groupEnd(); }";
    }

    pub fn group_collapsed_1(label: &JsValue) {
        let s = label.as_string().unwrap_or_default();
        let (p, l) = string::abi(&s);
        unsafe { js_group(p, l, 1) }
    }
    pub fn group_1(label: &JsValue) {
        let s = label.as_string().unwrap_or_default();
        let (p, l) = string::abi(&s);
        unsafe { js_group(p, l, 0) }
    }
    pub fn group_end() {
        unsafe { js_group_end() }
    }

}
