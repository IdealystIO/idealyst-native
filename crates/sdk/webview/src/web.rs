//! Web (`target_arch = "wasm32"`) implementation of the WebView SDK.
//!
//! Builds an `<iframe>` per mount. Reactive URL changes flow through
//! `Effect::new(...)` inside the handler (the framework runs us inside
//! the walker's active scope, so the effect is owned by the scope and
//! survives past handler return). Message / load / error callbacks are
//! wired as DOM event listeners with their closures persisted in JS
//! reflect slots on the iframe so the iframe's lifetime owns them.

use crate::{WebViewOps, WebViewProps};
use backend_web::WebBackend;
use runtime_core::Effect;
use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Event, MessageEvent};

/// Static referenced by `lib.rs`'s `OPS` slot on this target.
pub(crate) static OPS: &dyn WebViewOps = &WebWebViewOps;

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

/// Register the WebView handler against a `WebBackend`. One-line call
/// from the app's bootstrap.
pub fn register(backend: &mut WebBackend) {
    backend.register_external::<WebViewProps, _>(|props, _backend| build_iframe(props));
}

// Self-register at backend construction (no app-side `register` call needed).
// See [[project_inventory_self_registration]].
inventory::submit! {
    backend_web::WebExternalRegistrar(register)
}

fn build_iframe(props: &Rc<WebViewProps>) -> web_sys::Element {
    let document = web_sys::window()
        .expect("no window")
        .document()
        .expect("no document");
    let iframe = document
        .create_element("iframe")
        .expect("create_element(iframe) failed");
    // Only `border: 0` set inline — keeping size/positioning to the
    // author's stylesheet (inline `style` would override class rules).
    let _ = iframe.set_attribute("style", "border: 0");
    let _ = iframe.set_attribute(
        "data-external-kind",
        "webview::WebViewProps",
    );

    // Reactive src. The walker calls us inside its active scope, so
    // the Effect's slot is owned by that scope — `_effect` going out
    // of this function is fine, the scope keeps it alive.
    let iframe_for_url = iframe.clone();
    let url_fn = SharedUrl::new(props);
    let _effect = Effect::new(move || {
        let url = url_fn.read();
        let _ = iframe_for_url.set_attribute("src", &url);
    });

    // Persistent per-iframe listener state, held by the iframe via a
    // JS reflect property so the closure lifetimes match the iframe's.
    let state = Rc::new(RefCell::new(WebViewState {
        message_listener: None,
        load_listener: None,
        error_listener: None,
    }));

    if let Some(cb) = props.on_message.clone() {
        wire_on_message(&iframe, &state, cb);
    }
    if let Some(cb) = props.on_load.clone() {
        wire_on_load(&iframe, &state, cb);
    }
    if let Some(cb) = props.on_error.clone() {
        wire_on_error(&iframe, &state, cb);
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

    iframe
}

/// Wraps the props' `url` closure so the Effect closure can read it
/// without holding a borrow on the `Rc<WebViewProps>` itself. Cloning
/// the props Rc into the Effect is fine, but indirecting through this
/// keeps the Effect closure body readable (`url_fn.read()` vs
/// `(props_clone.url)()`).
struct SharedUrl(Rc<WebViewProps>);
impl SharedUrl {
    fn new(props: &Rc<WebViewProps>) -> Self {
        Self(props.clone())
    }
    fn read(&self) -> String {
        (self.0.url)()
    }
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
// Imperative ops
// ============================================================================

struct WebWebViewOps;

impl WebViewOps for WebWebViewOps {
    fn post_message(&self, node: &dyn Any, msg: &str) {
        let Some(iframe) = node
            .downcast_ref::<web_sys::Node>()
            .and_then(|n| n.clone().dyn_into::<web_sys::HtmlIFrameElement>().ok())
        else {
            return;
        };
        let Some(window) = iframe.content_window() else {
            return;
        };
        let _ = window.post_message(&JsValue::from_str(msg), "*");
    }

    fn reload(&self, node: &dyn Any) {
        let Some(iframe) = node
            .downcast_ref::<web_sys::Node>()
            .and_then(|n| n.clone().dyn_into::<web_sys::HtmlIFrameElement>().ok())
        else {
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

    fn execute_js(&self, node: &dyn Any, code: &str) -> Result<String, String> {
        let iframe = node
            .downcast_ref::<web_sys::Node>()
            .and_then(|n| n.clone().dyn_into::<web_sys::HtmlIFrameElement>().ok())
            .ok_or_else(|| "node is not an iframe".to_string())?;
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
}
