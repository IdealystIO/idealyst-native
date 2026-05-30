//! The persistent shell: sidebar with route links + a theme
//! toggle + the outlet for the active page. Wired into Navigator
//! via `.layout(...)`.
//!
//! Navigator's layout closure runs once per push/pop but the
//! enclosing scope stays mounted across navigation, so signal
//! reads inside (the dark-mode flag, the active route) re-fire
//! their dependent effects without rebuilding the chrome.

use std::rc::Rc;

use runtime_core::{ui, ChildList, Element, Signal, StyleApplication, VariantEnum};
// NOTE: LayoutProps is gone — the stack-navigator SDK no longer has
// a `.layout(...)` API. This shell module's old `web_layout` function
// has been deleted alongside; the helper components below
// (`sidebar`, `nav_link`) remain in case they're useful for a
// drawer-navigator-based rewrite of this example.
use idea_ui::{Typography, Card, dark_theme, Divider, light_theme, set_idea_theme, Stack, Switch, IdeaThemeRef, StackGap};

use crate::routes::INDEX;
use crate::styles::{Content, NavLink, PageRoot, Sidebar, SidebarHeader};

/// Build the docs app's layout. Receives the navigator's
/// `LayoutProps` (active-route signal + outlet primitive) and
/// returns the full chrome with the outlet embedded.
///
/// Web-only. On native (UIKit / Android), the platform's own
/// `UINavigationController` / `FragmentManager` provides the
/// chrome — a persistent sidebar fights the platform idiom there.
/// Build the sidebar: brand header, list of nav links, theme
/// toggle at the bottom. Pulled out so the layout closure stays
/// shallow.
#[cfg(target_arch = "wasm32")]
fn sidebar(
    active_route: Signal<&'static str>,
    is_dark: Signal<bool>,
    container_style: crate::styles::Sidebar,
) -> Element {
    let header_style = SidebarHeader();
    let header_children: Vec<Element> = vec![
        ui! { Typography(content = "idea-ui".to_string(), kind = idea_ui::typography_kind::H2) },
        ui! { Typography(content = "Component reference".to_string(), muted = true) },
    ];

    let mut links: Vec<Element> = Vec::with_capacity(INDEX.len());
    for entry in INDEX {
        links.push(nav_link(entry.name, entry.label, active_route));
    }

    let on_dark_change: Rc<dyn Fn(bool)> = Rc::new(move |dark| {
        is_dark.set(dark);
        if dark {
            set_idea_theme(dark_theme());
        } else {
            set_idea_theme(light_theme());
        }
    });

    // Compose the sidebar: brand header → links list → spacer
    // (handled by margin-top: auto on the theme switch row) →
    // theme switch.
    let theme_row_children: Vec<Element> = vec![
        ui! { Typography(content = "Theme".to_string(), kind = idea_ui::typography_kind::Caption) },
        ui! {
            Switch(
                label = Some("Dark mode".to_string()),
                value = is_dark,
                on_change = on_dark_change
            )
        },
    ];

    let mut children: Vec<Element> = Vec::new();
    children.push(ui! { view(style = header_style) { header_children } });
    for l in links {
        ChildList::append_to(l, &mut children);
    }
    // Divider before the theme toggle so it visually anchors to
    // the bottom of the sidebar without us needing a Spacer.
    children.push(ui! { Divider() });
    children.push(ui! { Stack(gap = StackGap::Xs) { theme_row_children } });

    ui! {
        view(style = container_style) { children }
    }
}

