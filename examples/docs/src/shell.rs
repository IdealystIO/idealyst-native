//! Persistent shell: sidebar (web) / drawer (mobile) with route
//! links, plus page-level helpers used by every page.
//!
//! The drawer navigator handles per-platform behavior automatically
//! — on web the sidebar pins beside the body, on mobile the same
//! content slides in as a drawer.

use std::rc::Rc;

use framework_core::{
    component, ui, DrawerContentProps, LayoutProps, Primitive, SafeAreaSides, Signal,
    StyleApplication,
};
use idea_ui::{
    body, caption, card, dark_theme, divider, heading, light_theme, set_idea_theme, stack, switch,
    BodyTone, HeadingKind, StackGap, StackPadding,
};

use crate::routes::SECTIONS;
use crate::styles::{
    CodeBlockSheet, CodeBlockText, Content, NavLink, PageRoot, Sidebar, SidebarHeader,
    SidebarSection, SidebarSectionLabel,
};

// =============================================================================
// Drawer content — the side panel itself. The framework hands this
// to the web layout (via `LayoutProps::sidebar`) on web; native
// backends render it as the drawer panel directly.
// =============================================================================

pub fn content_builder(
    is_dark: Signal<bool>,
) -> impl Fn(DrawerContentProps) -> Primitive + 'static {
    move |props: DrawerContentProps| {
        let active_route = props.active_route;
        drawer_content(active_route, is_dark)
    }
}

fn drawer_content(active_route: Signal<&'static str>, is_dark: Signal<bool>) -> Primitive {
    let container_style = Sidebar();
    let header_style = SidebarHeader();

    let header_children: Vec<Primitive> = vec![
        ui! { Heading(content = "Idealyst".to_string(), kind = HeadingKind::H2) },
        ui! {
            Body(
                content = "Cross-platform Rust framework".to_string(),
                tone = BodyTone::Muted,
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
        ui! { Caption(content = "Theme".to_string()) },
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
    let style = move || {
        let variant = if active_route.get() == route_for_match {
            "on"
        } else {
            "off"
        };
        StyleApplication::new(NavLink::sheet()).with("active", variant.to_string())
    };

    use crate::routes::{
        CLI_ROUTE, COMPONENTS_ROUTE, MACROS_ROUTE, NAVIGATION_ROUTE, OVERVIEW_ROUTE,
        PLATFORMS_ROUTE, PRIMITIVES_ROUTE, QUICKSTART_ROUTE, REACTIVITY_ROUTE, STYLES_ROUTE,
        UI_DSL_ROUTE,
    };

    match name {
        "overview" => ui! {
            Link(route = &OVERVIEW_ROUTE, params = ()) {
                Text(style = style) { label_text }
            }
        },
        "quickstart" => ui! {
            Link(route = &QUICKSTART_ROUTE, params = ()) {
                Text(style = style) { label_text }
            }
        },
        "components" => ui! {
            Link(route = &COMPONENTS_ROUTE, params = ()) {
                Text(style = style) { label_text }
            }
        },
        "reactivity" => ui! {
            Link(route = &REACTIVITY_ROUTE, params = ()) {
                Text(style = style) { label_text }
            }
        },
        "ui-dsl" => ui! {
            Link(route = &UI_DSL_ROUTE, params = ()) {
                Text(style = style) { label_text }
            }
        },
        "primitives" => ui! {
            Link(route = &PRIMITIVES_ROUTE, params = ()) {
                Text(style = style) { label_text }
            }
        },
        "styles" => ui! {
            Link(route = &STYLES_ROUTE, params = ()) {
                Text(style = style) { label_text }
            }
        },
        "navigation" => ui! {
            Link(route = &NAVIGATION_ROUTE, params = ()) {
                Text(style = style) { label_text }
            }
        },
        "macros" => ui! {
            Link(route = &MACROS_ROUTE, params = ()) {
                Text(style = style) { label_text }
            }
        },
        "cli" => ui! {
            Link(route = &CLI_ROUTE, params = ()) {
                Text(style = style) { label_text }
            }
        },
        "platforms" => ui! {
            Link(route = &PLATFORMS_ROUTE, params = ()) {
                Text(style = style) { label_text }
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
// Compiled unconditionally so the AAS recording backend (which runs
// natively on the dev host) can invoke it too: its `create_drawer_
// navigator` executes the closure, captures every node it builds as
// wire commands, and ships the layout to the browser via
// `Command::AttachNavigatorLayout`.
// =============================================================================

pub fn web_layout() -> impl Fn(LayoutProps) -> Primitive + 'static {
    move |props: LayoutProps| {
        let outlet = props.outlet;
        let sidebar_node = props.sidebar;

        let root_style = PageRoot();
        let content_style = Content();

        // ScrollView (not View) around the outlet so the right
        // column scrolls independently of the pinned sidebar. The
        // PageRoot sets `overflow: Hidden`, so the only
        // scrollable regions are the sidebar's own ScrollView (set
        // up by `drawer_content`) and this content-area ScrollView.
        ui! {
            View(style = root_style) {
                sidebar_node
                ScrollView(style = content_style) {
                    outlet
                }
            }
        }
    }
}

// =============================================================================
// Per-page surface helpers — exposed as `#[component]`s so pages
// invoke them through the `ui!` macro (`PageHeader(...)` etc.)
// rather than as plain function calls.
// =============================================================================

#[derive(Default)]
pub struct PageBodyProps {
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
pub fn pagebody(props: PageBodyProps) -> Primitive {
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
pub fn pageheader(props: PageHeaderProps) -> Primitive {
    let title = props.title;
    let description = props.description;
    let children: Vec<Primitive> = vec![
        ui! { Heading(content = title, kind = HeadingKind::H1) },
        ui! { Body(content = description, tone = BodyTone::Muted) },
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
pub fn section(props: SectionProps) -> Primitive {
    let title = props.title;
    let body_text = props.body;
    ui! {
        Card {
            Heading(content = title, kind = HeadingKind::H2)
            Body(content = body_text, tone = BodyTone::Muted)
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
pub fn sectionwithcode(props: SectionWithCodeProps) -> Primitive {
    let title = props.title;
    let body_text = props.body;
    let code_text = props.code;
    let code_style = CodeBlockSheet();
    let code_text_style = CodeBlockText();

    ui! {
        Card {
            Heading(content = title, kind = HeadingKind::H2)
            Body(content = body_text, tone = BodyTone::Muted)
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
pub fn codeblock(props: CodeBlockProps) -> Primitive {
    let code_text = props.code;
    let code_style = CodeBlockSheet();
    let code_text_style = CodeBlockText();
    ui! {
        View(style = code_style) {
            Text(style = code_text_style) { code_text }
        }
    }
}

