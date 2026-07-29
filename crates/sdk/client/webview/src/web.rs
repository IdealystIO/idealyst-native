//! Web (`target_arch = "wasm32"`) implementation of the WebView SDK.
//!
//! Builds an `<iframe>` per mount. Reactive URL changes flow through
//! an `effect!` inside the handler (the framework runs us inside
//! the walker's active scope, so the effect is owned by the scope and
//! survives past handler return). Message / load / error callbacks are
//! wired as DOM event listeners with their closures persisted in JS
//! reflect slots on the iframe so the iframe's lifetime owns them —
//! the wiring + imperative-ops bodies live in `web_util`, shared with
//! the new-core web leg so the two can't drift.

use crate::{WebViewOps, WebViewProps};
use backend_web::WebBackend;
use std::any::Any;
use std::rc::Rc;

/// Static referenced by `lib.rs`'s `OPS` slot on this target.
pub(crate) static OPS: &dyn WebViewOps = &WebWebViewOps;

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
    runtime_core::effect!({
        let url = url_fn.read();
        let _ = iframe_for_url.set_attribute("src", &url);
    });

    // Author-callback listeners + `__wv_state` closure persistence
    // (shared with the new-core leg via `web_util`). No flush-wrapping
    // here — the old core's dispatch model doesn't stage writes.
    crate::web_util::wire_listeners(
        &iframe,
        props.on_message.clone(),
        props.on_load.clone(),
        props.on_error.clone(),
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

// ============================================================================
// Imperative ops — bodies shared with the new-core leg via `web_util`.
// ============================================================================

struct WebWebViewOps;

impl WebViewOps for WebWebViewOps {
    fn post_message(&self, node: &dyn Any, msg: &str) {
        crate::web_util::post_message(node, msg);
    }

    fn reload(&self, node: &dyn Any) {
        crate::web_util::reload(node);
    }

    fn execute_js(&self, node: &dyn Any, code: &str) -> Result<String, String> {
        crate::web_util::execute_js(node, code)
    }
}
