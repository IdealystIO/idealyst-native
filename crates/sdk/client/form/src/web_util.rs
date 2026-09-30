//! wasm32 helpers for the web leg — pure DOM ops on the mounted
//! `<form>`, no core types, kept separable from the primitive's
//! core-facing surface.

use std::any::Any;
use wasm_bindgen::JsCast;

/// `FormOps::submit` on web: downcast the type-erased mounted node to
/// the concrete `<form>` element and call `requestSubmit()` (not
/// `submit()`) so constraint validation runs AND the `submit` event
/// fires — routing through the same listener that calls `on_submit` +
/// `preventDefault()`. Silently no-ops when the node isn't a form
/// (matches the ops-trait degradation contract).
pub(crate) fn request_submit(node: &dyn Any) {
    // HYBRID-BRIDGE: the host node is a `web_glue::dom::Node` since phase
    // 2b; this SDK's internals are still web-sys until phase 3.
    let Some(form) = backend_web::bridge::node_to_web_sys(node)
        .and_then(|n| n.dyn_into::<web_sys::HtmlFormElement>().ok())
    else {
        return;
    };
    let _ = form.request_submit();
}
