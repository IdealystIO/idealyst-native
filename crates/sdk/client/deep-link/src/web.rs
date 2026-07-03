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
