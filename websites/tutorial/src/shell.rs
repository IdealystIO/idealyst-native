//! The persistent sidebar (mounted once via `DrawerNavigator`) and the
//! per-screen layout wrapper.
//!
//! The sidebar walks `routes::SECTIONS` to render track headers + step
//! links. Each link reads `SlotProps::active_route` inside its style
//! closure, so the active highlight updates reactively without
//! rebuilding the tree — the same pattern the marketing site uses.

use std::rc::Rc;

use runtime_core::{derived, ui, Element, Signal};
use drawer_navigator::SlotProps;
use idea_ui::{dark_theme, light_theme, set_idea_theme, Switch, Typography};

use crate::routes::SECTIONS;
use crate::styles::{
    NavLink, NavLinkActive, ScreenScroll, SidebarBody, SidebarFooter, SidebarHeader,
    SidebarScroll, SidebarSection,
};

/// Wrap a step page in its own scroll surface (background, text color).
/// The drawer navigator no longer owns scroll — the screen owns it, so
/// each page provides its own `scroll_view`.
pub fn layout(content: Element) -> Element {
    let style = ScreenScroll();
    ui! { scroll_view(style = style) { content } }
}

/// Build the persistent sidebar. Runs once at navigator init; its
/// reactive scope lives for the navigator's whole lifetime.
pub fn sidebar(slot: SlotProps, is_dark: Signal<bool>) -> Element {
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

    let active_route = slot.active_route;

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
