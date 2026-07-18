//! Web hydration regression: a COLD DEEP LINK (sub-route) must hydrate the
//! screen the server rendered — not the navigator's hardcoded `initial`.
//!
//! Bug: on web, in local-render mode, the walker resolves the initial screen
//! from `peek_initial_path()`, which is `None` on web (the platform URL is
//! read in the SDK handler layer, not here). So it falls back to `initial`
//! (home) and — under hydration — builds HOME while adopting the server's
//! TARGET DOM in place. The two screens don't match, so the target's first
//! reactive-text nodes (title/intro) diverge and get dropped, and the
//! home/target DOM cross-mixes. The create-time microtask that would have
//! mounted the target is skipped under hydration (see hydration_web.rs), so
//! the target never renders correctly at all.
//!
//! SSR renders ONLY the target screen for a deep link (no home-underneath —
//! see tests/ssr.rs `render_path_mounts_matched_navigator_screen`). So the
//! fix is: under hydration on web, seed `set_initial_path(location.pathname)`
//! so the synchronous walker resolves and adopts the TARGET screen, matching
//! the server DOM exactly.
//!
//! This test SSR-renders the navigator at `/contact` (a reactive-text screen),
//! injects it, points `history` at `/contact`, then drives a real
//! `WebBackend::hydrate` + `runtime_core::mount` in headless Chrome and asserts
//! the target's reactive text survives hydration exactly once (and that home's
//! marker does NOT appear).

#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::rc::Rc;

use backend_web::WebBackend;
use runtime_core::primitives::navigator::Screen;
use runtime_core::{rx, signal, text, view, Route};
use stack_navigator::{Navigator, StackBuilder, StackScreenExt};
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

const HOME: Route<()> = Route::<()>::new("home", "/");
const CONTACT: Route<()> = Route::<()>::new("contact", "/contact");

// Home renders a plain marker; contact renders a REACTIVE (i18n-shaped) text
// node — `text(rx!(...))` → `TextSource::Bound` — the exact shape that dropped
// on the portfolio's sub-routes.
const HOME_MARKER: &str = "HOME-DEEPLINK-MARKER";
const CONTACT_MARKER: &str = "CONTACT-REACTIVE-MARKER";

fn app_tree() -> runtime_core::Element {
    Navigator::new(&HOME)
        .screen(HOME, |_| {
            Screen::new(view(vec![text(HOME_MARKER).into()]))
        })
        .screen(CONTACT, |_| {
            let sig = signal(CONTACT_MARKER.to_string());
            Screen::new(view(vec![text(rx!(sig.get())).into()]))
        })
        .into()
}

#[wasm_bindgen_test]
fn deep_link_target_reactive_text_survives_hydration() {
    // 1. Render the SSR HTML for the navigator AT THE DEEP LINK "/contact".
    //    The chrome handler is registered (as the app's `register_ssr_extensions`
    //    does) so the container carries the structural
    //    `NAV_ROOT_HYDRATION_CLASS` marker for client adoption.
    let ssr_html = backend_ssr::render_path_with(
        "/contact",
        |bk| stack_navigator::chrome::register(bk),
        app_tree,
    )
    .html;
    assert!(
        ssr_html.contains(CONTACT_MARKER),
        "SSR at /contact should render the target's reactive text: {ssr_html}"
    );
    assert!(
        !ssr_html.contains(HOME_MARKER),
        "SSR at /contact must NOT render home (no home-underneath): {ssr_html}"
    );

    // 2. Point the platform URL at the deep link, then inject the SSR HTML as
    //    the document the client hydrates. `current_pathname()` in the helper
    //    reads this to seed the walker's initial-path resolution.
    let win = web_sys::window().unwrap();
    win.history()
        .unwrap()
        .replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some("/contact"))
        .ok();
    let doc = win.document().unwrap();
    if let Some(existing) = doc.get_element_by_id("app") {
        existing.remove();
    }
    let app = doc.create_element("div").unwrap();
    app.set_id("app");
    app.set_inner_html(&ssr_html);
    doc.body().unwrap().append_child(&app).unwrap();

    // 3. Hydrate + mount the SAME tree.
    backend_web::install_scheduler();
    let mut backend = WebBackend::hydrate("#app");
    stack_navigator::register(&mut backend);
    let rc = Rc::new(RefCell::new(backend));
    backend_web::install_global_self(&rc);
    let _owner = runtime_core::mount(rc, app_tree);

    // The target's reactive text must survive hydration exactly once — not
    // dropped by a divergence remount, not duplicated. Before the fix the
    // walker builds HOME (initial fallback) and adopts contact's DOM as home,
    // so the contact reactive text diverges and is dropped.
    let html = app.inner_html();
    let contact_count = html.matches(CONTACT_MARKER).count();
    assert_eq!(
        contact_count, 1,
        "deep-link target reactive text must survive hydration exactly once \
         (walker must resolve the TARGET, not fall back to `initial`=home) \
         — got {contact_count} in {html}"
    );
    // Home must not appear: SSR rendered only the target, so a correctly
    // hydrated deep link shows only the target screen.
    assert_eq!(
        html.matches(HOME_MARKER).count(),
        0,
        "home must NOT render on a deep-link hydration — only the target: {html}"
    );
}
