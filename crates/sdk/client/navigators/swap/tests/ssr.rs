//! SSR integration for the swap navigator: render-at-path with author
//! layout chrome + SSG route discovery. A server-rendered page carries
//! the real navigation chrome (tab bar / drawer panel — plain author
//! views here) and the resolved screen in the outlet.
//!
//! This is the only place SSR is exercised THROUGH the SDK's authored
//! surface rather than against the vocabulary navigator handler
//! directly, so it guards the whole chain: fluent SDK builder →
//! vocabulary `swap_navigator()` builder → navigator handler → route
//! collector → `render_all` crawl.

#![cfg(not(target_arch = "wasm32"))]

use backend_ssr::newcore::{render_all, render_path_with};
use runtime_shared::primitives::navigator::{enable_route_collector, take_route_collector};
use runtime_vocabulary::builders::{text, view};
use runtime_vocabulary::glue::{Element, IntoElement};
use swap_navigator::{Route, Screen, SwapBuilder, SwapNavigator};

const HOME: Route<()> = Route::<()>::new("home", "/");
const SETTINGS: Route<()> = Route::<()>::new("settings", "/settings");

fn app() -> Element {
    SwapNavigator::new(&HOME)
        .screen(HOME, |_| {
            Screen::new(view().child(text().content("HOME CONTENT")).build())
        })
        .screen(SETTINGS, |_| {
            Screen::new(view().child(text().content("SETTINGS CONTENT")).build())
        })
        .layout(|nav| {
            view()
                .child(nav.outlet)
                .child(text().content("TAB BAR"))
                .build()
        })
        .into_element()
}

/// The requested URL mounts its own screen inside the author layout;
/// sibling screens are NOT rendered (swap shows one at a time).
#[test]
fn render_path_mounts_matched_screen_inside_author_layout() {
    let html = render_path_with("/settings", |_| {}, app).html;

    assert!(
        html.contains("SETTINGS CONTENT"),
        "deep-linked screen rendered: {html}"
    );
    assert!(html.contains("TAB BAR"), "author layout chrome rendered: {html}");
    assert!(
        !html.contains("HOME CONTENT"),
        "inactive sibling not rendered: {html}"
    );
}

/// SSG discovery: one render publishes every registered screen path.
#[test]
fn route_collector_publishes_every_screen_path() {
    enable_route_collector();
    let _ = render_path_with("/", |_| {}, app);
    let mut found = take_route_collector().expect("collector was enabled");
    found.sort();
    assert_eq!(found, vec!["/", "/settings"]);
}

/// SSG end-to-end: `render_all` crawls every literal screen path.
#[test]
fn render_all_crawls_every_literal_screen() {
    let result = render_all(|_| {}, app);

    let mut paths: Vec<_> = result.pages.keys().cloned().collect();
    paths.sort();
    assert_eq!(paths, vec!["/", "/settings"]);
    assert!(result.pages["/"].html.contains("HOME CONTENT"));
    assert!(result.pages["/settings"].html.contains("SETTINGS CONTENT"));
}
