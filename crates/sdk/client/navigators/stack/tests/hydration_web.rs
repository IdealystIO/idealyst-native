//! SSR-hydration for the outlet-model stack: the client mounts over
//! server-rendered HTML and the initial screen is BUILT exactly once
//! (the walker's `attach_initial` build; the handler's deferred layout
//! microtask seats that same screen instead of mounting a second copy).
//!
//! Mirrors the legacy `stack-navigator/tests/hydration_web.rs` contract
//! so the outlet model can replace it without regressing hydration.
//! Browser-run (`wasm_bindgen_test_configure!(run_in_browser)`) — run
//! with `wasm-pack test --headless --chrome`.

#![cfg(target_arch = "wasm32")]

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use backend_web::WebBackend;
use runtime_core::primitives::navigator::Screen;
use runtime_core::{text, view, IntoElement, Route};
use stack_navigator::{StackBuilder, StackNavigator, StackScreenExt};
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

const HOME: Route<()> = Route::<()>::new("home", "/");

const MARKER: &str = "HYDRATE-SCREEN-MARKER";

fn app_tree(builds: Rc<Cell<u32>>) -> runtime_core::Element {
    StackNavigator::new(&HOME)
        .screen(HOME, move |_| {
            builds.set(builds.get() + 1);
            Screen::new(view(vec![text(MARKER).into()])).title("Home")
        })
        .layout(|nav| {
            view(vec![text("HEADER CHROME").into_element(), nav.outlet]).into_element()
        })
        .into()
}

#[wasm_bindgen_test]
fn initial_screen_not_duplicated_under_hydration() {
    // 1. Server render (same tree, generic SSR registration).
    let ssr_builds = Rc::new(Cell::new(0u32));
    let ssr_html = {
        let b = ssr_builds.clone();
        backend_ssr::render_path_with(
            "/",
            |bk| stack_navigator::register_generic(bk),
            move || app_tree(b.clone()),
        )
        .html
    };
    assert!(ssr_html.contains(MARKER), "SSR rendered the screen: {ssr_html}");
    assert!(ssr_html.contains("HEADER CHROME"), "SSR rendered the chrome: {ssr_html}");

    // 2. Inject as the pre-rendered document; force the URL to "/" so
    //    the client's cold-start resolution matches the server's.
    let win = web_sys::window().unwrap();
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

    // 3. Hydrate + mount the SAME tree; count client-side screen builds.
    let client_builds = Rc::new(Cell::new(0u32));
    backend_web::install_scheduler();
    let mut backend = WebBackend::hydrate("#app");
    stack_navigator::register(&mut backend);
    let rc = Rc::new(RefCell::new(backend));
    backend_web::install_global_self(&rc);
    let builds = client_builds.clone();
    let _owner = runtime_core::mount(rc, move || app_tree(builds.clone()));

    assert_eq!(
        client_builds.get(),
        1,
        "initial screen must BUILD exactly once under hydration — the \
         walker's attach_initial build is authoritative and the handler's \
         deferred seating must reuse it, not mount a second copy"
    );

    // The visible tree has exactly one marker + the chrome.
    let body_text = doc.body().unwrap().text_content().unwrap_or_default();
    let marker_count = body_text.matches(MARKER).count();
    assert_eq!(marker_count, 1, "one screen copy in the DOM, got: {body_text}");
}
