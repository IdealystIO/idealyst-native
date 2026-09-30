//! Web platform helper.
//!
//! On the web there is no OS "open URL" event for a custom scheme — the
//! app's entry URL *is* the deep link, available synchronously as
//! `window.location.href`. We read it on bootstrap to seed
//! [`crate::initial_link`]. Subsequent in-app navigations / `popstate`
//! events are fed back through [`crate::feed_link`] by the host (the same
//! place the navigator SDK hooks `popstate`).
//!
//! The browser calls are web-glue bindings declared here (own-web-bindings
//! phase 3), each one JS expression.

use web_glue::string;

web_glue::import! {
    // 1 and `location.href` written to `out`, or 0 without a window.
    fn js_href(out: usize) -> u32 =
        "(o) => { if (typeof window === 'undefined') return 0; G.retStr(window.location.href, o); return 1; }";
    // 1 and `location.origin` written to `out`, or 0 without a window.
    fn js_origin(out: usize) -> u32 =
        "(o) => { if (typeof window === 'undefined') return 0; G.retStr(window.location.origin, o); return 1; }";
    // `replaceState` throws a SecurityError for a URL on another origin;
    // that is swallowed by the caller, exactly as before.
    #[catch]
    fn js_replace_url(p: usize, l: usize) =
        "(p, l) => { if (typeof window === 'undefined') return; \
           const h = window.history; h.replaceState(h.state, '', G.str(p, l)); }";
}

/// A string the snippet wrote into the out-slot, if it reported one.
fn read(f: impl FnOnce(usize) -> u32) -> Option<String> {
    let mut hit = 0;
    let s = string::receive(|o| hit = f(o));
    (hit != 0).then_some(s)
}

/// The current document URL (`window.location.href`), or `None` if there
/// is no `window` (e.g. a worker / SSR context).
pub(crate) fn current_href() -> Option<String> {
    read(|o| unsafe { js_href(o) })
}

/// `window.location.origin` (`scheme://host[:port]`), or `None` without a
/// `window`. The browser reports the literal string `"null"` for opaque
/// origins (`file:`, sandboxed frames); that is not an origin anyone can
/// build a URL on, so it maps to `None` too.
pub(crate) fn origin() -> Option<String> {
    let origin = read(|o| unsafe { js_origin(o) })?;
    (!origin.is_empty() && origin != "null").then_some(origin)
}

/// `history.replaceState(<current state>, "", url)`.
///
/// The CURRENT `history.state` is passed back rather than `null`: the
/// entry may carry state someone else put there (a navigator's
/// bookkeeping, a router's scroll record), and this call exists to
/// change the address, not to wipe what the entry remembers.
pub(crate) fn replace_url(url: &str) {
    let (p, l) = string::abi(url);
    let _ = unsafe { js_replace_url(p, l) };
}
