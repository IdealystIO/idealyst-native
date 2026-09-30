//! wasm32 helpers for the web leg — pure DOM ops on the mounted
//! `<form>`, no core types, kept separable from the primitive's
//! core-facing surface. DOM access is web-glue (the host node IS a
//! `web_glue::dom::Node`).

use std::any::Any;

use web_glue::dom::{Element, EventTarget, HtmlElement, Node};
use web_glue::JsCast;

web_glue::js_class! {
    /// `HTMLFormElement` — only what this SDK calls.
    pub(crate) struct HtmlFormElement: HtmlElement, Element, Node, EventTarget = "HTMLFormElement";
}

web_glue::import! {
    fn js_request_submit(form: u32) = "(f) => { G.get(f).requestSubmit(); }";
}

impl HtmlFormElement {
    /// `requestSubmit()`: runs constraint validation and fires `submit`
    /// (unlike `submit()`, which does neither).
    pub(crate) fn request_submit(&self) {
        // SAFETY: a live handle to a form element.
        unsafe { js_request_submit(self.as_js().raw()) }
    }
}

/// `FormOps::submit` on web: downcast the type-erased mounted node to
/// the concrete `<form>` element and call `requestSubmit()` (not
/// `submit()`) so constraint validation runs AND the `submit` event
/// fires — routing through the same listener that calls `on_submit` +
/// `preventDefault()`. Silently no-ops when the node isn't a form
/// (matches the ops-trait degradation contract).
pub(crate) fn request_submit(node: &dyn Any) {
    let Some(form) = node
        .downcast_ref::<Node>()
        .and_then(|n| n.dyn_ref::<HtmlFormElement>())
    else {
        return;
    };
    form.request_submit();
}
