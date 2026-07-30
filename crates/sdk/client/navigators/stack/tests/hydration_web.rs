//! SSR-hydration for the outlet-model stack: the client mounts over
//! server-rendered HTML and the initial screen is BUILT exactly once
//! (the navigator's initial seating must reuse the adopted screen
//! instead of mounting a second copy).
//!
//! This is the ONLY test in the repo that hydrates a NAVIGATOR over SSR
//! HTML. The frozen SSR/SSG corpora prove the server *output* is
//! byte-identical — which is the precondition for adoption — but they do
//! not exercise the client adopt path at all.
//!
//! Browser-run (`wasm_bindgen_test_configure!(run_in_browser)`) — run
//! with `wasm-pack test --headless --chrome`.

#![cfg(target_arch = "wasm32")]

use std::cell::Cell;
use std::rc::Rc;

use runtime_vocabulary::builders::{text, view};
use runtime_vocabulary::glue::{Element, IntoElement};
use stack_navigator::{Route, Screen, StackBuilder, StackNavigator, StackScreenExt};
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

const HOME: Route<()> = Route::<()>::new("home", "/");

const MARKER: &str = "HYDRATE-SCREEN-MARKER";

fn app_tree(builds: Rc<Cell<u32>>) -> Element {
    StackNavigator::new(&HOME)
        .screen(HOME, move |_| {
            builds.set(builds.get() + 1);
            Screen::new(view().child(text().content(MARKER)).build()).title("Home")
        })
        .layout(|nav| {
            view()
                .child(text().content("HEADER CHROME"))
                .child(nav.outlet)
                .build()
        })
        .into_element()
}

#[wasm_bindgen_test]
fn initial_screen_not_duplicated_under_hydration() {
    // 1. Server render (the same tree; the navigator handler is a
    //    vocabulary built-in, so the register seam is empty).
    let ssr_builds = Rc::new(Cell::new(0u32));
    let ssr_html = {
        let b = ssr_builds.clone();
        backend_ssr::newcore::render_path_with("/", |_| {}, move || app_tree(b.clone())).html
    };
    assert!(ssr_html.contains(MARKER), "SSR rendered the screen: {ssr_html}");
    assert!(
        ssr_html.contains("HEADER CHROME"),
        "SSR rendered the chrome: {ssr_html}"
    );

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
    //    `hydrate_in` is the adopt-mode boot entry (it falls back to a
    //    fresh boot when the mount has no server DOM, which is exactly
    //    what step 2 guarantees against).
    let client_builds = Rc::new(Cell::new(0u32));
    {
        let builds = client_builds.clone();
        backend_web::newcore::hydrate_in("#app", |_| {}, move || app_tree(builds.clone()));
    }

    assert_eq!(
        client_builds.get(),
        1,
        "initial screen must BUILD exactly once under hydration — the \
         adopted server screen is authoritative and the navigator's \
         initial seating must reuse it, not mount a second copy"
    );

    // The visible tree has exactly one marker + the chrome.
    let body_text = doc.body().unwrap().text_content().unwrap_or_default();
    let marker_count = body_text.matches(MARKER).count();
    assert_eq!(marker_count, 1, "one screen copy in the DOM, got: {body_text}");
}
