//! SSR integration for the Drawer navigator: the sidebar (an author
//! Element, built via the deferred `build_node_into` path) and the
//! path-matched screen both appear in the rendered HTML, laid out by the
//! shared navigator stylesheet (not guessed inline styles).

#![cfg(not(target_arch = "wasm32"))]

use backend_ssr::render_path_with;
use drawer_navigator::{DrawerBuilder, DrawerNavigator};
use runtime_core::primitives::navigator::Screen;
use runtime_core::{text, view, Route};

const HOME: Route<()> = Route::<()>::new("home", "/");
const ABOUT: Route<()> = Route::<()>::new("about", "/about");

#[test]
fn drawer_ssr_renders_sidebar_and_matched_screen() {
    let html = render_path_with(
        "/about",
        |b| drawer_navigator::chrome::register(b),
        || {
            DrawerNavigator::new(&HOME)
                .sidebar(view(vec![text("SIDEBAR NAV").into()]).into())
                .screen(HOME, |_| Screen::new(view(vec![text("home body").into()])))
                .screen(ABOUT, |_| Screen::new(view(vec![text("ABOUT BODY").into()])))
                .into()
        },
    )
    .html;

    assert!(html.contains("SIDEBAR NAV"), "expected sidebar chrome, got: {html}");
    assert!(html.contains("ABOUT BODY"), "expected matched screen body, got: {html}");
    assert!(
        !html.contains("home body"),
        "home screen should not render at /about, got: {html}"
    );
}

/// The next-gen slot system (`leading_with` / `bottom_with`, as the real
/// website uses) renders its chrome alongside the matched screen, and the
/// layout comes from the shared `ui-nav-drawer-*` classes + the navigator
/// stylesheet shipped via `register_raw_css` — never guessed inline CSS.
#[test]
fn drawer_ssr_renders_new_slot_chrome() {
    let page = render_path_with(
        "/about",
        |b| drawer_navigator::chrome::register(b),
        || {
            DrawerNavigator::new(&HOME)
                .leading_with(|_slot| view(vec![text("LEADING SIDEBAR").into()]).into())
                .bottom_with(|_slot| view(vec![text("SITE FOOTER").into()]).into())
                .screen(HOME, |_| Screen::new(view(vec![text("home body").into()])))
                .screen(ABOUT, |_| Screen::new(view(vec![text("ABOUT BODY").into()])))
                .into()
        },
    );
    let html = &page.html;

    assert!(html.contains("LEADING SIDEBAR"), "expected leading slot chrome, got: {html}");
    assert!(html.contains("SITE FOOTER"), "expected bottom slot chrome, got: {html}");
    assert!(html.contains("ABOUT BODY"), "expected matched screen body, got: {html}");
    assert!(!html.contains("home body"), "inactive screen should not render, got: {html}");

    // Layout is driven by the shared navigator classes — the same ones
    // the live web navigator stamps — not by inline styles invented here.
    assert!(html.contains("ui-nav-drawer-root"), "expected drawer root class, got: {html}");
    assert!(html.contains("ui-nav-drawer-middle"), "expected middle row class, got: {html}");
    assert!(html.contains("ui-nav-drawer-sidebar"), "expected sidebar class, got: {html}");
    assert!(html.contains("ui-nav-drawer-body"), "expected body outlet class, got: {html}");
    assert!(html.contains("ui-nav-drawer-bottom"), "expected bottom slot class, got: {html}");

    // The matching stylesheet is shipped for the document <head> (single
    // source of truth shared with the web backend) — the responsive
    // sidebar + row layout lives there, not inline on the nodes.
    let sheet = &page.head_css;
    assert!(
        sheet.contains(".ui-nav-drawer-middle{flex:1 1 auto;display:flex;flex-direction:row"),
        "expected the row layout in the shipped sheet, got: {sheet}"
    );
    // The sidebar is responsive: an off-canvas modal at the base
    // (narrow) — `position:fixed` + `translateX(-100%)`, slid in by the
    // `.drawer-open` class — and pinned in-flow at the large breakpoint
    // via the `@media (min-width: 1024px)` block (`position:static`,
    // fixed `width`). This replaced the old always-pinned
    // `flex:0 0 auto;height:100%` rule when the web drawer gained its
    // narrow-viewport modal behavior.
    assert!(
        sheet.contains(".ui-nav-drawer-sidebar{position:fixed;")
            && sheet.contains("transform:translateX(-100%)"),
        "expected the off-canvas modal sidebar rule in the shipped sheet, got: {sheet}"
    );
    assert!(
        sheet.contains(".ui-nav-drawer-root.drawer-open .ui-nav-drawer-sidebar{transform:translateX(0)"),
        "expected the drawer-open slide-in rule in the shipped sheet, got: {sheet}"
    );
    assert!(
        sheet.contains("@media (min-width: 1024px)")
            && sheet.contains(".ui-nav-drawer-sidebar{position:static"),
        "expected the pinned (wide-viewport) sidebar rule in the shipped sheet, got: {sheet}"
    );

    // Default mode is `bottom_in_scroll`: the body is the scroll context
    // and the footer mounts after the screen inside it.
    assert!(
        html.contains("ui-nav-drawer-body-scrolls"),
        "expected scroll-mode body class, got: {html}"
    );
    let screen = html.find("ABOUT BODY").unwrap();
    let footer = html.find("SITE FOOTER").unwrap();
    assert!(screen < footer, "screen should render before the footer, got: {html}");
}
