//! SSR integration for the Drawer navigator: the sidebar (an author
//! Element, built via the deferred `build_node_into` path) and the
//! path-matched screen both appear in the rendered HTML.

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
