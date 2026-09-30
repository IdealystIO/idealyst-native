//! The backend's side of the web-glue port (own-web-bindings phase 2a).
//!
//! Event listeners, the scheduler, the executor, the time source and the
//! JS shims run on web-glue; the DOM-operation surface (create / attribute
//! / style writes) and the `Host::Node` type are still web-sys until phase
//! 2b, because SDK mount handlers receive `web_sys::Node`s (phase 3). So a
//! listener's TARGET arrives as a web-sys value and crosses once, here,
//! through the HYBRID-BRIDGE (`web_glue::bridge`) — the one place this
//! crate converts between the two handle spaces. Phase 2b removes the
//! crossing when nodes are created as glue handles in the first place.

use web_glue::dom::{Event, EventTarget, Listener, ListenerOptions};

/// A web-sys value (an element, `window`, `document`) as a glue
/// `EventTarget`. HYBRID-BRIDGE.
pub(crate) fn target(t: &impl AsRef<wasm_bindgen::JsValue>) -> EventTarget {
    web_glue::bridge::from_bindgen(t.as_ref())
}

/// A glue handle as a web-sys value of type `T` (unchecked — the caller
/// knows what the handle is). HYBRID-BRIDGE.
pub(crate) fn to_web_sys<T: wasm_bindgen::JsCast>(v: &impl web_glue::JsCast) -> T {
    wasm_bindgen::JsCast::unchecked_into(web_glue::bridge::to_bindgen(v))
}

/// Attach `f` to `t` for `ty`; the returned [`Listener`] detaches on drop.
pub(crate) fn listen(
    t: &impl AsRef<wasm_bindgen::JsValue>,
    ty: &'static str,
    options: ListenerOptions,
    f: impl FnMut(Event) + 'static,
) -> Listener {
    Listener::new(target(t), ty, options, f)
}

/// Attach `f` to `t` for `ty` for the element's lifetime: the element keeps
/// the listener alive, and the Rust closure is released when JS collects
/// the element (see `web_glue::Closure::into_js_value` and
/// `primitives::own_listener`'s history). For listeners on nodes with no
/// teardown record; anything that must detach earlier uses
/// `WebBackend::track_listener`.
pub(crate) fn listen_for_element_lifetime(
    t: &impl AsRef<wasm_bindgen::JsValue>,
    ty: &'static str,
    options: ListenerOptions,
    f: impl FnMut(Event) + 'static,
) {
    listen(t, ty, options, f).into_target_owned();
}

/// `{ capture }` with the browser's default passivity.
pub(crate) fn capture(capture: bool) -> ListenerOptions {
    ListenerOptions { capture, ..ListenerOptions::default() }
}

web_glue::import! {
    fn js_set_onclick(el: u32, f: u32) = "(e, f) => { G.get(e).onclick = G.get(f); }";
    fn js_raf_once(f: u32) = "(f) => { requestAnimationFrame(G.get(f)); }";
}

/// Run `f` once, on the next animation frame. Fire-and-forget (nothing can
/// cancel it): the closure frees itself when it runs. For "measure once
/// the node is laid out" deferrals; anything cancellable goes through
/// `runtime_shared::after_animation_frame`.
pub(crate) fn next_frame(f: impl FnOnce() + 'static) {
    let func = web_glue::Closure::once_into_js(move |_| f());
    unsafe { js_raf_once(func.raw()) }
}

/// `el.onclick = f` — the property handler (one per element; replacing it
/// replaces the old one, unlike `addEventListener`). The element owns the
/// function; the Rust closure is released when JS collects it (see
/// [`listen_for_element_lifetime`]).
pub(crate) fn set_onclick(el: &impl AsRef<wasm_bindgen::JsValue>, f: impl FnMut() + 'static) {
    let mut f = f;
    let func = web_glue::Closure::new(move |_| f()).into_js_value();
    let el = target(el);
    unsafe { js_set_onclick(el.as_js().raw(), func.raw()) }
}

/// [`listen_for_element_lifetime`] for a handler that may be re-entered (a
/// `scroll` handler whose body synchronously re-fires `scroll`) — see
/// `web_glue::Closure::new_fn`.
pub(crate) fn listen_fn_for_element_lifetime(
    t: &impl AsRef<wasm_bindgen::JsValue>,
    ty: &'static str,
    f: impl Fn(Event) + 'static,
) {
    Listener::new_fn(target(t), ty, ListenerOptions::default(), f).into_target_owned();
}

/// [`listen`] for a handler that may be re-entered (a focus trap whose own
/// `.focus()` re-dispatches `focusin`) — see `web_glue::Closure::new_fn`.
pub(crate) fn listen_fn(
    t: &impl AsRef<wasm_bindgen::JsValue>,
    ty: &'static str,
    options: ListenerOptions,
    f: impl Fn(Event) + 'static,
) -> Listener {
    Listener::new_fn(target(t), ty, options, f)
}
