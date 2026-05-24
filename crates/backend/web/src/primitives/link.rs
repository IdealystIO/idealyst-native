//! `Primitive::Link` — a real `<a href>` element.
//!
//! The `<a>` is what makes web link semantics work without
//! re-implementation: hover URL preview in the status bar,
//! right-click "copy link," middle-click / cmd-click "open in
//! new tab," screen-reader "link" role, search-engine
//! crawlability. The framework provides the activation hook;
//! the browser does the rest.
//!
//! Click semantics:
//! - **Plain click (left mouse, no modifiers)** → `preventDefault`
//!   + fire `on_activate`. Keeps the SPA single-page.
//! - **Modified click** (cmd/ctrl/shift/alt, middle button, etc.)
//!   → fall through to the browser's default. "Open in new tab"
//!   etc. still work.
//!
//! The href reflects the framework's pre-computed URL (e.g.
//! `/detail/42`). On native, the same primitive renders without
//! a URL — `<a>` is web-only.

use crate::WebBackend;
use runtime_core::primitives::link::{LinkConfig, LinkHandle, LinkOps};
use std::any::Any;
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::Node;

pub(crate) fn create(b: &mut WebBackend, config: LinkConfig) -> Node {
    let anchor = b
        .doc
        .create_element("a")
        .expect("create anchor")
        .unchecked_into::<web_sys::HtmlAnchorElement>();
    anchor.set_href(&config.url);

    // The default anchor styles (blue, underlined) are usually the
    // wrong thing for app-shaped links — most authors want the
    // content's styling to show through. Reset color and decoration
    // here so the wrapping `Text` / `View` styles win out.
    // Authors who actually want anchor defaults can override via
    // their own style.
    //
    // Inline style via `set_attribute` rather than the typed
    // `.style()` API avoids needing the `CssStyleDeclaration`
    // web-sys feature. The framework applies its own classes on top
    // via `apply_style`; this inline reset is below those in
    // specificity terms when stylesheets target via class.
    let _ = anchor.set_attribute(
        "style",
        "color: inherit; text-decoration: none; display: inline-flex;",
    );

    let on_activate = config.on_activate.clone();
    let closure = Closure::<dyn FnMut(web_sys::MouseEvent)>::new(move |evt: web_sys::MouseEvent| {
        // Modified clicks fall through to the browser. Plain
        // left-click is what we intercept for SPA navigation.
        if evt.button() != 0
            || evt.meta_key()
            || evt.ctrl_key()
            || evt.shift_key()
            || evt.alt_key()
        {
            return;
        }
        evt.prevent_default();
        on_activate();
    });
    anchor
        .add_event_listener_with_callback("click", closure.as_ref().unchecked_ref())
        .expect("attach link click listener");
    // Stash so the closure lives as long as the WebBackend. The
    // anchor itself is held by the layout tree; when its scope
    // drops the node detaches but the closure handle stays in this
    // pool. For long-lived apps this would leak; for the framework's
    // current posture (Owner lifetime ≈ app lifetime) it's fine.
    b._link_click_closures.push(closure);

    // Hold the on_activate Rc separately so `make_link_handle` can
    // reach it through the node's data attribute (see below). The
    // simpler path: stash the Rc keyed by the anchor's identity in
    // a per-backend map. For v1 we don't support imperative
    // activation; the no-op `LinkOps` is enough.

    anchor.unchecked_into::<Node>()
}

/// `Ref<LinkHandle>` support. The minimal-correct path is to
/// expose `activate()` as `anchor.click()` — the browser fires a
/// synthetic click that our click listener intercepts. That
/// reproduces the exact semantics of a real user click without
/// the framework having to remember the `on_activate` Rc per
/// link.
pub(crate) fn make_handle(node: &Node) -> LinkHandle {
    let html: web_sys::HtmlElement = node
        .clone()
        .dyn_into()
        .expect("link node is not an HtmlElement");
    LinkHandle::new(Rc::new(html), &WebLinkOps)
}

struct WebLinkOps;
impl LinkOps for WebLinkOps {
    fn activate(&self, node: &dyn Any) {
        if let Some(html) = node.downcast_ref::<web_sys::HtmlElement>() {
            html.click();
        }
    }
}
