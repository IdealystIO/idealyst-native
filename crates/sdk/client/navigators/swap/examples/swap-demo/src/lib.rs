//! `swap-demo` — the canonical "TabBar wired to a router" case.
//!
//! One [`SwapNavigator`] with three co-equal screens. Selecting a tab swaps the
//! visible screen — no push/pop, no back-stack. The bottom tab bar is *author
//! layout*: the `.layout(|nav| …)` closure wraps the navigator's single outlet
//! (`{nav.outlet}`) in a column with an [`idea_ui_nav::TabBar`] wired to the
//! navigator's `active_route` / `on_select`. This is exactly the shape the old
//! tab navigator couldn't do without panicking on web.

use idea_ui::{install_idea_theme, light_theme, Typography};
use idea_ui_nav::{TabBar, TabItem};
use runtime_core::{
    component, ui, Element, Ref, Route, Screen, StyleApplication, StyleRules, StyleSheet,
};
use runtime_core::{AlignItems, FlexDirection, Length};
use std::rc::Rc;
use swap_navigator::{SwapBuilder, SwapHandle, SwapNavigator};

/// SDK-handler registration seam the CLI-generated wrappers invoke after
/// `runtime_vocabulary::register_builtins`. The swap navigator's runtime IS
/// a vocabulary built-in (one backend-neutral handler installed on every
/// host), so this demo has nothing extra to register — the seam is kept
/// because every wrapper calls it, and an unregistered payload panics at
/// realize.
pub fn register_scene_extensions<H: runtime_scene::Host>(
    _registry: &mut runtime_scene::Registry<H>,
) {
}

/// Runtime-server (sidecar) recorder seam — the recorder's scene registry
/// gets the navigator from `register_builtins` too, so there is nothing to
/// register host-side either.
#[cfg(feature = "sidecar")]
pub fn register_scene_extensions_recorder(_registry: &mut dev_server::newcore::SceneRegistry) {}

/// Android entry: the generated wrapper's `attach` mounts `scene_app()`
/// through `backend_android::newcore::start`.
pub fn scene_app() -> Element {
    app()
}

const HOME: Route<()> = Route::<()>::new("home", "/");
const SEARCH: Route<()> = Route::<()>::new("search", "/search");
const PROFILE: Route<()> = Route::<()>::new("profile", "/profile");

#[component]
pub fn app() -> Element {
    install_idea_theme(light_theme());

    let nav: Ref<SwapHandle> = Ref::new();

    let builder = SwapNavigator::new(&HOME)
        .screen(HOME, |_| Screen::new(page("Home", "The first tab.")))
        .screen(SEARCH, |_| Screen::new(page("Search", "The second tab.")))
        .screen(PROFILE, |_| Screen::new(page("Profile", "The third tab.")))
        .layout(|nav| {
            // Fill-height column: the outlet grows, the bar sits at the bottom.
            ui! {
                view(style = column_style) {
                    view(style = grow_style) {
                        nav.outlet
                    }
                    TabBar(
                        items = vec![
                            TabItem::new("home", "Home"),
                            TabItem::new("search", "Search"),
                            TabItem::new("profile", "Profile"),
                        ],
                        active_route = nav.active_route,
                        on_select = nav.on_select,
                    )
                }
            }
        });

    ui! { builder.bind(nav) }
}

fn page(title: &str, body: &str) -> Element {
    ui! {
        view(style = page_style) {
            Typography(content = title.to_string(), kind = idea_ui::typography_kind::H1)
            Typography(content = body.to_string(), muted = true)
        }
    }
}

// ---------------------------------------------------------------------------
// Inline styles (a demo, so kept local; a real app would use idea-ui layout).
// ---------------------------------------------------------------------------

// Shared empty base sheet via `cached_stylesheet` — a per-file `thread_local!`
// burns an Android/bionic pthread TLS key per sheet (capped ~128).
fn base() -> Rc<StyleSheet> {
    static KEY: u8 = 0;
    runtime_core::cached_stylesheet(&KEY as *const u8 as usize, || {
        Rc::new(StyleSheet::r#static(StyleRules::default()))
    })
}

fn column_style() -> StyleApplication {
    StyleApplication::new(base()).with_computed("swap_demo_column", || StyleRules {
        flex_direction: Some(FlexDirection::Column),
        height: Some(Length::Percent(100.0).into()),
        width: Some(Length::Percent(100.0).into()),
        ..Default::default()
    })
}

fn grow_style() -> StyleApplication {
    StyleApplication::new(base()).with_computed("swap_demo_grow", || StyleRules {
        flex_grow: Some(1.0.into()),
        ..Default::default()
    })
}

fn page_style() -> StyleApplication {
    StyleApplication::new(base()).with_computed("swap_demo_page", || StyleRules {
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::FlexStart),
        gap: Some(Length::Px(12.0).into()),
        padding_top: Some(Length::Px(24.0).into()),
        padding_left: Some(Length::Px(24.0).into()),
        padding_right: Some(Length::Px(24.0).into()),
        ..Default::default()
    })
}
