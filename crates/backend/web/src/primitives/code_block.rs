//! `Primitive::CodeBlock` — a `<pre>` containing one `<span>` per
//! colored text run. The span's CSS `color` is set inline from the
//! tuple's `Color`. The framework's stylesheet layer styles the
//! outer `<pre>` (font-family, padding, background) — we don't
//! set any of those here so callers stay in control.

use crate::WebBackend;
use framework_core::Color;
use wasm_bindgen::JsCast;
use web_sys::Node;

pub(crate) fn create(b: &mut WebBackend, spans: &[(String, Color)]) -> Node {
    let pre = b
        .doc
        .create_element("pre")
        .expect("create_element pre failed");
    // Match the layout defaults the textarea overlay relies on:
    // no margin (so it lines up with a positioned `<textarea>`
    // sibling), `white-space: pre` so leading whitespace renders
    // verbatim, `tab-size: 4` to match the textarea's tab width,
    // and `pointer-events: none` so clicks pass through to a
    // sibling `<textarea>` layered on top of (or beneath) it —
    // the fiddle's editor uses exactly that pattern.
    let _ = pre.set_attribute(
        "style",
        "margin: 0; white-space: pre; tab-size: 4; \
         pointer-events: none;",
    );
    write_spans(b, &pre, spans);
    pre.unchecked_into::<Node>()
}

pub(crate) fn update_spans(b: &mut WebBackend, node: &Node, spans: &[(String, Color)]) {
    let Ok(pre) = node.clone().dyn_into::<web_sys::Element>() else { return };
    // Clear in one shot — `set_inner_html("")` is cheaper than
    // walking and removing child-by-child. Per-keystroke updates
    // are the common case here so the savings matter.
    pre.set_inner_html("");
    write_spans(b, &pre, spans);
}

fn write_spans(b: &mut WebBackend, pre: &web_sys::Element, spans: &[(String, Color)]) {
    for (text, color) in spans {
        let span = b
            .doc
            .create_element("span")
            .expect("create_element span failed");
        // Inline `color: …` so authors don't have to thread a
        // stylesheet rule per token kind. The Color is already a
        // CSS-style string (`#rrggbb` / `rgb(...)`) since it's
        // what the rest of the framework uses for resolved values.
        let _ = span.set_attribute("style", &format!("color: {};", color.0));
        span.set_text_content(Some(text));
        let _ = pre.append_child(&span);
    }
}
