//! SSR integration for the outlet-model stack navigator: render-at-path,
//! author layout chrome, SSG route discovery.
//!
//! This is the only place SSR is exercised THROUGH the SDK's authored
//! surface rather than against the vocabulary navigator handler
//! directly, so it guards the whole chain: fluent SDK builder →
//! vocabulary `stack_navigator()` builder → navigator handler → route
//! collector → `render_all` crawl.

#![cfg(not(target_arch = "wasm32"))]

use backend_ssr::newcore::{render_all, render_path_with};
use runtime_shared::primitives::navigator::{enable_route_collector, take_route_collector};
use runtime_vocabulary::builders::{text, view};
use runtime_vocabulary::glue::{Element, IntoElement};
use stack_navigator::{Route, Screen, StackBuilder, StackNavigator, StackScreenExt};

const HOME: Route<()> = Route::<()>::new("home", "/");
const ABOUT: Route<()> = Route::<()>::new("about", "/about");
const CONTACT: Route<()> = Route::<()>::new("contact", "/contact");

fn app() -> Element {
    StackNavigator::new(&HOME)
        .screen(HOME, |_| {
            Screen::new(view().child(text().content("HOME PAGE")).build()).title("Home")
        })
        .screen(ABOUT, |_| {
            Screen::new(view().child(text().content("ABOUT PAGE")).build()).title("About")
        })
        .screen(CONTACT, |_| {
            Screen::new(view().child(text().content("CONTACT PAGE")).build()).title("Contact")
        })
        // Author chrome: a persistent header marker above the outlet.
        .layout(|nav| {
            view()
                .child(text().content("HEADER CHROME"))
                .child(nav.outlet)
                .build()
        })
        .into_element()
}

/// The requested URL mounts its own screen (not the configured initial),
/// wrapped in the author layout chrome.
#[test]
fn render_path_mounts_matched_screen_inside_author_layout() {
    let html = render_path_with("/about", |_| {}, app).html;

    assert!(html.contains("ABOUT PAGE"), "deep-linked screen rendered: {html}");
    assert!(
        html.contains("HEADER CHROME"),
        "author layout chrome rendered: {html}"
    );
    assert!(
        !html.contains("HOME PAGE"),
        "home must not render at /about (the synthesized index stays \
         cold until a pop): {html}"
    );
}

/// SSG discovery: one render publishes every registered screen path.
#[test]
fn route_collector_publishes_every_screen_path() {
    enable_route_collector();
    let _ = render_path_with("/", |_| {}, app);
    let mut found = take_route_collector().expect("collector was enabled");
    found.sort();
    assert_eq!(found, vec!["/", "/about", "/contact"]);
}

/// SSG end-to-end: `render_all` crawls every literal screen path.
#[test]
fn render_all_crawls_every_literal_screen() {
    let result = render_all(|_| {}, app);

    let mut paths: Vec<_> = result.pages.keys().cloned().collect();
    paths.sort();
    assert_eq!(paths, vec!["/", "/about", "/contact"]);
    assert!(result.pages["/"].html.contains("HOME PAGE"));
    assert!(result.pages["/about"].html.contains("ABOUT PAGE"));
    assert!(result.pages["/contact"].html.contains("CONTACT PAGE"));
}