/// A nav link. Reads `active_route` so the highlight updates
/// reactively whenever the navigator pushes/pops without
/// rebuilding the whole sidebar.
#[cfg(target_arch = "wasm32")]
fn nav_link(
    name: &'static str,
    label: &'static str,
    active_route: Signal<&'static str>,
) -> Element {
    let label_text = label.to_string();
    // The on_click side-navigates by calling history.pushState
    // through the Navigator. Since we don't have a `Ref` to the
    // NavigatorHandle here (the layout doesn't receive it), we use
    // a `Link` primitive instead — it finds the ambient navigator
    // automatically and emits a real `<a href>` on web so
    // middle-click "open in new tab" works.
    //
    // Wrap the styled label inside the Link's children block. The
    // active-variant flip happens through the style closure
    // reading `active_route`.
    let route_for_match: &str = name;
    let style = move || {
        let variant = if active_route.get() == route_for_match {
            "on"
        } else {
            "off"
        };
        StyleApplication::new(NavLink::sheet()).with("active", variant.to_string())
    };

    // Find the right Route to point to.
    use crate::routes::{
        ACTIONS_ROUTE, FEEDBACK_ROUTE, INPUTS_ROUTE, LAYOUT_ROUTE, OVERLAYS_ROUTE, OVERVIEW_ROUTE,
        STATEFUL_ROUTE, THEMES_ROUTE, TYPOGRAPHY_ROUTE,
    };
    let _ = (
        ACTIONS_ROUTE,
        FEEDBACK_ROUTE,
        INPUTS_ROUTE,
        LAYOUT_ROUTE,
        OVERLAYS_ROUTE,
        OVERVIEW_ROUTE,
        STATEFUL_ROUTE,
        THEMES_ROUTE,
        TYPOGRAPHY_ROUTE,
    );

    // Match each route by name. (Route<P> isn't Copy on its own;
    // these consts are.)
    match name {
        "overview" => ui! {
            link(route = &OVERVIEW_ROUTE, params = ()) {
                text(style = style) { label_text }
            }
        },
        "themes" => ui! {
            link(route = &THEMES_ROUTE, params = ()) {
                text(style = style) { label_text }
            }
        },
        "layout" => ui! {
            link(route = &LAYOUT_ROUTE, params = ()) {
                text(style = style) { label_text }
            }
        },
        "typography" => ui! {
            link(route = &TYPOGRAPHY_ROUTE, params = ()) {
                text(style = style) { label_text }
            }
        },
        "actions" => ui! {
            link(route = &ACTIONS_ROUTE, params = ()) {
                text(style = style) { label_text }
            }
        },
        "inputs" => ui! {
            link(route = &INPUTS_ROUTE, params = ()) {
                text(style = style) { label_text }
            }
        },
        "feedback" => ui! {
            link(route = &FEEDBACK_ROUTE, params = ()) {
                text(style = style) { label_text }
            }
        },
        "overlays" => ui! {
            link(route = &OVERLAYS_ROUTE, params = ()) {
                text(style = style) { label_text }
            }
        },
        "stateful" => ui! {
            link(route = &STATEFUL_ROUTE, params = ()) {
                text(style = style) { label_text }
            }
        },
        _ => ui! { text { label_text } },
    }
}

// =============================================================================
// Per-page surface helpers — small wrappers reused by every page
// =============================================================================

/// Container for a single component demo: title + description +
/// preview/controls row. Pages compose multiple of these per
/// page.
pub fn demo_card(
    title: &str,
    description: &str,
    preview: Element,
    controls: Element,
) -> Element {
    use crate::styles::{ControlsBox, DemoCard, DemoRow, PreviewBox};
    let title_text = title.to_string();
    let desc_text = description.to_string();
    let card_style = DemoCard();
    let row_style = DemoRow();
    let preview_style = PreviewBox();
    let controls_style = ControlsBox();

    let preview_box = ui! {
        view(style = preview_style) { preview }
    };
    let controls_box = ui! {
        view(style = controls_style) { controls }
    };
    let row = ui! {
        view(style = row_style) {
            preview_box
            controls_box
        }
    };

    let body_node = if desc_text.is_empty() {
        ui! { view {} }
    } else {
        ui! { Typography(content = desc_text, muted = true) }
    };

    ui! {
        view(style = card_style) {
            Typography(content = title_text, kind = idea_ui::typography_kind::H2)
            body_node
            row
        }
    }
}

/// Page title block — every page calls this at the top.
pub fn page_header(title: &str, description: &str) -> Element {
    let title_text = title.to_string();
    let desc_text = description.to_string();
    let children: Vec<Element> = vec![
        ui! { Typography(content = title_text, kind = idea_ui::typography_kind::H1) },
        ui! { Typography(content = desc_text, muted = true) },
    ];
    ui! {
        Stack(gap = StackGap::Sm) { children }
    }
}
