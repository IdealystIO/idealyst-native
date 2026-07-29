//! SSR integration for the swap navigator: render-at-path with author
//! layout chrome + SSG route discovery. A server-rendered page carries
//! the real navigation chrome (tab bar / drawer panel — plain author
//! views here) and the walker-resolved screen in the outlet.

// Old-core suite: the `new-core` feature swaps the crate to the
// vocabulary-backed surface (mutually exclusive names) — these tests
// exercise the old walker/registry path only.
#![cfg(not(feature = "new-core"))]

#![cfg(not(target_arch = "wasm32"))]

use backend_ssr::{render_all, render_path_with};
use runtime_core::primitives::navigator::{
    enable_route_collector, take_route_collector, Screen,
};
use runtime_core::{text, view, IntoElement, Route};
use swap_navigator::{SwapBuilder, SwapNavigator};

const HOME: Route<()> = Route::<()>::new("home", "/");
const SETTINGS: Route<()> = Route::<()>::new("settings", "/settings");

fn app() -> runtime_core::Element {
    SwapNavigator::new(&HOME)
        .screen(HOME, |_| Screen::new(view(vec![text("HOME CONTENT").into()])))
        .screen(SETTINGS, |_| Screen::new(view(vec![text("SETTINGS CONTENT").into()])))
        .layout(|nav| view(vec![nav.outlet, text("TAB BAR").into_element()]).into_element())
        .into()
}

/// The requested URL mounts its own screen inside the author layout;
/// sibling screens are NOT rendered (swap shows one at a time).
#[test]
fn render_path_mounts_matched_screen_inside_author_layout() {
    let html = render_path_with(
        "/settings",
        |b| swap_navigator::register_generic(b),
        app,
    )
    .html;

    assert!(html.contains("SETTINGS CONTENT"), "deep-linked screen rendered: {html}");
    assert!(html.contains("TAB BAR"), "author layout chrome rendered: {html}");
    assert!(!html.contains("HOME CONTENT"), "inactive sibling not rendered: {html}");
}

/// SSG discovery: one render publishes every registered screen path.
#[test]
fn route_collector_publishes_every_screen_path() {
    enable_route_collector();
    let _ = render_path_with("/", |b| swap_navigator::register_generic(b), app);
    let mut found = take_route_collector().expect("collector was enabled");
    found.sort();
    assert_eq!(found, vec!["/", "/settings"]);
}

/// SSG end-to-end: `render_all` crawls every literal screen path.
#[test]
fn render_all_crawls_every_literal_screen() {
    let result = render_all(|b| swap_navigator::register_generic(b), app);

    let mut paths: Vec<_> = result.pages.keys().cloned().collect();
    paths.sort();
    assert_eq!(paths, vec!["/", "/settings"]);
    assert!(result.pages["/"].html.contains("HOME CONTENT"));
    assert!(result.pages["/settings"].html.contains("SETTINGS CONTENT"));
}
