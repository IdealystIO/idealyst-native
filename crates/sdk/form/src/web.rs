//! Web (`target_arch = "wasm32"`) implementation of the Form SDK.
//!
//! Builds a real `<form>` element per mount. The framework parents the
//! author's children (inputs, buttons) into it as real DOM descendants
//! — that's what makes browser autofill, password-manager grouping, and
//! submit-on-enter work; none of it is reproducible by styling a
//! `<div>`.
//!
//! The form's native `submit` event is wired to `on_submit` with a
//! `preventDefault()` so the browser never navigates/reloads (idealyst
//! apps don't POST form-encoded data — `on_submit` reads field signals).

use crate::{FormOps, FormProps};
use backend_web::WebBackend;
use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::Event;

/// Static referenced by `lib.rs`'s `OPS` slot on this target.
pub(crate) static OPS: &dyn FormOps = &WebFormOps;

/// Per-form owned state — the submit listener closure stays alive here
/// so the browser's event-target table keeps a valid callback to fire.
/// Detaching the form drops the `Rc` (held via a JS reflect property),
/// which drops the closure.
struct FormState {
    submit_listener: Option<Closure<dyn FnMut(Event)>>,
}

/// Register the Form handler against a `WebBackend`. One-line call from
/// the app's bootstrap.
pub fn register(backend: &mut WebBackend) {
    backend.register_external::<FormProps, _>(|props, _backend| build_form(props));
}

fn build_form(props: &Rc<FormProps>) -> web_sys::Element {
    let document = web_sys::window()
        .expect("no window")
        .document()
        .expect("no document");
    let form = document
        .create_element("form")
        .expect("create_element(form) failed");
    let _ = form.set_attribute("data-external-kind", "form::FormProps");

    let state = Rc::new(RefCell::new(FormState { submit_listener: None }));

    if let Some(cb) = props.on_submit.clone() {
        // `preventDefault()` is mandatory: without it the browser
        // performs the default GET/POST navigation and reloads the SPA,
        // tearing down the framework runtime. idealyst forms carry their
        // data in signals, not FormData, so the default action is never
        // wanted.
        let closure: Closure<dyn FnMut(Event)> = Closure::new(move |ev: Event| {
            ev.prevent_default();
            cb();
        });
        let _ = form
            .add_event_listener_with_callback("submit", closure.as_ref().unchecked_ref());
        state.borrow_mut().submit_listener = Some(closure);
    }

    // Stash the state Rc on the form so its lifetime matches the form's.
    let raw = Rc::into_raw(state);
    let _ = js_sys::Reflect::set(
        form.as_ref(),
        &JsValue::from_str("__form_state"),
        &JsValue::from_f64(raw as usize as f64),
    );

    form
}

// ============================================================================
// Imperative ops
// ============================================================================

struct WebFormOps;

impl FormOps for WebFormOps {
    fn submit(&self, node: &dyn Any) {
        let Some(form) = node
            .downcast_ref::<web_sys::Node>()
            .and_then(|n| n.clone().dyn_into::<web_sys::HtmlFormElement>().ok())
        else {
            return;
        };
        // `requestSubmit()` (not `submit()`) so constraint validation
        // runs AND the `submit` event fires — routing through the same
        // listener that calls `on_submit` + `preventDefault()`.
        let _ = form.request_submit();
    }
}
