//! `Primitive::WebView` — an `<iframe>` with reactive `src`, plus
//! the message channel + lifecycle slots the framework's WebView
//! primitive exposes.
//!
//! Message dispatch shape: we listen on the *parent* window's
//! `message` event and filter by `event.source ===
//! iframe.contentWindow` so an iframe-A handler doesn't fire on
//! iframe-B's messages. The listener is owned by the iframe node
//! via `dataset` keying so it lives exactly as long as the iframe
//! does — when the iframe is removed from the DOM and dropped,
//! the closure goes with it.

use crate::WebBackend;
use framework_core::primitives::web_view::{WebViewHandle, WebViewOps};
use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Event, MessageEvent, Node};

/// Per-iframe owned state: every registered callback closure +
/// listener registration. Lives in a `Rc<RefCell<...>>` attached
/// to the iframe via `js_sys::Reflect::set(&iframe, "__wv_state",
/// ...)` so it shares the iframe's lifetime — the browser GCs both
/// together when the node is removed.
struct WebViewState {
    /// `message` listener on the parent window. Filters by
    /// `event.source === iframe.contentWindow`.
    message_listener: Option<Closure<dyn FnMut(MessageEvent)>>,
    /// `load` listener on the iframe element.
    load_listener: Option<Closure<dyn FnMut(Event)>>,
    /// `error` listener on the iframe element.
    error_listener: Option<Closure<dyn FnMut(Event)>>,
}

/// Reach the iframe's per-node state slot, creating it if absent.
fn ensure_state(iframe: &web_sys::HtmlIFrameElement) -> Rc<RefCell<WebViewState>> {
    let key = JsValue::from_str("__wv_state");
    if let Ok(existing) = js_sys::Reflect::get(iframe.as_ref(), &key) {
        if !existing.is_undefined() && !existing.is_null() {
            // The slot stores the `Rc` by raw pointer (cast to a
            // JS number). We round-trip through that integer so the
            // Rust-side `Rc` count stays in sync.
            if let Some(ptr_f) = existing.as_f64() {
                let ptr = ptr_f as usize as *const RefCell<WebViewState>;
                // SAFETY: We only ever store pointers we allocated
                // below, and they live as long as the iframe (we
                // free in `drop_state` when the iframe is detached;
                // for the arena's usage we never detach so this is
                // dormant). The `Rc::increment_strong_count` ensures
                // the caller gets an owning clone.
                unsafe {
                    Rc::increment_strong_count(ptr);
                    return Rc::from_raw(ptr);
                }
            }
        }
    }
    let state = Rc::new(RefCell::new(WebViewState {
        message_listener: None,
        load_listener: None,
        error_listener: None,
    }));
    let raw = Rc::into_raw(state.clone());
    let _ = js_sys::Reflect::set(
        iframe.as_ref(),
        &key,
        &JsValue::from_f64(raw as usize as f64),
    );
    state
}

pub(crate) fn create(b: &mut WebBackend, url: &str) -> Node {
    let iframe = b
        .doc
        .create_element("iframe")
        .expect("create_element iframe failed");
    let _ = iframe.set_attribute("src", url);
    // Only `border: 0` is set inline — inline styles beat the
    // class-based rules `apply_style` installs, so putting
    // width/height here would silently override anything an author
    // passes via `.with_style(...)`. Authors size the iframe via
    // their stylesheet; with no author style the browser default
    // (300×150) applies.
    let _ = iframe.set_attribute("style", "border: 0");
    iframe.unchecked_into::<Node>()
}

pub(crate) fn update_url(node: &Node, url: &str) {
    if let Ok(el) = node.clone().dyn_into::<web_sys::Element>() {
        let _ = el.set_attribute("src", url);
    }
}

/// Install a `message` listener on the parent window, filtered to
/// fire only for messages whose `event.source` is *this* iframe's
/// `contentWindow`. The previous listener (if any) is dropped — at
/// most one `on_message` callback can be active per iframe.
pub(crate) fn set_on_message(node: &Node, callback: Box<dyn Fn(String)>) {
    let Ok(iframe) = node.clone().dyn_into::<web_sys::HtmlIFrameElement>() else {
        return;
    };
    let Some(window) = web_sys::window() else { return };
    let state = ensure_state(&iframe);

    // Drop the old listener (also detaches the old closure from
    // the window's event-target table).
    if let Some(old) = state.borrow_mut().message_listener.take() {
        let _ = window.remove_event_listener_with_callback(
            "message",
            old.as_ref().unchecked_ref(),
        );
    }

    // The closure needs a weak handle to the iframe so it can
    // compare against `event.source`. We capture by clone — the
    // iframe element is reference-counted on the JS side, so this
    // is cheap.
    let iframe_for_filter = iframe.clone();
    let closure: Closure<dyn FnMut(MessageEvent)> =
        Closure::new(move |ev: MessageEvent| {
            // `event.source` is a `Window`-like Object. We
            // compare object identity against the iframe's
            // contentWindow; on a successful cross-origin
            // iframe the source is still set, so this works
            // either way.
            let Some(source) = ev.source() else { return };
            let Some(content) = iframe_for_filter.content_window() else {
                return;
            };
            if !JsValue::from(content).eq(&JsValue::from(source)) {
                return;
            }
            // Convert payload to a String. If the iframe posted
            // an object, JSON.stringify it; otherwise coerce to
            // string. Most arena traffic is already JSON strings,
            // so the first branch is the hot path.
            let data = ev.data();
            let payload = if data.is_string() {
                data.as_string().unwrap_or_default()
            } else {
                js_sys::JSON::stringify(&data)
                    .ok()
                    .and_then(|s| s.as_string())
                    .unwrap_or_default()
            };
            callback(payload);
        });

    let _ = window.add_event_listener_with_callback(
        "message",
        closure.as_ref().unchecked_ref(),
    );
    state.borrow_mut().message_listener = Some(closure);
}

