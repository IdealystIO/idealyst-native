//! SSR integration for the Stack navigator: render-at-path, page
//! metadata capture, and header chrome via the SSR handler. Lives here
//! (not in backend-ssr) because the SSR handler depends on backend-ssr,
//! so backend-ssr can't dev-depend back on this SDK without a cycle.

#![cfg(not(target_arch = "wasm32"))]

use backend_ssr::{render_path, render_path_with};
use runtime_core::primitives::navigator::Screen;
use runtime_core::{set_page_metadata, text, view, PageMetadata, Route};
use stack_navigator::{Navigator, StackBuilder, StackScreenExt};

const HOME: Route<()> = Route::<()>::new("home", "/");
const ABOUT: Route<()> = Route::<()>::new("about", "/about");

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
