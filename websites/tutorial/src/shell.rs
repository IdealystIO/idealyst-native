//! The persistent sidebar (built once into the `AppShell` panel), the
//! mobile header, and the per-screen layout wrapper.
//!
//! The sidebar walks `routes::SECTIONS` to render track headers + step
//! links. Each link reads the navigator's reactive `active_route`
//! inside its style closure, so the active highlight updates reactively
//! without rebuilding the tree — the same pattern the marketing site
//! uses.

use std::rc::Rc;

use runtime_core::{
    derived, pressable, text, ui, Element, IntoElement, Signal, StyleApplication,
};
use idea_ui::{dark_theme, light_theme, set_idea_theme, Switch, Typography};

use crate::routes::SECTIONS;
use crate::styles::{
    MobileHeader, MobileHeaderButton, MobileHeaderTitle, MobileHeaderTitleWrap, NavLink,
    NavLinkActive, ScreenScroll, SidebarBody, SidebarFooter, SidebarHeader, SidebarScroll,
    SidebarSection,
};

/// Wrap a step page in its own scroll surface (background, text color).
/// The drawer navigator no longer owns scroll — the screen owns it, so
/// each page provides its own `scroll_view`.
pub fn layout(content: Element) -> Element {
    let style = ScreenScroll();
    ui! { scroll_view(style = style) { content } }
}

/// Header-over-outlet column for the swap layout in `app()` (the `ui!`
/// call sites live in `lib.rs`).
pub fn shell_column_style() -> runtime_core::StyleApplication {
    static KEY: u8 = 0;
    let sheet = runtime_core::cached_stylesheet(&KEY as *const u8 as usize, || {
        std::rc::Rc::new(runtime_core::StyleSheet::r#static(runtime_core::StyleRules {
            flex_direction: Some(runtime_core::FlexDirection::Column),
            width: Some(runtime_core::Length::Percent(100.0).into()),
            height: Some(runtime_core::Length::Percent(100.0).into()),
            min_height: Some(runtime_core::Length::Px(0.0).into()),
            ..Default::default()
        }))
    });
    runtime_core::StyleApplication::new(sheet)
}

/// The outlet's growing wrapper: absorbs the column's remaining height
/// so screens (whose own `scroll_view`s need a bounded box) fill below
/// the mobile header.
pub fn outlet_grow_style() -> runtime_core::StyleApplication {
    static KEY: u8 = 0;
    let sheet = runtime_core::cached_stylesheet(&KEY as *const u8 as usize, || {
        std::rc::Rc::new(runtime_core::StyleSheet::r#static(runtime_core::StyleRules {
            flex_direction: Some(runtime_core::FlexDirection::Column),
            flex_grow: Some(1.0f32.into()),
            min_height: Some(runtime_core::Length::Px(0.0).into()),
            ..Default::default()
        }))
    });
    runtime_core::StyleApplication::new(sheet)
}

/// Mobile-style top bar — hamburger on the left, the active lesson's
/// label leading-aligned. Mounts ONCE in the swap layout column and
/// survives every screen swap. Rendered unconditionally; the
/// `MobileHeader` sheet's `breakpoint lg` overlay collapses it to zero
/// height at pinned widths (the hamburger is the only way to open the
/// drawer once the sidebar overlays itself, so the bar must appear at
/// exactly the width where the AppShell sidebar collapses).
pub fn mobile_header(
    active_route: Signal<&'static str>,
    drawer_open: Signal<bool>,
) -> Element {
    let header_style = MobileHeader();
    let title_wrap_style = MobileHeaderTitleWrap();
    let title_style = move || StyleApplication::new(MobileHeaderTitle::sheet());
    let button_style = move || StyleApplication::new(MobileHeaderButton::sheet());

    // --- menu button (leading) ---
    let menu_icon: Element = ui! { text(style = button_style) { "\u{2630}" } };
    let menu_button = pressable(vec![menu_icon], move || drawer_open.set(true))
        .into_element();

    // --- title — reactive on the navigator's active_route ---
    let title_source = move || {
        let route = active_route.get();
        crate::label_for_route(route).unwrap_or(route).to_string()
    };
    let title_view: Element = text(title_source).with_style(title_style).into_element();
    let title_node = ui! {
        view(style = title_wrap_style) { title_view }
    };

    ui! {
        view(style = header_style) {
            menu_button
            title_node
        }
    }
}

/// Build the persistent sidebar. Runs once (the AppShell panel mounts
/// it a single time); its reactive scope lives for the navigator's
/// whole lifetime.
pub fn sidebar(active_route: Signal<&'static str>, is_dark: Signal<bool>) -> Element {
    let body_style = SidebarBody();
    let header_style = SidebarHeader();
    let footer_style = SidebarFooter();

    let header_children: Vec<Element> = vec![
        ui! { Typography(content = "Idealyst Tutorial".to_string(), kind = idea_ui::typography_kind::H3) },
        ui! {
            Typography(
                content = "Learn the core concepts, hands-on.".to_string(),
                muted = true,
            )
        },
    ];

    ui! {
        // The WHOLE sidebar scrolls as one — header, nav list, and the theme
        // toggle all move together when the content is taller than the panel.
        // The scroll surface fills the drawer's sidebar panel; `SidebarBody`
        // (background + padding, `min_height: 100%`) is the scrolled content so
        // its background still spans the full panel when the content is short.
        scroll_view(style = SidebarScroll()) {
            view(style = body_style) {
                view(style = header_style) { header_children }
                for section in SECTIONS {
                    (!section.title.is_empty()).then(|| ui! {
                        text(style = SidebarSection()) { section.title.to_string() }
                    })
                    for entry in section.entries {
                        nav_link(entry.route, entry.label, active_route)
                    }
                }
                theme_toggle(footer_style, is_dark)
            }
        }
    }
}

/// Dark/light switch pinned to the bottom of the sidebar. Flips
/// `is_dark` and swaps the installed idea-ui theme so every token
/// re-resolves — including this tutorial's own chrome and the
/// theme-aware code panels.
fn theme_toggle(footer_style: SidebarFooter, is_dark: Signal<bool>) -> Element {
    let on_change: Rc<dyn Fn(bool)> = Rc::new(move |dark| {
        is_dark.set(dark);
        if dark {
            set_idea_theme(dark_theme());
        } else {
            set_idea_theme(light_theme());
        }
    });

    let row_children: Vec<Element> = vec![ui! {
        Switch(
            label = Some("Dark mode".to_string()),
            value = is_dark,
            on_change = on_change,
        )
    }];

    ui! { view(style = footer_style) { row_children } }
}

/// One sidebar nav link. The `active` axis is derived reactively from
/// `active_route`, so the highlight flips without rebuilding the link.
fn nav_link(
    route: &'static runtime_core::Route<()>,
    label: &'static str,
    active_route: Signal<&'static str>,
) -> Element {
    let route_for_match: &'static str = route.name();
    let style = NavLink().active(derived(move || {
        if active_route.get() == route_for_match {
            NavLinkActive::On
        } else {
            NavLinkActive::Off
        }
    }));
    let label_text = label.to_string();

    ui! {
        link(route = route, params = ()) {
            text(style = style) { label_text }
        }
    }
}
