//! Pure wasm32 DOM helpers for the web leg (no core types): the iframe
//! event-listener wiring (with its `__wv_state` closure persistence)
//! and the imperative `WebViewOps` bodies. Kept out of `lib.rs` so the
//! DOM plumbing stays separable from the primitive's core-facing
//! surface.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Event, MessageEvent};

// ============================================================================
// Event listener wiring
// ============================================================================

/// Per-iframe owned state — the registered listener closures stay
/// alive in here so the browser's event-target table keeps a valid
/// callback to fire. Detaching the iframe drops the `Rc` (the slot is
/// held by the iframe's JS reflect property) which drops every closure
/// inside.
struct WebViewState {
    message_listener: Option<Closure<dyn FnMut(MessageEvent)>>,
    load_listener: Option<Closure<dyn FnMut(Event)>>,
    error_listener: Option<Closure<dyn FnMut(Event)>>,
}

/// Wire the author's message/load/error callbacks as DOM event
/// listeners on `iframe`, then persist the listener closures via the
/// iframe's `__wv_state` JS reflect slot so their lifetimes match the
/// iframe's. The web handler calls this with the flush-wrapped
/// callbacks it wants fired.
pub(crate) fn wire_listeners(
    iframe: &web_sys::Element,
    on_message: Option<Rc<dyn Fn(String)>>,
    on_load: Option<Rc<dyn Fn()>>,
    on_error: Option<Rc<dyn Fn()>>,
) {
    // Persistent per-iframe listener state, held by the iframe via a
    // JS reflect property so the closure lifetimes match the iframe's.
    let state = Rc::new(RefCell::new(WebViewState {
        message_listener: None,
        load_listener: None,
        error_listener: None,
    }));

    if let Some(cb) = on_message {
        wire_on_message(iframe, &state, cb);
    }
    if let Some(cb) = on_load {
        wire_on_load(iframe, &state, cb);
    }
    if let Some(cb) = on_error {
        wire_on_error(iframe, &state, cb);
    }

    // Stash the state Rc on the iframe so its lifetime matches the
    // iframe's. Using the same `__wv_state` slot the framework-shipped
    // impl used so debugging tools that introspect this property keep
    // working through the migration.
    let raw = Rc::into_raw(state);
    let _ = js_sys::Reflect::set(
        iframe.as_ref(),
        &JsValue::from_str("__wv_state"),
        &JsValue::from_f64(raw as usize as f64),
    );
}

fn wire_on_message(
    iframe: &web_sys::Element,
    state: &Rc<RefCell<WebViewState>>,
    cb: Rc<dyn Fn(String)>,
) {
    let Some(window) = web_sys::window() else { return };
    let Ok(iframe_typed) = iframe.clone().dyn_into::<web_sys::HtmlIFrameElement>() else {
        return;
    };
    // The closure filters by `event.source === iframe.contentWindow`
    // so messages from sibling iframes don't fire this handler.
    let iframe_for_filter = iframe_typed.clone();
    let closure: Closure<dyn FnMut(MessageEvent)> =
        Closure::new(move |ev: MessageEvent| {
            let Some(source) = ev.source() else { return };
            let Some(content) = iframe_for_filter.content_window() else {
                return;
            };
            if !JsValue::from(content).eq(&JsValue::from(source)) {
                return;
            }
            let data = ev.data();
            let payload = if data.is_string() {
                data.as_string().unwrap_or_default()
            } else {
                js_sys::JSON::stringify(&data)
                    .ok()
                    .and_then(|s| s.as_string())
                    .unwrap_or_default()
            };
            cb(payload);
        });
    let _ = window.add_event_listener_with_callback(
        "message",
        closure.as_ref().unchecked_ref(),
    );
    state.borrow_mut().message_listener = Some(closure);
}

fn wire_on_load(
    iframe: &web_sys::Element,
    state: &Rc<RefCell<WebViewState>>,
    cb: Rc<dyn Fn()>,
) {
    let closure: Closure<dyn FnMut(Event)> = Closure::new(move |_| cb());
    let _ = iframe.add_event_listener_with_callback(
        "load",
        closure.as_ref().unchecked_ref(),
    );
    state.borrow_mut().load_listener = Some(closure);
}

fn wire_on_error(
    iframe: &web_sys::Element,
    state: &Rc<RefCell<WebViewState>>,
    cb: Rc<dyn Fn()>,
) {
    let closure: Closure<dyn FnMut(Event)> = Closure::new(move |_| cb());
    let _ = iframe.add_event_listener_with_callback(
        "error",
        closure.as_ref().unchecked_ref(),
    );
    state.borrow_mut().error_listener = Some(closure);
}

// ============================================================================
// Imperative ops bodies — the whole `WebViewOps` impl on web. Each
// takes the type-erased handle node (a `web_sys::Node` wrapping the
// `<iframe>`).
// ============================================================================

/// Downcast the type-erased handle node to the mounted `<iframe>`.
fn as_iframe(node: &dyn Any) -> Option<web_sys::HtmlIFrameElement> {
    node.downcast_ref::<web_sys::Node>()
        .and_then(|n| n.clone().dyn_into::<web_sys::HtmlIFrameElement>().ok())
}

/// `WebViewOps::post_message` body: route to
/// `iframe.contentWindow.postMessage(msg, "*")`.
pub(crate) fn post_message(node: &dyn Any, msg: &str) {
    let Some(iframe) = as_iframe(node) else {
        return;
    };
    let Some(window) = iframe.content_window() else {
        return;
    };
    let _ = window.post_message(&JsValue::from_str(msg), "*");
}

/// `WebViewOps::reload` body.
pub(crate) fn reload(node: &dyn Any) {
    let Some(iframe) = as_iframe(node) else {
        return;
    };
    // Re-set src to current value to trigger a navigation.
    // `contentWindow.location.reload()` would be cleaner but
    // throws on cross-origin frames; the src-reset path works for
    // both.
    if let Some(src) = iframe.get_attribute("src") {
        let _ = iframe.set_attribute("src", &src);
    }
}

/// `WebViewOps::execute_js` body: sync `eval` in the iframe's global
/// scope, JSON-stringified result.
pub(crate) fn execute_js(node: &dyn Any, code: &str) -> Result<String, String> {
    let iframe = as_iframe(node).ok_or_else(|| "node is not an iframe".to_string())?;
    let window = iframe
        .content_window()
        .ok_or_else(|| "iframe has no contentWindow".to_string())?;
    let eval_val = js_sys::Reflect::get(window.as_ref(), &JsValue::from_str("eval"))
        .map_err(|_| "iframe is cross-origin; eval is inaccessible".to_string())?;
    let eval_fn: js_sys::Function = eval_val
        .dyn_into()
        .map_err(|_| "iframe's `eval` is not callable".to_string())?;
    let result = eval_fn
        .call1(window.as_ref(), &JsValue::from_str(code))
        .map_err(|e| {
            js_sys::JSON::stringify(&e)
                .ok()
                .and_then(|s| s.as_string())
                .unwrap_or_else(|| "(non-stringifiable exception)".to_string())
        })?;
    if result.is_undefined() {
        return Ok(String::new());
    }
    js_sys::JSON::stringify(&result)
        .ok()
        .and_then(|s| s.as_string())
        .ok_or_else(|| "result is not JSON-stringifiable".to_string())
}
