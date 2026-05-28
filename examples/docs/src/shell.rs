//! Persistent shell: sidebar (web) / drawer (mobile) with route
//! links, plus page-level helpers used by every page.
//!
//! The drawer navigator handles per-platform behavior automatically
//! — on web the sidebar pins beside the body, on mobile the same
//! content slides in as a drawer.

use std::rc::Rc;

use runtime_core::{component, ui, Primitive, SafeAreaSides, Signal, StyleApplication};
use drawer_navigator::DrawerSlotProps;
use idea_ui::{Typography, Card, dark_theme, Divider, light_theme, set_idea_theme, Stack, Switch, StackGap, StackPadding};

use crate::routes::SECTIONS;
use crate::styles::{
    CodeBlockSheet, CodeBlockText, Content, NavLinkBox, NavLinkText, PageRoot, Sidebar,
    SidebarHeader, SidebarSection, SidebarSectionLabel,
};

// =============================================================================
// Drawer content — the side panel itself. The framework hands this
// to the web layout (via `LayoutProps::sidebar`) on web; native
// backends render it as the drawer panel directly.
// =============================================================================

pub fn content_builder(
    is_dark: Signal<bool>,
) -> impl Fn(DrawerSlotProps) -> Primitive + 'static {
    move |props: DrawerSlotProps| {
        let active_route = props.active_route;
        drawer_content(active_route, is_dark)
    }
}

fn drawer_content(active_route: Signal<&'static str>, is_dark: Signal<bool>) -> Primitive {
    let container_style = Sidebar();
    let header_style = SidebarHeader();

    let header_children: Vec<Primitive> = vec![
        ui! { Typography(content = "Idealyst".to_string(), kind = idea_ui::typography_kind::H2.into()) },
        ui! {
            Typography(
                content = "Cross-platform Rust framework".to_string(),
                muted = true,
            )
        },
    ];

    let mut children: Vec<Primitive> = Vec::new();
    children.push(ui! { View(style = header_style) { header_children } });

    for s in SECTIONS {
        children.push(sidebar_section(s, active_route));
    }

    children.push(ui! { Divider() });
    children.push(theme_toggle(is_dark));

    // ScrollView so the panel scrolls when content exceeds height
    // (long nav, small screen). `.safe_area(ALL)` keeps the brand
    // out of the status bar and the theme toggle out of the home
    // indicator — backends call `set_safe_area_insets(...)`; the
    // padding reactively tracks orientation changes and pin/unpin.
    ui! {
        ScrollView(style = container_style) { children }
            .safe_area(SafeAreaSides::ALL)
    }
}

fn sidebar_section(
    s: &'static crate::routes::IndexSection,
    active_route: Signal<&'static str>,
) -> Primitive {
    let section_style = SidebarSection();
    let label_style = SidebarSectionLabel();
    let label_text = s.label.to_string();

    let mut entries: Vec<Primitive> = Vec::with_capacity(s.items.len() + 1);
    entries.push(ui! { Text(style = label_style) { label_text } });
    for entry in s.items {
        entries.push(nav_link(entry.name, entry.label, active_route));
    }

    ui! {
        View(style = section_style) { entries }
    }
}

fn theme_toggle(is_dark: Signal<bool>) -> Primitive {
    let on_dark_change: Rc<dyn Fn(bool)> = Rc::new(move |dark| {
        is_dark.set(dark);
        if dark {
            set_idea_theme(dark_theme());
        } else {
            set_idea_theme(light_theme());
        }
    });

    let row_children: Vec<Primitive> = vec![
        ui! { Typography(content = "Theme".to_string(), kind = idea_ui::typography_kind::Caption.into()) },
        ui! {
            Switch(
                label = Some("Dark mode".to_string()),
                value = is_dark,
                on_change = on_dark_change,
            )
        },
    ];

    ui! {
        Stack(gap = StackGap::Xs) { row_children }
    }
}

// =============================================================================
// Nav link — anchored to a Route; reactive active-highlight.
// =============================================================================

