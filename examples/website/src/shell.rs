//! Persistent sidebar — mounted once via `DrawerNavigator::sidebar_with`.
//!
//! On web, drawer-navigator renders as a permanent flex-row layout
//! (sidebar + body). The sidebar mounts ONCE at navigator init and
//! survives every screen change — the navigator only swaps the
//! body. On iOS/Android the same drawer SDK becomes a slide-in
//! side panel.
//!
//! Active-link highlight: `DrawerSlotProps::active_route` is a
//! reactive `Signal<&'static str>` that the navigator updates on
//! every screen change. Each nav link reads it inside its style
//! closure, so the highlight updates without rebuilding the
//! sidebar tree.

use std::rc::Rc;

use runtime_core::{ui, Primitive, Signal, StyleApplication};
use drawer_navigator::DrawerSlotProps;
use idea_ui::{
    dark_theme, light_theme, set_idea_theme, spacer, switch, typography, TypographyKind,
    TypographyTone,
};

use crate::routes::{
    AGENTIC_ROUTE, BACKENDS_ROUTE, CONCEPTS_ROUTE, DEMO_ANIMATIONS_ROUTE, DEMO_COMPONENTS_ROUTE,
    DEMO_COUNTER_ROUTE, DEMO_NAVIGATION_ROUTE, FURTHER_READING_ROUTE, HOME_ROUTE, INSTALL_ROUTE,
    QUICKSTART_ROUTE, SECTIONS, WHY_RUST_ROUTE,
};
use crate::styles::{
    NavLink, ScreenScroll, SidebarBody, SidebarFooter, SidebarHeader, SidebarSection,
};

/// Wrap a page's content in a `ScrollView` sized to the drawer
/// body. The drawer-navigator's `.ui-nav-drawer-body` div has
/// `overflow: hidden`, so the screen needs its own scroll context
/// for long content. On native targets where the drawer SDK
/// supplies the scroll affordance (UIScrollView / Android
/// NestedScrollView), the `ScrollView` is the same primitive —
/// one author tree, every backend.
pub fn layout(content: Primitive) -> Primitive {
    let scroll_style = ScreenScroll();
    ui! {
        ScrollView(style = scroll_style) {
            content
        }
    }
}

/// Build the persistent sidebar. Called once by drawer-navigator
/// during `init`; the returned Primitive's reactive scope survives
/// for the navigator's entire lifetime.
///
/// `is_dark` is an app-level signal lifted out of `app()` so the
/// theme-toggle's state survives navigation (signals scoped to a
/// screen would reset on every push). Toggling it both flips the
/// signal AND swaps the installed idea-ui theme via
/// `set_idea_theme(...)`.
pub fn sidebar(slot: DrawerSlotProps, is_dark: Signal<bool>) -> Primitive {
    let body_style = SidebarBody();
    let header_style = SidebarHeader();
    let footer_style = SidebarFooter();

    let header_children: Vec<Primitive> = vec![
        ui! { Typography(content = "Idealyst".to_string(), kind = TypographyKind::H3) },
        ui! {
            Typography(
                content = "One codebase, native everywhere.".to_string(),
                tone = TypographyTone::Muted,
            )
        },
    ];

    let active_route = slot.active_route;

    let mut children: Vec<Primitive> = Vec::new();
    children.push(ui! { View(style = header_style) { header_children } });

    for section in SECTIONS {
        if !section.title.is_empty() {
            let title = section.title.to_string();
            let section_style = SidebarSection();
            children.push(ui! { Text(style = section_style) { title } });
        }
        for entry in section.entries {
            children.push(nav_link(entry.name, entry.label, active_route));
        }
    }

    // `Spacer` grows to fill the leftover vertical space, pinning
    // the footer to the bottom of the sidebar column when nav
    // content is short. When nav content overflows, the spacer has
    // no room to grow and the footer just sits after the last nav
    // link (the outer `.ui-nav-drawer-sidebar` div is overflow:auto
    // so the whole sidebar scrolls in that case).
    children.push(ui! { Spacer() });
    children.push(theme_toggle(footer_style, is_dark));

    ui! { View(style = body_style) { children } }
}

/// Dark/light theme switch pinned to the bottom of the sidebar.
/// Flips `is_dark` AND swaps the installed `IdeaTheme` so every
/// component re-renders against the new token set.
fn theme_toggle(footer_style: SidebarFooter, is_dark: Signal<bool>) -> Primitive {
    let on_change: Rc<dyn Fn(bool)> = Rc::new(move |dark| {
        is_dark.set(dark);
        if dark {
            set_idea_theme(dark_theme());
        } else {
            set_idea_theme(light_theme());
        }
    });

    let row_children: Vec<Primitive> = vec![
        ui! {
            Switch(
                label = Some("Dark mode".to_string()),
                value = is_dark,
                on_change = on_change,
            )
        },
    ];

    ui! { View(style = footer_style) { row_children } }
}

/// One sidebar nav link. Routes are matched by name; each emits a
/// `Link` to the corresponding `Route<()>` constant, which the
/// drawer SDK rewrites to a `Select` command. The style closure
/// reads `active_route` so the active variant flips reactively
/// without rebuilding the link.
fn nav_link(
    name: &'static str,
    label: &'static str,
    active_route: runtime_core::Signal<&'static str>,
) -> Primitive {
    let route_for_match: &'static str = name;
    let style = move || {
        let variant = if active_route.get() == route_for_match {
            "on"
        } else {
            "off"
        };
        StyleApplication::new(NavLink::sheet()).with("active", variant.to_string())
    };
    let label_text = label.to_string();

    match name {
        "home" => ui! {
            Link(route = &HOME_ROUTE, params = ()) {
                Text(style = style) { label_text }
            }
        },
        "install" => ui! {
            Link(route = &INSTALL_ROUTE, params = ()) {
                Text(style = style) { label_text }
            }
        },
        "quickstart" => ui! {
            Link(route = &QUICKSTART_ROUTE, params = ()) {
                Text(style = style) { label_text }
            }
        },
        "concepts" => ui! {
            Link(route = &CONCEPTS_ROUTE, params = ()) {
                Text(style = style) { label_text }
            }
        },
        "why-rust" => ui! {
            Link(route = &WHY_RUST_ROUTE, params = ()) {
                Text(style = style) { label_text }
            }
        },
        "demo-counter" => ui! {
            Link(route = &DEMO_COUNTER_ROUTE, params = ()) {
                Text(style = style) { label_text }
            }
        },
        "demo-components" => ui! {
            Link(route = &DEMO_COMPONENTS_ROUTE, params = ()) {
                Text(style = style) { label_text }
            }
        },
        "demo-animations" => ui! {
            Link(route = &DEMO_ANIMATIONS_ROUTE, params = ()) {
                Text(style = style) { label_text }
            }
        },
        "demo-navigation" => ui! {
            Link(route = &DEMO_NAVIGATION_ROUTE, params = ()) {
                Text(style = style) { label_text }
            }
        },
        "backends" => ui! {
            Link(route = &BACKENDS_ROUTE, params = ()) {
                Text(style = style) { label_text }
            }
        },
        "agentic" => ui! {
            Link(route = &AGENTIC_ROUTE, params = ()) {
                Text(style = style) { label_text }
            }
        },
        "further-reading" => ui! {
            Link(route = &FURTHER_READING_ROUTE, params = ()) {
                Text(style = style) { label_text }
            }
        },
        _ => ui! { Text { label_text } },
    }
}
