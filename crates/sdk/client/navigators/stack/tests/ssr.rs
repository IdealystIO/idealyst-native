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

/// Regression (navigator styling goes through the style system): the SSR
/// stack no longer stamps hand-named `ui-nav-*` classes or ships the
/// injected navigator stylesheet. Instead the container carries
/// `stack_container_rules()` and the screen carries the
/// `stack_screen_fill_rules()` override — both resolved to normal
/// content-hashed classes whose rule bodies land in the emitted
/// stylesheet, exactly as the live web client mints them (byte-identical
/// first paint). The only remaining class-shaped artifact is the
/// structural `NAV_ROOT_HYDRATION_CLASS` marker, which carries no CSS and
/// exists solely so the hydrating client can adopt the container.
#[test]
fn regression_stack_ssr_styles_via_style_system_not_injected_classes() {
    use runtime_core::primitives::navigator::{
        stack_container_rules, stack_screen_fill_rules, NAV_ROOT_HYDRATION_CLASS,
    };

    let page = render_path_with(
        "/about",
        |b| stack_navigator::chrome::register(b),
        || {
            Navigator::new(&HOME)
                .screen(HOME, |_| Screen::new(view(vec![text("home").into()])))
                .screen(ABOUT, |_| Screen::new(view(vec![text("ABOUT BODY").into()])))
                .into()
        },
    );
    let html = &page.html;
    let head_css = &page.head_css;

    // No legacy class names in the markup or the emitted stylesheet.
    assert!(!html.contains("ui-nav-root"), "legacy container class leaked: {html}");
    assert!(!html.contains("ui-nav-screen"), "legacy screen class leaked: {html}");
    assert!(
        !head_css.contains("ui-nav-"),
        "legacy navigator stylesheet leaked into head CSS: {head_css}"
    );

    // The structural hydration marker is present on the container.
    assert!(
        html.contains(NAV_ROOT_HYDRATION_CLASS),
        "hydration adoption marker missing: {html}"
    );

    // The style-system rule bodies are in the emitted stylesheet — the
    // same `rules_to_css` bytes the live web backend inserts, so the
    // first paint matches the hydrated client.
    let container_css = css::rules_to_css(&stack_container_rules());
    assert!(
        head_css.contains(&container_css),
        "container fill rules missing from emitted CSS (wanted `{container_css}`): {head_css}"
    );
    let screen_css = css::rules_to_css(&stack_screen_fill_rules());
    assert!(
        head_css.contains(&screen_css),
        "screen full-bleed rules missing from emitted CSS (wanted `{screen_css}`): {head_css}"
    );
}

/// Regression: the screen fill override must COMPOSE with the screen's own
/// style, not replace it — the author's styling survives while the
/// navigator's placement fields win.
#[test]
fn regression_stack_ssr_screen_fill_composes_with_author_style() {
    use runtime_core::{Color, StyleApplication, StyleRules, StyleSheet, Tokenized};
    use std::rc::Rc;

    let page = render_path_with(
        "/",
        |b| stack_navigator::chrome::register(b),
        || {
            Navigator::new(&HOME)
                .screen(HOME, |_| {
                    let author = Rc::new(StyleSheet::r#static(StyleRules {
                        background: Some(Tokenized::Literal(Color("#123456".into()))),
                        ..Default::default()
                    }));
                    Screen::new(
                        view(vec![text("home").into()])
                            .with_style(StyleApplication::new(author)),
                    )
                })
                .into()
        },
    );
    let head_css = &page.head_css;

    // One class carries BOTH the author background and the navigator's
    // absolute placement — a single merged resolution, not two competing
    // rules. Find the rule body containing the absolute placement and
    // assert the author's background lives in the SAME body.
    let abs_at = head_css
        .find("position: absolute")
        .expect("screen fill placement missing from emitted CSS");
    let body_start = head_css[..abs_at].rfind('{').expect("rule body open brace");
    let body_end = abs_at + head_css[abs_at..].find('}').expect("rule body close brace");
    let body = &head_css[body_start..body_end];
    assert!(
        body.contains("#123456"),
        "author background must be merged into the screen's fill rule, got body `{body}` in: {head_css}"
    );
}
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

