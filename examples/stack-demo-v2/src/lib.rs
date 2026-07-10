//! `stack-demo-v2` — a stack navigator on the OUTLET model.
//!
//! One [`StackNavigator`] with three screens. Home pushes Detail / Settings;
//! the header (an [`idea_ui_nav::StackHeader`]) is *author layout* wrapping the
//! navigator's single outlet, with a back arrow that appears past the root
//! (`can_go_back`) and calls `nav.pop`. The title is derived from the active
//! route. No built-in navigator chrome — the same header renders on every
//! backend.

use idea_ui::{install_idea_theme, light_theme, Typography};
use idea_ui_nav::StackHeader;
use runtime_core::primitives::navigator::HeaderButton;
use runtime_core::{
    component, rx, ui, Element, Ref, Route, Screen, StyleApplication, StyleRules, StyleSheet,
};
use runtime_core::{AlignItems, FlexDirection, Length};
use std::cell::RefCell;
use std::rc::Rc;
use stack_navigator_v2::{
    header_state, StackBuilder, StackContext, StackHandle, StackNavigator, StackScreenExt,
};

/// The stack SDK self-registers via `inventory::submit!` (it force-links its web
/// module so this works in dev + release). Hook kept for the CLI bootstrap.
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

const HOME: Route<()> = Route::<()>::new("home", "/");
const DETAIL: Route<()> = Route::<()>::new("detail", "/detail");
const SETTINGS: Route<()> = Route::<()>::new("settings", "/settings");

#[component]
pub fn app() -> Element {
    install_idea_theme(light_theme());

    let nav: Ref<StackHandle> = Ref::new();

    let builder = StackNavigator::new(&HOME)
        // Per-screen header slots: title (native bar title on mobile, drawn
        // header title on web) + a trailing button on Detail.
        .screen(HOME, move |_| Screen::new(home_page(nav)).title("Home"))
        .screen(DETAIL, |_| {
            Screen::new(page("Detail", "Pushed onto the stack. The header's back arrow pops it."))
                .title("Detail")
                .header_right(HeaderButton::text("Edit").on_press(|| {}))
        })
        .screen(SETTINGS, |_| {
            Screen::new(page("Settings", "Another pushed screen. Back returns to Home."))
                .title("Settings")
        })
        .layout(move |nav: StackContext| {
            // The header reads the active screen's slots (title / left / right /
            // hidden) — the same data the native bar renders on mobile.
            let screen_chrome = nav.screen_chrome;
            let state = rx!(header_state(&screen_chrome));

            let header: Element = ui! {
                StackHeader(
                    state = state,
                    show_back = nav.can_go_back,
                    on_back = Some(nav.pop.clone()),
                )
            };
            let body: Element = ui! {
                view(style = grow_style) {
                    { nav.outlet }
                }
            };
            let mut children: Vec<Element> = Vec::with_capacity(2);
            children.push(header);
            children.push(body);
            ui! {
                view(style = column_style) {
                    children
                }
            }
        });

    ui! { builder.bind(nav) }
}

fn home_page(nav: Ref<StackHandle>) -> Element {
    let go_detail = move || {
        nav.get().map(|h| h.push(&DETAIL, ())).unwrap_or_default();
    };
    let go_settings = move || {
        nav.get().map(|h| h.push(&SETTINGS, ())).unwrap_or_default();
    };

    let children: Vec<Element> = vec![
        ui! { Typography(content = "Home".to_string(), kind = idea_ui::typography_kind::H1) },
        ui! {
            Typography(
                content = "Push a detail screen. The header grows a back arrow past the root.".to_string(),
                muted = true,
            )
        },
        ui! { button(label = "Open Detail".to_string(), on_click = go_detail) },
        ui! { button(label = "Open Settings".to_string(), on_click = go_settings) },
    ];
    ui! {
        view(style = page_style) {
            children
        }
    }
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
// Inline styles (demo-local).
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
    StyleApplication::new(base()).with_computed("stackv2_column", || StyleRules {
        flex_direction: Some(FlexDirection::Column),
        height: Some(Length::Percent(100.0).into()),
        width: Some(Length::Percent(100.0).into()),
        ..Default::default()
    })
}
fn grow_style() -> StyleApplication {
    StyleApplication::new(base()).with_computed("stackv2_grow", || StyleRules {
        flex_grow: Some(1.0.into()),
        ..Default::default()
    })
}
fn page_style() -> StyleApplication {
    StyleApplication::new(base()).with_computed("stackv2_page", || StyleRules {
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::FlexStart),
        gap: Some(Length::Px(12.0).into()),
        padding_top: Some(Length::Px(24.0).into()),
        padding_left: Some(Length::Px(24.0).into()),
        padding_right: Some(Length::Px(24.0).into()),
        ..Default::default()
    })
}