fn nav_link(
    name: &'static str,
    label: &'static str,
    active_route: Signal<&'static str>,
) -> Primitive {
    let label_text = label.to_string();
    let route_for_match: &str = name;
    // Padding/background/border-radius belong to the outer pill (a
    // View) — padding on a Text node is a framework no-op. The text
    // color flips on `active` too, so the inner Text gets its own
    // style closure subscribed to the same signal.
    let box_style = move || {
        let variant = if active_route.get() == route_for_match { "on" } else { "off" };
        StyleApplication::new(NavLinkBox::sheet()).with("active", variant.to_string())
    };
    let text_style = move || {
        let variant = if active_route.get() == route_for_match { "on" } else { "off" };
        StyleApplication::new(NavLinkText::sheet()).with("active", variant.to_string())
    };

    use crate::routes::{
        ANIMATION_ROUTE, BACKENDS_ROUTE, BUILDING_A_THEME_SYSTEM_ROUTE, CLI_ROUTE,
        COMPONENTS_ROUTE, DEV_TOOLS_ROUTE, ICONS_ROUTE, INTRODUCTION_ROUTE, LISTS_ROUTE,
        MACROS_ROUTE, NAVIGATION_ROUTE, OVERVIEW_ROUTE, PLATFORMS_ROUTE, PORTAL_ROUTE,
        PRIMITIVES_ROUTE, QUICKSTART_ROUTE, REACTIVE_TEXT_BINDINGS_ROUTE, REACTIVITY_ROUTE,
        REFS_ROUTE, ROBOT_ROUTE, SIMULATOR_ROUTE, STYLES_ROUTE, THIRD_PARTY_PRIMITIVES_ROUTE,
        UI_DSL_ROUTE, WGPU_NATIVE_API_ROUTE, WRITING_A_BACKEND_ROUTE,
    };

    match name {
        "introduction" => ui! {
            Link(route = &INTRODUCTION_ROUTE, params = ()) {
                View(style = box_style) {
                    Text(style = text_style) { label_text }
                }
            }
        },
        "overview" => ui! {
            Link(route = &OVERVIEW_ROUTE, params = ()) {
                View(style = box_style) {
                    Text(style = text_style) { label_text }
                }
            }
        },
        "quickstart" => ui! {
            Link(route = &QUICKSTART_ROUTE, params = ()) {
                View(style = box_style) {
                    Text(style = text_style) { label_text }
                }
            }
        },
        "components" => ui! {
            Link(route = &COMPONENTS_ROUTE, params = ()) {
                View(style = box_style) {
                    Text(style = text_style) { label_text }
                }
            }
        },
        "reactivity" => ui! {
            Link(route = &REACTIVITY_ROUTE, params = ()) {
                View(style = box_style) {
                    Text(style = text_style) { label_text }
                }
            }
        },
        "ui-dsl" => ui! {
            Link(route = &UI_DSL_ROUTE, params = ()) {
                View(style = box_style) {
                    Text(style = text_style) { label_text }
                }
            }
        },
        "primitives" => ui! {
            Link(route = &PRIMITIVES_ROUTE, params = ()) {
                View(style = box_style) {
                    Text(style = text_style) { label_text }
                }
            }
        },
        "styles" => ui! {
            Link(route = &STYLES_ROUTE, params = ()) {
                View(style = box_style) {
                    Text(style = text_style) { label_text }
                }
            }
        },
        "animation" => ui! {
            Link(route = &ANIMATION_ROUTE, params = ()) {
                View(style = box_style) {
                    Text(style = text_style) { label_text }
                }
            }
        },
        "navigation" => ui! {
            Link(route = &NAVIGATION_ROUTE, params = ()) {
                View(style = box_style) {
                    Text(style = text_style) { label_text }
                }
            }
        },
        "macros" => ui! {
            Link(route = &MACROS_ROUTE, params = ()) {
                View(style = box_style) {
                    Text(style = text_style) { label_text }
                }
            }
        },
        "cli" => ui! {
            Link(route = &CLI_ROUTE, params = ()) {
                View(style = box_style) {
                    Text(style = text_style) { label_text }
                }
            }
        },
        "platforms" => ui! {
            Link(route = &PLATFORMS_ROUTE, params = ()) {
                View(style = box_style) {
                    Text(style = text_style) { label_text }
                }
            }
        },
        "simulator" => ui! {
            Link(route = &SIMULATOR_ROUTE, params = ()) {
                View(style = box_style) {
                    Text(style = text_style) { label_text }
                }
            }
        },
        "lists" => ui! {
            Link(route = &LISTS_ROUTE, params = ()) {
                View(style = box_style) {
                    Text(style = text_style) { label_text }
                }
            }
        },
        "icons" => ui! {
            Link(route = &ICONS_ROUTE, params = ()) {
                View(style = box_style) {
                    Text(style = text_style) { label_text }
                }
            }
        },
        "refs" => ui! {
            Link(route = &REFS_ROUTE, params = ()) {
                View(style = box_style) {
                    Text(style = text_style) { label_text }
                }
            }
        },
        "portal" => ui! {
            Link(route = &PORTAL_ROUTE, params = ()) {
                View(style = box_style) {
                    Text(style = text_style) { label_text }
                }
            }
        },
        "robot" => ui! {
            Link(route = &ROBOT_ROUTE, params = ()) {
                View(style = box_style) {
                    Text(style = text_style) { label_text }
                }
            }
        },
        "dev-tools" => ui! {
            Link(route = &DEV_TOOLS_ROUTE, params = ()) {
                View(style = box_style) {
                    Text(style = text_style) { label_text }
                }
            }
        },
        "backends" => ui! {
            Link(route = &BACKENDS_ROUTE, params = ()) {
                View(style = box_style) {
                    Text(style = text_style) { label_text }
                }
            }
        },
        "writing-a-backend" => ui! {
            Link(route = &WRITING_A_BACKEND_ROUTE, params = ()) {
                View(style = box_style) {
                    Text(style = text_style) { label_text }
                }
            }
        },
        "third-party-primitives" => ui! {
            Link(route = &THIRD_PARTY_PRIMITIVES_ROUTE, params = ()) {
                View(style = box_style) {
                    Text(style = text_style) { label_text }
                }
            }
        },
        "building-a-theme-system" => ui! {
            Link(route = &BUILDING_A_THEME_SYSTEM_ROUTE, params = ()) {
                View(style = box_style) {
                    Text(style = text_style) { label_text }
                }
            }
        },
        "reactive-text-bindings" => ui! {
            Link(route = &REACTIVE_TEXT_BINDINGS_ROUTE, params = ()) {
                View(style = box_style) {
                    Text(style = text_style) { label_text }
                }
            }
        },
        "wgpu-native-api" => ui! {
            Link(route = &WGPU_NATIVE_API_ROUTE, params = ()) {
                View(style = box_style) {
                    Text(style = text_style) { label_text }
                }
            }
        },
        _ => ui! { Text { label_text } },
    }
}

