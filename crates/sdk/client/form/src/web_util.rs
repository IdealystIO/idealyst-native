//! Shared wasm32 helpers used by BOTH cores' web legs — pure DOM ops on
//! the mounted `<form>`, no core types, so the two cores' imperative
//! behavior can't drift.

use std::any::Any;
use wasm_bindgen::JsCast;

/// `FormOps::submit` on web: downcast the type-erased mounted node to
/// the concrete `<form>` element and call `requestSubmit()` (not
/// `submit()`) so constraint validation runs AND the `submit` event
/// fires — routing through the same listener that calls `on_submit` +
/// `preventDefault()`. Silently no-ops when the node isn't a form
/// (matches the ops-trait degradation contract).
pub(crate) fn request_submit(node: &dyn Any) {
    let Some(form) = node
        .downcast_ref::<web_sys::Node>()
        .and_then(|n| n.clone().dyn_into::<web_sys::HtmlFormElement>().ok())
    else {
        return;
    };
    let _ = form.request_submit();
}
