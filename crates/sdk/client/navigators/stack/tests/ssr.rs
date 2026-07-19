//! SSR integration for the outlet-model stack navigator: render-at-path,
//! author layout chrome, SSG route discovery. Mirrors the legacy
//! `stack-navigator/tests/ssr.rs` contract so the outlet model can
//! replace it without losing server rendering.
//!
//! The handler defers its author-layout build (and initial-screen
//! seating) to a microtask; the SSR backend's queuing scheduler drains
//! those inside `render_path`, so the emitted HTML carries both the
//! chrome and the walker-resolved screen.

#![cfg(not(target_arch = "wasm32"))]

use backend_ssr::{render_all, render_path_with};
use runtime_core::primitives::navigator::{
    enable_route_collector, take_route_collector, Screen,
};
use runtime_core::{text, view, IntoElement, Route};
use stack_navigator::{StackBuilder, StackNavigator, StackScreenExt};

const HOME: Route<()> = Route::<()>::new("home", "/");
const ABOUT: Route<()> = Route::<()>::new("about", "/about");
const CONTACT: Route<()> = Route::<()>::new("contact", "/contact");

fn app() -> runtime_core::Element {
    StackNavigator::new(&HOME)
        .screen(HOME, |_| Screen::new(view(vec![text("HOME PAGE").into()])).title("Home"))
        .screen(ABOUT, |_| Screen::new(view(vec![text("ABOUT PAGE").into()])).title("About"))
        .screen(CONTACT, |_| {
            Screen::new(view(vec![text("CONTACT PAGE").into()])).title("Contact")
        })
        // Author chrome: a persistent header marker above the outlet —
        // the outlet-model replacement for the legacy native header.
        .layout(|nav| {
            view(vec![text("HEADER CHROME").into_element(), nav.outlet]).into_element()
        })
        .into()
}

/// The requested URL mounts its own screen (not the configured initial),
/// wrapped in the author layout chrome.
#[test]
fn render_path_mounts_matched_screen_inside_author_layout() {
    let html = render_path_with(
        "/about",
        |b| stack_navigator::register_generic(b),
        app,
    )
    .html;

    assert!(html.contains("ABOUT PAGE"), "deep-linked screen rendered: {html}");
    assert!(html.contains("HEADER CHROME"), "author layout chrome rendered: {html}");
    assert!(
        !html.contains("HOME PAGE"),
        "home must not render at /about (Rebuild retention keeps the \
         synthesized index cold): {html}"
    );
}

/// SSG discovery: one render publishes every registered screen path.
#[test]
fn route_collector_publishes_every_screen_path() {
    enable_route_collector();
    let _ = render_path_with("/", |b| stack_navigator::register_generic(b), app);
    let mut found = take_route_collector().expect("collector was enabled");
    found.sort();
    assert_eq!(found, vec!["/", "/about", "/contact"]);
}

/// SSG end-to-end: `render_all` crawls every literal screen path.
#[test]
fn render_all_crawls_every_literal_screen() {
    let result = render_all(|b| stack_navigator::register_generic(b), app);

    let mut paths: Vec<_> = result.pages.keys().cloned().collect();
    paths.sort();
    assert_eq!(paths, vec!["/", "/about", "/contact"]);
    assert!(result.pages["/"].html.contains("HOME PAGE"));
    assert!(result.pages["/about"].html.contains("ABOUT PAGE"));
    assert!(result.pages["/contact"].html.contains("CONTACT PAGE"));
}
