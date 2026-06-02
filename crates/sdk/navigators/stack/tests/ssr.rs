//! SSR integration for the Stack navigator: render-at-path, page
//! metadata capture, and header chrome via the SSR handler. Lives here
//! (not in backend-ssr) because the SSR handler depends on backend-ssr,
//! so backend-ssr can't dev-depend back on this SDK without a cycle.

#![cfg(not(target_arch = "wasm32"))]

use backend_ssr::{render_all, render_path, render_path_with};
use runtime_core::primitives::navigator::{
    enable_route_collector, take_route_collector, Screen,
};
use runtime_core::{set_page_metadata, text, view, PageMetadata, Route};
use stack_navigator::{Navigator, StackBuilder, StackScreenExt};

const HOME: Route<()> = Route::<()>::new("home", "/");
const ABOUT: Route<()> = Route::<()>::new("about", "/about");
const CONTACT: Route<()> = Route::<()>::new("contact", "/contact");

/// The requested URL mounts its own screen, not the hardcoded initial.
#[test]
fn render_path_mounts_matched_navigator_screen() {
    let html = render_path("/about", || {
        Navigator::new(&HOME)
            .screen(HOME, |_| Screen::new(view(vec![text("HOME PAGE").into()])))
            .screen(ABOUT, |_| Screen::new(view(vec![text("ABOUT PAGE").into()])))
            .into()
    })
    .html;

    assert!(html.contains("ABOUT PAGE"), "expected About screen, got: {html}");
    assert!(
        !html.contains("HOME PAGE"),
        "Home should not render at /about, got: {html}"
    );
}

/// Metadata a screen declares is captured for the matched URL.
#[test]
fn render_path_captures_page_metadata() {
    let page = render_path("/about", || {
        Navigator::new(&HOME)
            .screen(HOME, |_| Screen::new(view(vec![text("home").into()])))
            .screen(ABOUT, |_| {
                set_page_metadata(PageMetadata {
                    title: Some("About — Idealyst".into()),
                    description: Some("What Idealyst is.".into()),
                    ..Default::default()
                });
                Screen::new(view(vec![text("about").into()]))
            })
            .into()
    });

    assert_eq!(page.metadata.title.as_deref(), Some("About — Idealyst"));
    assert_eq!(page.metadata.description.as_deref(), Some("What Idealyst is."));
}

/// With the SSR handler registered, the navigator renders real chrome:
/// the matched screen's header title AND its body.
#[test]
fn stack_ssr_handler_renders_header_chrome() {
    let html = render_path_with(
        "/about",
        |b| stack_navigator::chrome::register(b),
        || {
            Navigator::new(&HOME)
                .screen(HOME, |_| {
                    Screen::new(view(vec![text("home").into()])).title("Home")
                })
                .screen(ABOUT, |_| {
                    Screen::new(view(vec![text("ABOUT BODY").into()])).title("About Title")
                })
                .into()
        },
    )
    .html;

    assert!(html.contains("About Title"), "expected header chrome title, got: {html}");
    assert!(html.contains("ABOUT BODY"), "expected screen body, got: {html}");
}

/// SSG nav-hierarchy discovery: with the route collector enabled, a
/// single `render_path` call publishes every registered screen path
/// (not just the one the URL matched). This is the hook
/// `backend_ssr::render_all` drives the crawl from.
#[test]
fn route_collector_publishes_every_screen_path_on_mount() {
    enable_route_collector();
    let _ = render_path("/", || {
        Navigator::new(&HOME)
            .screen(HOME, |_| Screen::new(view(vec![text("home").into()])))
            .screen(ABOUT, |_| Screen::new(view(vec![text("about").into()])))
            .screen(CONTACT, |_| Screen::new(view(vec![text("contact").into()])))
            .into()
    });
    let mut found = take_route_collector().expect("collector was enabled");
    found.sort();
    assert_eq!(found, vec!["/", "/about", "/contact"]);
}

/// SSG end-to-end: `render_all` discovers every literal screen path
/// reachable from the root navigator and produces a `RenderedPage` per
/// path. Parameterized routes are skipped.
#[test]
fn render_all_crawls_every_literal_screen() {
    const USER: Route<()> = Route::<()>::new("user", "/user/:id");
    let result = render_all(
        |_| {},
        || {
            Navigator::new(&HOME)
                .screen(HOME, |_| Screen::new(view(vec![text("HOME").into()])))
                .screen(ABOUT, |_| Screen::new(view(vec![text("ABOUT").into()])))
                .screen(CONTACT, |_| Screen::new(view(vec![text("CONTACT").into()])))
                .screen(USER, |_| Screen::new(view(vec![text("USER").into()])))
                .into()
        },
    );

    let mut paths: Vec<_> = result.pages.keys().cloned().collect();
    paths.sort();
    assert_eq!(paths, vec!["/", "/about", "/contact"]);
    assert_eq!(result.skipped_parameterized, vec!["/user/:id"]);

    assert!(result.pages["/"].html.contains("HOME"));
    assert!(result.pages["/about"].html.contains("ABOUT"));
    assert!(result.pages["/contact"].html.contains("CONTACT"));
}

/// Collector is opt-in. When `enable_route_collector` isn't called,
/// `take_route_collector` returns None and the framework path stays
/// zero-allocation.
#[test]
fn route_collector_disabled_by_default() {
    let _ = render_path("/", || {
        Navigator::new(&HOME)
            .screen(HOME, |_| Screen::new(view(vec![text("home").into()])))
            .screen(ABOUT, |_| Screen::new(view(vec![text("about").into()])))
            .into()
    });
    assert!(take_route_collector().is_none());
}
