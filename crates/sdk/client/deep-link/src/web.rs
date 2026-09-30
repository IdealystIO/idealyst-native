//! Web platform helper.
//!
//! On the web there is no OS "open URL" event for a custom scheme — the
//! app's entry URL *is* the deep link, available synchronously as
//! `window.location.href`. We read it on bootstrap to seed
//! [`crate::initial_link`]. Subsequent in-app navigations / `popstate`
//! events are fed back through [`crate::feed_link`] by the host (the same
//! place the navigator SDK hooks `popstate`).

/// The current document URL (`window.location.href`), or `None` if there
/// is no `window` (e.g. a worker / SSR context).
pub(crate) fn current_href() -> Option<String> {
    let win = web_sys::window()?;
    win.location().href().ok()
}

/// `window.location.origin` (`scheme://host[:port]`), or `None` without a
/// `window`. The browser reports the literal string `"null"` for opaque
/// origins (`file:`, sandboxed frames); that is not an origin anyone can
/// build a URL on, so it maps to `None` too.
pub(crate) fn origin() -> Option<String> {
    let origin = web_sys::window()?.location().origin().ok()?;
    (!origin.is_empty() && origin != "null").then_some(origin)
}

/// `history.replaceState(<current state>, "", url)`.
///
/// The CURRENT `history.state` is passed back rather than `null`: the
/// entry may carry state someone else put there (a navigator's
/// bookkeeping, a router's scroll record), and this call exists to
/// change the address, not to wipe what the entry remembers.
pub(crate) fn replace_url(url: &str) {
    let Some(history) = web_sys::window().and_then(|w| w.history().ok()) else {
        return;
    };
    let state = history.state().unwrap_or(web_sys::wasm_bindgen::JsValue::NULL);
    let _ = history.replace_state_with_url(&state, "", Some(url));
}
