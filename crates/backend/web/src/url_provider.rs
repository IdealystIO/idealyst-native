//! Browser History-API provider for the navigator substrate's URL sync.
//!
//! Installs `runtime_shared::primitives::navigator::url_sync`'s platform
//! provider (pushState / replaceState / back / current path) and wires
//! the window `popstate` event to the substrate reconciler. Outlet-model
//! navigators (`swap-navigator`, `stack-navigator`) opt in via
//! `NavigatorControl::enable_url_sync()`; the legacy class-based
//! navigators keep their own helpers-crate URL machinery and ignore
//! this entirely.
//!
//! Also seeds `runtime_shared`'s initial-path slot from
//! `window.location.pathname` so the walker's cold-start deep-link
//! resolution (the same path SSR and native deep links use) mounts the
//! URL's screen instead of the configured initial. The walker clears
//! the slot once the root navigator subtree has mounted, and the legacy
//! web navigators discard the walker's initial build regardless, so the
//! seed is invisible to them.

use runtime_shared::primitives::navigator::{self as nav, UrlProvider};
use std::cell::Cell;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

thread_local! {
    static INSTALLED: Cell<bool> = const { Cell::new(false) };
    /// Keeps the popstate listener alive for the page's lifetime.
    static POPSTATE_LISTENER: std::cell::RefCell<Option<Closure<dyn FnMut(web_sys::Event)>>> =
        const { std::cell::RefCell::new(None) };
}

fn pathname() -> String {
    web_sys::window()
        .and_then(|w| w.location().pathname().ok())
        .unwrap_or_else(|| "/".to_string())
}

/// Install the browser URL provider + popstate wiring + initial-path
/// seed. Idempotent, safe headless (no `window` ⇒ no-op).
///
/// Called from the boot seam's `BuiltinSet::nav_services` closure
/// ([`crate::newcore::start_in_with`] /
/// [`crate::newcore_hydrate::hydrate_in_with`]), NOT from
/// `install_scheduler` — it used to ride the scheduler so every web host
/// got it for free, but the popstate listener below calls
/// `nav::handle_popstate`, which kept `NavigatorControl` reachable from
/// boot in bundles that had dropped the navigator primitives (10,827
/// bytes on a `--primitives view,text` hello-world). Behind the set, an
/// app without `nav` never names this fn at all.
pub fn install_url_provider() {
    if INSTALLED.with(|c| c.get()) {
        return;
    }
    let Some(window) = web_sys::window() else { return };
    INSTALLED.with(|c| c.set(true));

    nav::install_url_provider(UrlProvider {
        current_path: Box::new(pathname),
        push_state: Box::new(|url| {
            if let Some(w) = web_sys::window() {
                if let Ok(h) = w.history() {
                    let _ = h.push_state_with_url(
                        &wasm_bindgen::JsValue::NULL,
                        "",
                        Some(url),
                    );
                }
            }
        }),
        replace_state: Box::new(|url| {
            if let Some(w) = web_sys::window() {
                if let Ok(h) = w.history() {
                    let _ = h.replace_state_with_url(
                        &wasm_bindgen::JsValue::NULL,
                        "",
                        Some(url),
                    );
                }
            }
        }),
        history_back: Box::new(|| {
            if let Some(w) = web_sys::window() {
                if let Ok(h) = w.history() {
                    let _ = h.back();
                }
            }
        }),
    });

    // popstate → substrate reconciler with the (already-changed) path.
    let listener = Closure::wrap(Box::new(move |_e: web_sys::Event| {
        nav::handle_popstate(&pathname());
    }) as Box<dyn FnMut(web_sys::Event)>);
    let _ = window
        .add_event_listener_with_callback("popstate", listener.as_ref().unchecked_ref());
    POPSTATE_LISTENER.with(|slot| *slot.borrow_mut() = Some(listener));

    // Cold-start deep-link seed for the walker's initial resolution.
    nav::set_initial_path(Some(pathname()));
}
