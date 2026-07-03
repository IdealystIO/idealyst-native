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

    // The navigator never owns scroll: the body is a plain flex container,
    // NOT a scroll context. The `-scrolls` class must be gone entirely.
    assert!(
        !html.contains("ui-nav-drawer-body-scrolls"),
        "body must not be a scroll context (no -scrolls class), got: {html}"
    );

    // The footer (bottom slot) is a PINNED SIBLING of the middle row —
    // inserted into the root after the middle — not a child of the body.
    // So in source order the whole middle subtree (sidebar + body +
    // screen) precedes the bottom slot.
    let middle = html.find("ui-nav-drawer-middle").unwrap();
    let body = html.find("ui-nav-drawer-body").unwrap();
    let bottom = html.find("ui-nav-drawer-bottom").unwrap();
    let screen = html.find("ABOUT BODY").unwrap();
    let footer = html.find("SITE FOOTER").unwrap();
    assert!(middle < bottom, "bottom slot must come after the middle row, got: {html}");
    assert!(body < bottom, "bottom slot must come after the body outlet, got: {html}");
    assert!(screen < footer, "footer renders after the screen content, got: {html}");
}

/// Locks the de-opinionated structure: regardless of builder config the
/// bottom slot is a pinned sibling of the middle row (in the root), never
/// nested inside the scrolling body — because the navigator no longer
/// owns scroll. Host-testable via the SSR chrome handler (no device).
#[test]
fn drawer_ssr_bottom_slot_is_pinned_sibling() {
    let page = render_path_with(
        "/",
        |b| drawer_navigator::chrome::register(b),
        || {
            DrawerNavigator::new(&HOME)
                .leading_with(|_slot| view(vec![text("SIDE").into()]).into())
                .bottom_with(|_slot| view(vec![text("FOOT").into()]).into())
                .screen(HOME, |_| Screen::new(view(vec![text("home body").into()])))
                .into()
        },
    );
    let html = &page.html;

    // No scroll context anywhere on the body.
    assert!(
        !html.contains("ui-nav-drawer-body-scrolls"),
        "body must not be a scroll context, got: {html}"
    );

    // The bottom slot sits as a root-level sibling after the middle row.
    let middle = html.find("ui-nav-drawer-middle").expect("middle row");
    let bottom = html.find("ui-nav-drawer-bottom").expect("bottom slot");
    assert!(middle < bottom, "bottom slot must be a sibling after the middle row, got: {html}");
}
