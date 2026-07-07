//! Web hydration regression: the navigator's INITIAL screen must be built
//! exactly ONCE during SSR hydration — not twice.
//!
//! Bug: in local-render mode the walker builds the initial screen and hands
//! it to `navigator_attach_initial` (which, under hydration, adopts the
//! server's screen DOM in place), AND a create-time microtask separately
//! auto-mounts the initial screen. Off hydration the `attach_initial` build is
//! a discarded throwaway; but under hydration the throwaway build ADOPTS the
//! SSR DOM while the microtask builds a FRESH copy — so the whole screen
//! (chrome, nav, content) renders twice.
//!
//! Fix (`helpers/web` + `backend_web::is_hydrating`): during hydration the
//! walker's `attach_initial` is the authoritative, already-adopted mount and
//! the create-time microtask skips.
//!
//! The test renders the navigator's SSR HTML via `backend-ssr`, injects it
//! into `#app`, then drives a real `WebBackend::hydrate` + `runtime_core::mount`
//! in a headless browser and asserts the initial screen is BUILT exactly once.
//!
//! Scope note: this is an INTEGRATION smoke test for the navigator hydration
//! path — it exercises `hydrate → mount → drain(microtask) → finish` end to end
//! (and caught a cyclic-insert crash during development). It does not, on its
//! own, reproduce the full-app duplication (that needs the app's exact
//! structure/URL/adoption); the strict behavioral 2→1 guard is the e2e
//! (Playwright) check against the built site.

#![cfg(target_arch = "wasm32")]

use std::cell::Cell;
use std::cell::RefCell;
use std::rc::Rc;

use backend_web::WebBackend;
use runtime_core::primitives::navigator::Screen;
use runtime_core::{text, view, Route};
use stack_navigator::{Navigator, StackBuilder, StackScreenExt};
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

const HOME: Route<()> = Route::<()>::new("home", "/");

// A marker only the initial screen renders, so we can count screen copies.
const MARKER: &str = "HYDRATE-SCREEN-MARKER";

fn app_tree(builds: Rc<Cell<u32>>) -> runtime_core::Element {
    Navigator::new(&HOME)
        .screen(HOME, move |_| {
            builds.set(builds.get() + 1);
            Screen::new(view(vec![text(MARKER).into()]))
        })
        .into()
}

#[wasm_bindgen_test]
fn initial_screen_not_duplicated_under_hydration() {
    // 1. Render the SSR HTML for the navigator at "/" (faithful structure).
    let ssr_builds = Rc::new(Cell::new(0u32));
    let ssr_html = {
        let b = ssr_builds.clone();
        // Register the navigator's SSR chrome handler — same as the app's
        // `register_ssr_extensions` — so the container carries `ui-nav-root`
        // and the client adopts it (rather than rebuilding fresh).
        backend_ssr::render_path_with(
            "/",
            |bk| stack_navigator::chrome::register(bk),
            move || app_tree(b.clone()),
        )
        .html
    };
    assert!(
        ssr_html.contains(MARKER),
        "SSR should render the initial screen once: {ssr_html}"
    );

    // 2. Inject it as the pre-rendered document the client hydrates.
    let win = web_sys::window().unwrap();
    // The create-time auto-mount microtask is URL-driven (it mounts the initial
    // route only when `current_pathname() == "/"`); the wasm-test page URL
    // isn't "/", so force it so the microtask path is actually exercised.
    win.history()
        .unwrap()
        .replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some("/"))
        .ok();
    let doc = win.document().unwrap();
    if let Some(existing) = doc.get_element_by_id("app") {
        existing.remove();
    }
    let app = doc.create_element("div").unwrap();
    app.set_id("app");
    app.set_inner_html(&ssr_html);
    doc.body().unwrap().append_child(&app).unwrap();

    // 3. Hydrate + mount the SAME tree. Count client-side screen builds only.
    let client_builds = Rc::new(Cell::new(0u32));
    backend_web::install_scheduler();
    let mut backend = WebBackend::hydrate("#app");
    stack_navigator::register(&mut backend); // inventory submit isn't retained in a test bin
    let rc = Rc::new(RefCell::new(backend));
    backend_web::install_global_self(&rc);
    let builds = client_builds.clone();
    let _owner = runtime_core::mount(rc, move || app_tree(builds.clone()));

    // `mount` runs the walker pass AND drains the buffered create-time
    // microtask inside the adoption window. This is the invariant the fix
    // guarantees: the initial screen is BUILT exactly once under hydration.
    // Before the fix it builds twice — the walker's `attach_initial` (which
    // adopts the SSR screen in place) AND the create-time microtask (fresh) —
    // which is what duplicates the whole screen tree. After the fix the
    // microtask skips during hydration and `attach_initial` is authoritative.
    assert_eq!(
        client_builds.get(),
        1,
        "initial screen must BUILD exactly once under hydration, not twice \
         (walker attach_initial is authoritative; create-time microtask skips) \
         — got {} builds",
        client_builds.get()
    );
}
