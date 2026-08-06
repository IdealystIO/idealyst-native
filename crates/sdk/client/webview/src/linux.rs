//! Linux (GTK4) implementation of the WebView SDK — WebKitGTK 6.
//!
//! Builds a real embedded browser via `webkit6::WebView` (the GTK4 port
//! of WebKitGTK, linking `webkitgtk-6.0`). This is the direct analogue
//! of the iOS leaf's `WKWebView` and the web leaf's `<iframe>` — a full
//! rendering engine, not the framework's `External` placeholder.
//!
//! # Source resolution + reactivity
//!
//! A single `effect!` reads [`WebViewProps::url`] each run and navigates
//! the view. Because the read happens *inside* the effect, any signal the
//! URL closure touches re-fires it and re-navigates — one mechanism for a
//! reactive URL. A string that looks like inline HTML markup (starts with
//! `<`) is rendered via `load_html`; anything else is treated as a URI and
//! passed to `load_uri`. This keeps the canonical case (a URL, matching
//! the web/iOS leaves) identical while still honoring an inline-HTML
//! source on this backend.
//!
//! # Callbacks (parity with the web / iOS leaves)
//!
//! * `on_load` — fires from `load-changed` when the load reaches
//!   `LoadEvent::Finished` (the iframe `load` / `didFinishNavigation:`
//!   equivalent).
//! * `on_error` — fires from `load-failed`.
//! * `on_message` — page → native. Wired through a
//!   [`webkit6::UserContentManager`] script-message handler named
//!   `idealyst`. To keep the page-side API symmetrical with the web leaf
//!   (which listens for `window.message` events), a `UserScript` injected
//!   at document-start wraps `window.postMessage` so it ALSO fires
//!   `window.webkit.messageHandlers.idealyst.postMessage(payload)` — the
//!   channel this handler listens on. Byte-for-byte the same shim the iOS
//!   leaf installs.
//!
//! # Imperative ops
//!
//! `reload` and `post_message` (native → page, via a synthetic
//! `MessageEvent` dispatched with `evaluate_javascript`). Like the iOS
//! leaf, `execute_js` stays the trait-default error: WebKitGTK's
//! `evaluate_javascript` is callback-only (async), and honoring the
//! synchronous `execute_js` signature would mean blocking the GTK main
//! loop — which we refuse. `LinuxNode` hands the ops only a type-erased
//! `&LinuxNode`, so build-time (where we own the `WebView`) bridges to
//! op-time through a thread-local table keyed on the node's stable id —
//! mirroring the Video leaf.

use crate::{WebViewOps, WebViewProps};
use backend_linux::{LinuxBackend, LinuxNode};
use gtk4::gio;
use gtk4::prelude::*;
use javascriptcore6::prelude::*;
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use webkit6::prelude::*;

pub(crate) static OPS: &dyn WebViewOps = &LinuxWebViewOps;

thread_local! {
    /// node id → live `webkit6::WebView`. Populated by [`build_web_view`]
    /// at mount, cleared by the `on_cleanup` it installs (so a popped
    /// screen stops the engine). The imperative [`WebViewOps`] look their
    /// target up here by the id read off the `LinuxNode` they're
    /// dispatched with. Single-threaded (GTK main loop) — `WebView` is
    /// `!Send`, so a thread-local is the correct home.
    static WEBVIEWS: RefCell<HashMap<u64, webkit6::WebView>> = RefCell::new(HashMap::new());
}

/// The script-message channel name. The page reaches it via
/// `window.webkit.messageHandlers.idealyst.postMessage(...)`; the shim
/// below routes `window.postMessage` through it so authors keep the web
/// leaf's `postMessage` API.
const HANDLER_NAME: &str = "idealyst";

/// Injected at document-start: rebroadcasts the page's `window.postMessage`
/// to the `idealyst` script-message channel so `on_message` fires. The
/// original `postMessage` behavior is preserved. Identical to the iOS
/// leaf's shim — same page-side contract on every backend.
const POST_MESSAGE_SHIM_JS: &str = r#"
(function () {
  if (window.__idealyst_pm_shim_installed) return;
  window.__idealyst_pm_shim_installed = true;
  var original = window.postMessage;
  window.postMessage = function (msg, targetOrigin, transfer) {
    try {
      var payload = (typeof msg === 'string') ? msg : JSON.stringify(msg);
      window.webkit.messageHandlers.idealyst.postMessage(payload);
    } catch (e) {
      // Channel unavailable (handler removed, cross-origin) — fall
      // through so existing same-window listeners keep firing.
    }
    return original.apply(this, arguments);
  };
})();
"#;

/// Register the Linux `WebView` external handler on `backend`. Call once
/// at app boot (the app's `register_extensions` on Linux) so `WebView`
/// elements lower to a real `webkit6::WebView` instead of the framework's
/// External placeholder.
// =========================================================================
// Build + reactive source
// =========================================================================