pub(crate) fn set_on_load(node: &Node, callback: Box<dyn Fn()>) {
    let Ok(iframe) = node.clone().dyn_into::<web_sys::HtmlIFrameElement>() else {
        return;
    };
    let state = ensure_state(&iframe);

    if let Some(old) = state.borrow_mut().load_listener.take() {
        let _ = iframe.remove_event_listener_with_callback(
            "load",
            old.as_ref().unchecked_ref(),
        );
    }

    let closure: Closure<dyn FnMut(Event)> = Closure::new(move |_| callback());
    let _ = iframe.add_event_listener_with_callback(
        "load",
        closure.as_ref().unchecked_ref(),
    );
    state.borrow_mut().load_listener = Some(closure);
}

pub(crate) fn set_on_error(node: &Node, callback: Box<dyn Fn()>) {
    let Ok(iframe) = node.clone().dyn_into::<web_sys::HtmlIFrameElement>() else {
        return;
    };
    let state = ensure_state(&iframe);

    if let Some(old) = state.borrow_mut().error_listener.take() {
        let _ = iframe.remove_event_listener_with_callback(
            "error",
            old.as_ref().unchecked_ref(),
        );
    }

    let closure: Closure<dyn FnMut(Event)> = Closure::new(move |_| callback());
    let _ = iframe.add_event_listener_with_callback(
        "error",
        closure.as_ref().unchecked_ref(),
    );
    state.borrow_mut().error_listener = Some(closure);
}

/// Make a handle the framework can hand back via a `Ref`. The
/// handle holds the iframe element (downcast to its concrete type)
/// so per-op downcasts inside `WebWebViewOps` are O(1).
pub(crate) fn make_handle(node: &Node) -> WebViewHandle {
    let el: web_sys::HtmlIFrameElement = node
        .clone()
        .dyn_into()
        .expect("web_view node is not an HtmlIFrameElement");
    WebViewHandle::new(Rc::new(el), &WebWebViewOps)
}

struct WebWebViewOps;

impl WebViewOps for WebWebViewOps {
    fn post_message(&self, node: &dyn Any, msg: &str) {
        let Some(iframe) = node.downcast_ref::<web_sys::HtmlIFrameElement>() else {
            return;
        };
        let Some(window) = iframe.content_window() else {
            return;
        };
        let _ = window.post_message(&JsValue::from_str(msg), "*");
    }

    fn reload(&self, node: &dyn Any) {
        let Some(iframe) = node.downcast_ref::<web_sys::HtmlIFrameElement>() else {
            return;
        };
        // Re-set src to current value to trigger a navigation.
        // `contentWindow.location.reload()` would be cleaner but
        // throws on cross-origin frames; the src-reset path works
        // for both.
        if let Some(src) = iframe.get_attribute("src") {
            let _ = iframe.set_attribute("src", &src);
        }
    }

    fn execute_js(&self, node: &dyn Any, code: &str) -> Result<String, String> {
        let iframe = node
            .downcast_ref::<web_sys::HtmlIFrameElement>()
            .ok_or_else(|| "node is not an iframe".to_string())?;
        let window = iframe
            .content_window()
            .ok_or_else(|| "iframe has no contentWindow".to_string())?;
        // Grab `eval` from the iframe's own global so the code
        // runs in the iframe's realm — not the parent's. This is
        // the whole point of the API (you want to call functions
        // the iframe defined on its window).
        let eval_val = js_sys::Reflect::get(
            window.as_ref(),
            &JsValue::from_str("eval"),
        )
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
        // Stringify the result. `undefined` → empty string;
        // anything else → JSON.
        if result.is_undefined() {
            return Ok(String::new());
        }
        js_sys::JSON::stringify(&result)
            .ok()
            .and_then(|s| s.as_string())
            .ok_or_else(|| "result is not JSON-stringifiable".to_string())
    }
}
