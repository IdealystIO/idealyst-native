//! SSR integration for the Tab navigator: the tab bar (all tab labels)
//! and the active tab's screen appear; inactive screens don't.

#![cfg(not(target_arch = "wasm32"))]

use backend_ssr::render_path_with;
use runtime_core::primitives::navigator::Screen;
use runtime_core::{text, view, Route};
use tab_navigator::{TabNavigator, TabSpec, TabsBuilder};

const HOME: Route<()> = Route::<()>::new("home", "/");
const ABOUT: Route<()> = Route::<()>::new("about", "/about");

#[test]
fn tab_ssr_renders_tab_bar_and_active_screen() {
    let html = render_path_with(
        "/about",
        |b| tab_navigator::chrome::register(b),
        || {
            TabNavigator::new(&HOME)
                .tab(HOME, TabSpec::new("HomeTab"), |_| {
                    Screen::new(view(vec![text("home body").into()]))
                })
                .tab(ABOUT, TabSpec::new("AboutTab"), |_| {
                    Screen::new(view(vec![text("ABOUT BODY").into()]))
                })
                .into()
        },
    )
    .html;

    // Tab bar shows every tab label regardless of which is active.
    assert!(html.contains("HomeTab"), "expected Home tab label, got: {html}");
    assert!(html.contains("AboutTab"), "expected About tab label, got: {html}");
    // Active screen body present; inactive screen body absent.
    assert!(html.contains("ABOUT BODY"), "expected active screen body, got: {html}");
    assert!(
        !html.contains("home body"),
        "inactive screen body should not render, got: {html}"
    );
}
