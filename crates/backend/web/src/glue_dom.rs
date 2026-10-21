//! Listener helpers over `web_glue::dom`.
//!
//! Every value the backend holds is a glue handle, so a listener target is
//! just the node itself — `AsRef<JsValue>` covers every typed class.

use web_glue::dom::{Event, EventTarget, Listener, ListenerOptions};
use web_glue::{JsCast, JsValue};

fn target(t: &impl AsRef<JsValue>) -> EventTarget {
    t.as_ref().clone().unchecked_into()
}

/// Attach `f` to `t` for `ty`; the returned [`Listener`] detaches on drop.
pub(crate) fn listen(
    t: &impl AsRef<web_glue::JsValue>,
    ty: &'static str,
    options: ListenerOptions,
    f: impl FnMut(Event) + 'static,
) -> Listener {
    Listener::new(target(t), ty, options, f)
}

/// Attach `f` to `t` for `ty` for the element's lifetime: the element keeps
/// the listener alive, and the Rust closure is released when JS collects
/// the element (see `web_glue::Closure::into_js_value`). For listeners on nodes with no
/// teardown record; anything that must detach earlier uses
/// `WebBackend::track_listener`.
pub(crate) fn listen_for_element_lifetime(
    t: &impl AsRef<web_glue::JsValue>,
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
    fn js_set_press_onclick(el: u32, f: u32) = "(e, f) => { const h = G.get(f); G.get(e).onclick = (ev) => { if (ev.__iyPress) return; ev.__iyPress = true; h(ev); }; }";
    fn js_claim_press(ev: u32) -> u32 = "(ev) => { const e = G.get(ev); if (e.__iyPress) return 0; e.__iyPress = true; return 1; }";
    fn js_raf_once(f: u32) = "(f) => { requestAnimationFrame(G.get(f)); }";
    fn js_timeout_once(f: u32, ms: i32) = "(f, ms) => { setTimeout(G.get(f), ms); }";
}

/// Run `f` once, on the next animation frame. Fire-and-forget (nothing can
/// cancel it): the closure frees itself when it runs. For "measure once
/// the node is laid out" deferrals; anything cancellable goes through
/// `runtime_shared::after_animation_frame`.
/// Run `f` once after `ms` milliseconds. Fire-and-forget, like
/// [`next_frame`].
pub(crate) fn after_ms_once(ms: i32, f: impl FnOnce() + 'static) {
    let func = web_glue::Closure::once_into_js(move |_| f());
    unsafe { js_timeout_once(func.raw(), ms) }
}

pub(crate) fn next_frame(f: impl FnOnce() + 'static) {
    let func = web_glue::Closure::once_into_js(move |_| f());
    unsafe { js_raf_once(func.raw()) }
}

/// `el.onclick = f` for a press target (pressable, button) — the property
/// handler (one per element; replacing it replaces the old one, unlike
/// `addEventListener`). The element owns the function; the Rust closure is
/// released when JS collects it (see [`listen_for_element_lifetime`]).
///
/// Only the innermost press target runs for one click. `click` bubbles, so
/// a pressable inside a pressable (a button in a clickable row) would
/// otherwise fire inner then outer, where native toolkits deliver the tap
/// to the innermost view alone. The first press handler to see the event
/// claims it with an expando flag and ancestors skip a claimed event. Not
/// `stopPropagation`: document-level click listeners (outside-click
/// dismissal) must still see the click. See [`claim_press`] for links.
pub(crate) fn set_press_onclick(el: &impl AsRef<web_glue::JsValue>, f: impl FnMut() + 'static) {
    let mut f = f;
    let func = web_glue::Closure::new(move |_| f()).into_js_value();
    let el = target(el);
    unsafe { js_set_press_onclick(el.as_js().raw(), func.raw()) }
}

/// Claim `ev` as handled by an inner press target (a link), so ancestor
/// [`set_press_onclick`] handlers skip it. Returns `false` when a deeper
/// target already claimed it.
pub(crate) fn claim_press(ev: &Event) -> bool {
    unsafe { js_claim_press(ev.as_js().raw()) == 1 }
}

/// [`listen_for_element_lifetime`] for a handler that may be re-entered (a
/// `scroll` handler whose body synchronously re-fires `scroll`) — see
/// `web_glue::Closure::new_fn`.
pub(crate) fn listen_fn_for_element_lifetime(
    t: &impl AsRef<web_glue::JsValue>,
    ty: &'static str,
    f: impl Fn(Event) + 'static,
) {
    Listener::new_fn(target(t), ty, ListenerOptions::default(), f).into_target_owned();
}

/// [`listen`] for a handler that may be re-entered (a focus trap whose own
/// `.focus()` re-dispatches `focusin`) — see `web_glue::Closure::new_fn`.
pub(crate) fn listen_fn(
    t: &impl AsRef<web_glue::JsValue>,
    ty: &'static str,
    options: ListenerOptions,
    f: impl Fn(Event) + 'static,
) -> Listener {
    Listener::new_fn(target(t), ty, options, f)
}

// Callback shapes the JS shims take (virtualizer / virtual grid): each is
// a `web_glue::Closure::new_with_args`, which receives every JS argument
// and returns a value to the shim. The argument count is the shim's
// contract; a missing argument reads as `undefined`.
fn arg(a: &[JsValue], i: usize) -> JsValue {
    a.get(i).cloned().unwrap_or_default()
}
pub(crate) fn fn0r(mut f: impl FnMut() -> JsValue + 'static) -> web_glue::Closure {
    web_glue::Closure::new_with_args(move |_| f())
}
pub(crate) fn fn1r(mut f: impl FnMut(JsValue) -> JsValue + 'static) -> web_glue::Closure {
    web_glue::Closure::new_with_args(move |a| f(arg(a, 0)))
}
pub(crate) fn fn1(mut f: impl FnMut(JsValue) + 'static) -> web_glue::Closure {
    web_glue::Closure::new_with_args(move |a| {
        f(arg(a, 0));
        JsValue::UNDEFINED
    })
}
pub(crate) fn fn2(mut f: impl FnMut(JsValue, JsValue) + 'static) -> web_glue::Closure {
    web_glue::Closure::new_with_args(move |a| {
        f(arg(a, 0), arg(a, 1));
        JsValue::UNDEFINED
    })
}
pub(crate) fn fn2r(mut f: impl FnMut(JsValue, JsValue) -> JsValue + 'static) -> web_glue::Closure {
    web_glue::Closure::new_with_args(move |a| f(arg(a, 0), arg(a, 1)))
}
pub(crate) fn fn4r(
    mut f: impl FnMut(JsValue, JsValue, JsValue, JsValue) -> JsValue + 'static,
) -> web_glue::Closure {
    web_glue::Closure::new_with_args(move |a| f(arg(a, 0), arg(a, 1), arg(a, 2), arg(a, 3)))
}
