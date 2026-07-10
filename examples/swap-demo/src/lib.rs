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
use std::cell::RefCell;
use std::rc::Rc;
use swap_navigator::{SwapBuilder, SwapHandle, SwapNavigator};

/// Navigators self-register at backend construction via `inventory::submit!`
/// (the SDK force-links its web module so this works in dev + release). The app
/// just uses them; this hook stays for the CLI bootstrap.
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(backend: &mut dev_server::WireRecordingBackend) {
    swap_navigator::recording::register(backend);
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
            let outlet = ui! {
                view(style = grow_style) {
                    { nav.outlet }
                }
            };
            let bar: Element = ui! {
                TabBar(
                    items = vec![
                        TabItem::new("home", "Home"),
                        TabItem::new("search", "Search"),
                        TabItem::new("profile", "Profile"),
                    ],
                    active_route = nav.active_route,
                    on_select = nav.on_select,
                )
            };
            let mut children: Vec<Element> = Vec::with_capacity(2);
            children.push(outlet);
            children.push(bar);
            ui! {
                view(style = column_style) {
                    children
                }
            }
        });

    ui! { builder.bind(nav) }
}

fn page(title: &str, body: &str) -> Element {
    let children: Vec<Element> = vec![
        ui! { Typography(content = title.to_string(), kind = idea_ui::typography_kind::H1) },
        ui! { Typography(content = body.to_string(), muted = true) },
    ];
    ui! {
        view(style = page_style) {
            children
        }
    }
}

// ---------------------------------------------------------------------------
// Inline styles (a demo, so kept local; a real app would use idea-ui layout).
// ---------------------------------------------------------------------------

thread_local! {
    static BASE: RefCell<Option<Rc<StyleSheet>>> = const { RefCell::new(None) };
}
fn base() -> Rc<StyleSheet> {
    BASE.with(|s| {
        if s.borrow().is_none() {
            *s.borrow_mut() = Some(Rc::new(StyleSheet::r#static(StyleRules::default())));
        }
        s.borrow().as_ref().cloned().unwrap()
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
