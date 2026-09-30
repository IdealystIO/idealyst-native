//! Pure wasm32 DOM helpers for the web leg (no core types): the iframe
//! event-listener wiring and the imperative `WebViewOps` bodies. Kept out
//! of `lib.rs` so the DOM plumbing stays separable from the primitive's
//! core-facing surface. DOM access is web-glue; the host node the ops
//! receive IS a `web_glue::dom::Node`.

use std::any::Any;
use std::rc::Rc;

use web_glue::dom::{Element, EventTarget, HtmlIFrameElement, Listener, ListenerOptions, Node};
use web_glue::{string, JsCast};

web_glue::import! {
    /// `message` from THIS iframe: write the payload (a string as-is,
    /// anything else JSON-stringified, `""` if that fails) and return 1;
    /// any other source returns 0. The filter is `event.source ===
    /// iframe.contentWindow`, so sibling iframes never reach the callback.
    fn js_iframe_message(ev: u32, iframe: u32, out: usize) -> u32 =
        "(e, f, o) => { const ev = G.get(e), w = G.get(f).contentWindow; \
           if (ev.source == null || w == null || ev.source !== w) return 0; \
           const d = ev.data; let s = ''; \
           if (typeof d === 'string') s = d; \
           else { try { const j = JSON.stringify(d); if (typeof j === 'string') s = j; } catch (_) {} } \
           G.retStr(s, o); return 1; }";

    /// `iframe.contentWindow.postMessage(msg, "*")`; no window, no-op.
    fn js_post_message(iframe: u32, p: usize, l: usize) =
        "(f, p, l) => { const w = G.get(f).contentWindow; if (w) w.postMessage(G.str(p, l), '*'); }";

    /// Sync `eval` in the iframe's global scope. Returns 0 with the
    /// JSON-stringified result (`""` for `undefined`) written out, or an
    /// error code: 1 no contentWindow, 2 cross-origin (reading `eval`
    /// throws), 3 `eval` not callable, 4 result not JSON-stringifiable,
    /// 5 the code threw (the exception, JSON-stringified, written out).
    fn js_execute(iframe: u32, p: usize, l: usize, out: usize) -> u32 =
        "(f, p, l, o) => { const w = G.get(f).contentWindow; if (!w) return 1; \
           let ev; try { ev = w.eval; } catch (_) { return 2; } \
           if (typeof ev !== 'function') return 3; \
           let r; try { r = ev.call(w, G.str(p, l)); } catch (e) { \
             let s; try { s = JSON.stringify(e); } catch (_) {} \
             G.retStr(typeof s === 'string' ? s : '(non-stringifiable exception)', o); return 5; } \
           if (r === undefined) return 0; \
           let s; try { s = JSON.stringify(r); } catch (_) {} \
           if (typeof s !== 'string') return 4; \
           G.retStr(s, o); return 0; }";
}

// ============================================================================
// Event listener wiring
// ============================================================================

/// The author-callback listeners of one mounted iframe. Dropping it
/// detaches each listener and then frees its closure — the web handler
/// drops it at unmount (`on_teardown`).
pub(crate) struct WebViewListeners(#[allow(dead_code)] Vec<Listener>);

/// Wire the author's message/load/error callbacks as DOM event
/// listeners on `iframe` (the `message` one on `window`, filtered to this
/// iframe's `contentWindow`). The web handler calls this with the
/// flush-wrapped callbacks it wants fired and owns the result until
/// unmount.
pub(crate) fn wire_listeners(
    iframe: &Element,
    on_message: Option<Rc<dyn Fn(String)>>,
    on_load: Option<Rc<dyn Fn()>>,
    on_error: Option<Rc<dyn Fn()>>,
) -> WebViewListeners {
    let mut listeners = Vec::new();
    if let Some(cb) = on_message {
        if let Some(window) = web_glue::dom::window() {
            let frame = iframe.clone();
            listeners.push(Listener::new(
                window.into(),
                "message",
                ListenerOptions::default(),
                move |ev| {
                    let mut ours = 0;
                    let payload = string::receive(|o| {
                        // SAFETY: live handles; `o` is the out-slot.
                        ours = unsafe { js_iframe_message(ev.as_js().raw(), frame.as_js().raw(), o) };
                    });
                    if ours != 0 {
                        cb(payload);
                    }
                },
            ));
        }
    }
    let target: EventTarget = iframe.clone().into();
    if let Some(cb) = on_load {
        listeners.push(Listener::new(target.clone(), "load", ListenerOptions::default(), move |_| cb()));
    }
    if let Some(cb) = on_error {
        listeners.push(Listener::new(target, "error", ListenerOptions::default(), move |_| cb()));
    }
    WebViewListeners(listeners)
}

// ============================================================================
// Imperative ops bodies — the whole `WebViewOps` impl on web. Each
// takes the type-erased handle node (the `<iframe>` as a
// `web_glue::dom::Node`).
// ============================================================================

/// Downcast the type-erased handle node to the mounted `<iframe>`.
fn as_iframe(node: &dyn Any) -> Option<&HtmlIFrameElement> {
    node.downcast_ref::<Node>()?.dyn_ref::<HtmlIFrameElement>()
}

/// `WebViewOps::post_message` body: route to
/// `iframe.contentWindow.postMessage(msg, "*")`.
pub(crate) fn post_message(node: &dyn Any, msg: &str) {
    let Some(iframe) = as_iframe(node) else {
        return;
    };
    let (p, l) = string::abi(msg);
    // SAFETY: a live iframe handle and a borrowed string.
    unsafe { js_post_message(iframe.as_js().raw(), p, l) }
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
    let (p, l) = string::abi(code);
    let mut status = 0;
    let text = string::receive(|o| {
        // SAFETY: a live iframe handle, a borrowed string, the out-slot.
        status = unsafe { js_execute(iframe.as_js().raw(), p, l, o) };
    });
    match status {
        0 => Ok(text),
        1 => Err("iframe has no contentWindow".to_string()),
        2 => Err("iframe is cross-origin; eval is inaccessible".to_string()),
        3 => Err("iframe's `eval` is not callable".to_string()),
        4 => Err("result is not JSON-stringifiable".to_string()),
        _ => Err(text),
    }
}