pub(crate) fn build_web_view(props: &Rc<WebViewProps>, b: &mut LinuxBackend) -> LinuxNode {
    // A UserContentManager owns the script-message channel + the injected
    // shim. It's a construct-only property on WebView, so it must be set
    // via the builder (there's no post-construction setter).
    let ucm = webkit6::UserContentManager::new();
    ucm.register_script_message_handler(HANDLER_NAME, None);

    // Inject the postMessage shim at document-start into the top frame
    // only — injecting into sub-frames would loop their own postMessage
    // calls back to native (same reasoning as the iOS leaf's
    // `forMainFrameOnly: YES`).
    let shim = webkit6::UserScript::new(
        POST_MESSAGE_SHIM_JS,
        webkit6::UserContentInjectedFrames::TopFrame,
        webkit6::UserScriptInjectionTime::Start,
        &[],
        &[],
    );
    ucm.add_script(&shim);

    // page → native. The handler name is `idealyst`, matching the shim's
    // channel. The payload arrives as a JSC `Value`; we serialize it to
    // JSON (falling back to its string form) so the callback receives the
    // same opaque-string contract as the web/iOS leaves.
    if let Some(cb) = props.on_message.clone() {
        ucm.connect_script_message_received(Some(HANDLER_NAME), move |_ucm, value| {
            let payload = value
                .to_json(0)
                .map(|g| g.to_string())
                .unwrap_or_else(|| value.to_str().to_string());
            cb(payload);
        });
    }

    let web_view = webkit6::WebView::builder()
        .user_content_manager(&ucm)
        .build();

    // on_load: the load reaching `Finished`. `load-changed` fires through
    // Started → Committed → Finished; we only surface Finished, matching
    // the iframe `load` / WKWebView `didFinishNavigation:` semantics.
    if let Some(cb) = props.on_load.clone() {
        web_view.connect_load_changed(move |_wv, event| {
            if event == webkit6::LoadEvent::Finished {
                cb();
            }
        });
    }

    // on_error: any load failure. Returning `false` lets WebKit show its
    // default error page rather than us suppressing it.
    if let Some(cb) = props.on_error.clone() {
        web_view.connect_load_failed(move |_wv, _event, _uri, _error| {
            cb();
            false
        });
    }

    // Register the widget with the backend's Taffy tree (a flex parent
    // sizes + positions it) and record it for imperative ops.
    let node = b.register_external_view(web_view.clone().upcast::<gtk4::Widget>());
    let id = node.id();
    WEBVIEWS.with(|m| m.borrow_mut().insert(id, web_view.clone()));

    // Reactive URL. Owned by the walker's active scope (the framework runs
    // this handler inside it), so it survives past handler return and
    // re-fires when a signal the URL closure reads changes.
    let web_view_for_effect = web_view.clone();
    let props_for_effect = props.clone();
    runtime_core::effect!({
        let src = (props_for_effect.url)();
        load_source(&web_view_for_effect, &src);
    });

    // Tear the engine down when the WebView unmounts (screen pop /
    // navigation). Without this the entry leaks and the web process keeps
    // running. Mirrors the Video leaf's `on_cleanup`.
    runtime_core::on_cleanup(move || {
        if let Some(wv) = WEBVIEWS.with(|m| m.borrow_mut().remove(&id)) {
            wv.stop_loading();
        }
    });

    node
}

/// Navigate `web_view` to `src`. A string that looks like inline HTML
/// markup (first non-whitespace char is `<`) renders via `load_html`;
/// anything else is treated as a URI. An empty string clears the view to
/// `about:blank` rather than triggering a spurious load error.
fn load_source(web_view: &webkit6::WebView, src: &str) {
    let trimmed = src.trim_start();
    if trimmed.is_empty() {
        web_view.load_uri("about:blank");
    } else if trimmed.starts_with('<') {
        // Inline HTML. `None` base URI resolves relative links against
        // `about:blank`; authors embedding assets should use absolute URLs.
        web_view.load_html(src, None);
    } else {
        web_view.load_uri(src);
    }
}

// =========================================================================
// Imperative ops
// =========================================================================

struct LinuxWebViewOps;

impl WebViewOps for LinuxWebViewOps {
    fn reload(&self, node: &dyn Any) {
        with_web_view(node, |wv| wv.reload());
    }

    /// Native → page postMessage. Dispatches a synthetic `MessageEvent`
    /// on the page's `window`, so page authors can use
    /// `window.addEventListener('message', ...)` the same way they would
    /// on the web leaf. Fire-and-forget: `evaluate_javascript`'s result
    /// callback is a no-op.
    fn post_message(&self, node: &dyn Any, msg: &str) {
        with_web_view(node, |wv| {
            let escaped = escape_for_js_string(msg);
            let js = format!(
                "window.dispatchEvent(new MessageEvent('message', \
                     {{ data: \"{}\", source: window }}));",
                escaped
            );
            wv.evaluate_javascript(&js, None, None, gio::Cancellable::NONE, |_res| {});
        });
    }

    // `execute_js` stays the trait-default Err — WebKitGTK's
    // `evaluate_javascript` is callback-only and can't be made synchronous
    // without blocking the GTK main loop, which we refuse. Same posture as
    // the iOS leaf's WKWebView.
}

/// Look up the `webkit6::WebView` for `node` and run `f` against it. No-op
/// when the node isn't a `LinuxNode` or isn't in the table (already
/// unmounted, or an id miss).
fn with_web_view(node: &dyn Any, f: impl FnOnce(&webkit6::WebView)) {
    let Some(ln) = node.downcast_ref::<LinuxNode>() else {
        return;
    };
    let id = ln.id();
    WEBVIEWS.with(|m| {
        if let Some(wv) = m.borrow().get(&id) {
            f(wv);
        }
    });
}

/// Escape a Rust string for embedding inside a JS double-quoted string
/// literal — the four characters that would otherwise break the literal.
/// Callers needing arbitrary structured payloads should `JSON.stringify`
/// on the page side rather than rely on this for safety. Identical to the
/// iOS leaf's helper so the outbound-message escaping is uniform.
fn escape_for_js_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::escape_for_js_string;

    #[test]
    fn escapes_js_string_breaking_chars() {
        assert_eq!(escape_for_js_string(r#"a"b"#), r#"a\"b"#);
        assert_eq!(escape_for_js_string("a\\b"), "a\\\\b");
        assert_eq!(escape_for_js_string("a\nb"), "a\\nb");
        assert_eq!(escape_for_js_string("a\rb"), "a\\rb");
        assert_eq!(escape_for_js_string("plain"), "plain");
    }
}