// =============================================================================
// Web layout — places the drawer's pre-built sidebar beside the
// outlet. Native backends draw their own drawer chrome and ignore
// this slot.
//
// Compiled unconditionally so the runtime-server recording backend (which runs
// natively on the dev host) can invoke it too: its `create_drawer_
// navigator` executes the closure, captures every node it builds as
// wire commands, and ships the layout to the browser via
// `Command::AttachNavigatorLayout`.
// =============================================================================

// web_layout removed — `.layout(...)` is no longer part of the
// drawer-navigator API. On web, the drawer SDK's chrome arranges
// the sidebar + screen outlet itself; the author just passes the
// sidebar Primitive via `.sidebar_with(builder)`.

// =============================================================================
// Per-page surface helpers — exposed as `#[component]`s so pages
// invoke them through the `ui!` macro (`PageHeader(...)` etc.)
// rather than as plain function calls.
// =============================================================================

#[derive(Default)]
pub struct PageTypographyProps {
    pub children: Vec<Primitive>,
}

/// `PageBody { ... }` — the per-page surface every doc screen wraps
/// its content in. Bakes in the recurring `ScrollView { Stack(gap,
/// padding) { ... } }` shape so every page lays out identically
/// (gap = `Xl`, padding = `Lg`). Pages just drop their `PageHeader`
/// / `Card` / `Section` children inside.
///
/// Hardcoded by design — the docs site wants a single consistent
/// page layout. If you need a one-off override, hand-roll the
/// `ScrollView { Stack(...) { ... } }` instead.
#[component]
pub fn PageBody(props: PageTypographyProps) -> Primitive {
    let children = props.children;
    ui! {
        ScrollView {
            Stack(gap = StackGap::Xl, padding = StackPadding::Lg) {
                children
            }
        }
    }
}

#[derive(Default)]
pub struct PageHeaderProps {
    pub title: String,
    pub description: String,
}

/// `PageHeader(title = "...", description = "...")` — the H1 + subtitle
/// every page opens with.
#[component]
pub fn PageHeader(props: PageHeaderProps) -> Primitive {
    let title = props.title;
    let description = props.description;
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = title, kind = idea_ui::typography_kind::H1.into()) },
        ui! { Typography(content = description, muted = true) },
    ];
    ui! {
        Stack(gap = StackGap::Sm) { children }
    }
}

#[derive(Default)]
pub struct SectionProps {
    pub title: String,
    pub body: String,
}

/// `Section(title = "...", body = "...")` — a card with an H2 and a
/// muted body paragraph. Use [`SectionWithCode`] when the section
/// also has a code sample, or compose a `Card { ... }` by hand for
/// richer layouts.
#[component]
pub fn Section(props: SectionProps) -> Primitive {
    let title = props.title;
    let body_text = props.body;
    ui! {
        Card {
            Typography(content = title, kind = idea_ui::typography_kind::H2.into())
            Typography(content = body_text, muted = true)
        }
    }
}

#[derive(Default)]
pub struct SectionWithCodeProps {
    pub title: String,
    pub body: String,
    pub code: String,
}

/// `SectionWithCode(title = "...", body = "...", code = "...")` —
/// section card with a code block under the prose.
#[component]
pub fn SectionWithCode(props: SectionWithCodeProps) -> Primitive {
    let title = props.title;
    let body_text = props.body;
    let code_text = props.code;
    let code_style = CodeBlockSheet();
    let code_text_style = CodeBlockText();

    ui! {
        Card {
            Typography(content = title, kind = idea_ui::typography_kind::H2.into())
            Typography(content = body_text, muted = true)
            View(style = code_style) {
                Text(style = code_text_style) { code_text }
            }
        }
    }
}

#[derive(Default)]
pub struct CodeBlockProps {
    pub code: String,
}

/// `CodeBlock(code = "...")` — standalone code surface, no card
/// chrome. Use inside a hand-rolled `Card { ... }` when you need
/// multiple code blocks or interleaved prose.
#[component]
pub fn CodeBlock(props: CodeBlockProps) -> Primitive {
    let code_text = props.code;
    let code_style = CodeBlockSheet();
    let code_text_style = CodeBlockText();
    ui! {
        View(style = code_style) {
            Text(style = code_text_style) { code_text }
        }
    }
}

