//! Persistent shell: the `AppShell` sidebar panel (pinned on wide
//! viewports, off-canvas drawer below them), the mobile header, plus
//! page-level helpers used by every page.
//!
//! The sidebar builds ONCE into the AppShell panel and survives every
//! navigation; each link reads the navigator's reactive `active_route`
//! inside its style closure, so the active highlight updates without
//! rebuilding the tree.

use std::rc::Rc;

use runtime_core::{
    component, pressable, text, ui, Element, IntoElement, SafeAreaSides, Signal,
    StyleApplication,
};
use idea_ui::{Typography, Card, dark_theme, Divider, light_theme, set_idea_theme, Stack, Switch, StackGap, StackPadding};

use crate::routes::SECTIONS;
use crate::styles::{
    CodeBlockSheet, CodeBlockText, MobileHeader, MobileHeaderButton, MobileHeaderTitle,
    MobileHeaderTitleWrap, NavLinkBox, NavLinkText, Sidebar, SidebarHeader, SidebarSection,
    SidebarSectionLabel,
};

// =============================================================================
// Sidebar — the AppShell panel content. Built exactly once; the same
// nodes serve both the pinned column and the off-canvas drawer.
// =============================================================================

/// Build the persistent sidebar. `active_route` is the navigator's
/// reactive route mirror (from `SwapContext`); `is_dark` is the
/// app-level theme flag the dark-mode toggle drives.
pub fn sidebar(active_route: Signal<&'static str>, is_dark: Signal<bool>) -> Element {
    let container_style = Sidebar();
    let header_style = SidebarHeader();

    let header_children: Vec<Element> = vec![
        ui! { Typography(content = "Idealyst".to_string(), kind = idea_ui::typography_kind::H2) },
        ui! {
            Typography(
                content = "Cross-platform Rust framework".to_string(),
                muted = true,
            )
        },
    ];

    // ScrollView so the panel scrolls when content exceeds height
    // (long nav, small screen). `.safe_area(ALL)` keeps the brand
    // out of the status bar and the theme toggle out of the home
    // indicator — backends call `set_safe_area_insets(...)`; the
    // padding reactively tracks orientation changes and pin/unpin.
    ui! {
        scroll_view(style = container_style) {
            view(style = header_style) { header_children }
            for s in SECTIONS {
                sidebar_section(s, active_route)
            }
            Divider()
            theme_toggle(is_dark)
        }
            .safe_area(SafeAreaSides::ALL)
    }
}

// =============================================================================
// Swap-layout chrome helpers — the `ui!` call sites live in `lib.rs`.
// =============================================================================

/// Header-over-outlet column for the swap layout in `app()`.
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

/// The active screen's sidebar label — drives the mobile header's
/// reactive title (the per-screen `.title(...)` chrome is gone with
/// the outlet model). Falls back to the route name for any route
/// missing from `SECTIONS`.
fn label_for_route(route_name: &'static str) -> &'static str {
    for s in SECTIONS {
        for entry in s.items {
            if entry.name == route_name {
                return entry.label;
            }
        }
    }
    route_name
}

/// Mobile-style top bar — hamburger on the left, the active page's
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
    let title_source = move || label_for_route(active_route.get()).to_string();
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

fn sidebar_section(
    s: &'static crate::routes::IndexSection,
    active_route: Signal<&'static str>,
) -> Element {
    let section_style = SidebarSection();
    let label_style = SidebarSectionLabel();
    let label_text = s.label.to_string();

    let mut entries: Vec<Element> = Vec::with_capacity(s.items.len() + 1);
    entries.push(ui! { text(style = label_style) { label_text } });
    for entry in s.items {
        entries.push(nav_link(entry.name, entry.label, active_route));
    }

    ui! {
        view(style = section_style) { entries }
    }
}

fn theme_toggle(is_dark: Signal<bool>) -> Element {
    let on_dark_change: Rc<dyn Fn(bool)> = Rc::new(move |dark| {
        is_dark.set(dark);
        if dark {
            set_idea_theme(dark_theme());
        } else {
            set_idea_theme(light_theme());
        }
    });

    let row_children: Vec<Element> = vec![
        ui! { Typography(content = "Theme".to_string(), kind = idea_ui::typography_kind::Caption) },
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
) -> Element {
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
            link(route = &INTRODUCTION_ROUTE, params = ()) {
                view(style = box_style) {
                    text(style = text_style) { label_text }
                }
            }
        },
        "overview" => ui! {
            link(route = &OVERVIEW_ROUTE, params = ()) {
                view(style = box_style) {
                    text(style = text_style) { label_text }
                }
            }
        },
        "quickstart" => ui! {
            link(route = &QUICKSTART_ROUTE, params = ()) {
                view(style = box_style) {
                    text(style = text_style) { label_text }
                }
            }
        },
        "components" => ui! {
            link(route = &COMPONENTS_ROUTE, params = ()) {
                view(style = box_style) {
                    text(style = text_style) { label_text }
                }
            }
        },
        "reactivity" => ui! {
            link(route = &REACTIVITY_ROUTE, params = ()) {
                view(style = box_style) {
                    text(style = text_style) { label_text }
                }
            }
        },
        "ui-dsl" => ui! {
            link(route = &UI_DSL_ROUTE, params = ()) {
                view(style = box_style) {
                    text(style = text_style) { label_text }
                }
            }
        },
        "primitives" => ui! {
            link(route = &PRIMITIVES_ROUTE, params = ()) {
                view(style = box_style) {
                    text(style = text_style) { label_text }
                }
            }
        },
        "styles" => ui! {
            link(route = &STYLES_ROUTE, params = ()) {
                view(style = box_style) {
                    text(style = text_style) { label_text }
                }
            }
        },
        "animation" => ui! {
            link(route = &ANIMATION_ROUTE, params = ()) {
                view(style = box_style) {
                    text(style = text_style) { label_text }
                }
            }
        },
        "navigation" => ui! {
            link(route = &NAVIGATION_ROUTE, params = ()) {
                view(style = box_style) {
                    text(style = text_style) { label_text }
                }
            }
        },
        "macros" => ui! {
            link(route = &MACROS_ROUTE, params = ()) {
                view(style = box_style) {
                    text(style = text_style) { label_text }
                }
            }
        },
        "cli" => ui! {
            link(route = &CLI_ROUTE, params = ()) {
                view(style = box_style) {
                    text(style = text_style) { label_text }
                }
            }
        },
        "platforms" => ui! {
            link(route = &PLATFORMS_ROUTE, params = ()) {
                view(style = box_style) {
                    text(style = text_style) { label_text }
                }
            }
        },
        "simulator" => ui! {
            link(route = &SIMULATOR_ROUTE, params = ()) {
                view(style = box_style) {
                    text(style = text_style) { label_text }
                }
            }
        },
        "lists" => ui! {
            link(route = &LISTS_ROUTE, params = ()) {
                view(style = box_style) {
                    text(style = text_style) { label_text }
                }
            }
        },
        "icons" => ui! {
            link(route = &ICONS_ROUTE, params = ()) {
                view(style = box_style) {
                    text(style = text_style) { label_text }
                }
            }
        },
        "refs" => ui! {
            link(route = &REFS_ROUTE, params = ()) {
                view(style = box_style) {
                    text(style = text_style) { label_text }
                }
            }
        },
        "portal" => ui! {
            link(route = &PORTAL_ROUTE, params = ()) {
                view(style = box_style) {
                    text(style = text_style) { label_text }
                }
            }
        },
        "robot" => ui! {
            link(route = &ROBOT_ROUTE, params = ()) {
                view(style = box_style) {
                    text(style = text_style) { label_text }
                }
            }
        },
        "dev-tools" => ui! {
            link(route = &DEV_TOOLS_ROUTE, params = ()) {
                view(style = box_style) {
                    text(style = text_style) { label_text }
                }
            }
        },
        "backends" => ui! {
            link(route = &BACKENDS_ROUTE, params = ()) {
                view(style = box_style) {
                    text(style = text_style) { label_text }
                }
            }
        },
        "writing-a-backend" => ui! {
            link(route = &WRITING_A_BACKEND_ROUTE, params = ()) {
                view(style = box_style) {
                    text(style = text_style) { label_text }
                }
            }
        },
        "third-party-primitives" => ui! {
            link(route = &THIRD_PARTY_PRIMITIVES_ROUTE, params = ()) {
                view(style = box_style) {
                    text(style = text_style) { label_text }
                }
            }
        },
        "building-a-theme-system" => ui! {
            link(route = &BUILDING_A_THEME_SYSTEM_ROUTE, params = ()) {
                view(style = box_style) {
                    text(style = text_style) { label_text }
                }
            }
        },
        "reactive-text-bindings" => ui! {
            link(route = &REACTIVE_TEXT_BINDINGS_ROUTE, params = ()) {
                view(style = box_style) {
                    text(style = text_style) { label_text }
                }
            }
        },
        "wgpu-native-api" => ui! {
            link(route = &WGPU_NATIVE_API_ROUTE, params = ()) {
                view(style = box_style) {
                    text(style = text_style) { label_text }
                }
            }
        },
        _ => ui! { text { label_text } },
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
// sidebar Element via `.sidebar_with(builder)`.

// =============================================================================
// Per-page surface helpers — exposed as `#[component]`s so pages
// invoke them through the `ui!` macro (`PageHeader(...)` etc.)
// rather than as plain function calls.
// =============================================================================

#[derive(Default)]
pub struct PageTypographyProps {
    pub children: Vec<Element>,
}

/// `PageBody { ... }` — the per-page surface every doc screen wraps
/// its content in. Bakes in the recurring `scroll_view { Stack(gap,
/// padding) { ... } }` shape so every page lays out identically
/// (gap = `Xl`, padding = `Lg`). Pages just drop their `PageHeader`
/// / `Card` / `Section` children inside.
///
/// Hardcoded by design — the docs site wants a single consistent
/// page layout. If you need a one-off override, hand-roll the
/// `scroll_view { Stack(...) { ... } }` instead.
#[component]
pub fn PageBody(props: PageTypographyProps) -> Element {
    let children = props.children;
    ui! {
        scroll_view {
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
pub fn PageHeader(props: PageHeaderProps) -> Element {
    let title = props.title;
    let description = props.description;
    let children: Vec<Element> = vec![
        ui! { Typography(content = title, kind = idea_ui::typography_kind::H1) },
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
pub fn Section(props: SectionProps) -> Element {
    let title = props.title;
    let body_text = props.body;
    ui! {
        Card {
            Typography(content = title, kind = idea_ui::typography_kind::H2)
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
pub fn SectionWithCode(props: SectionWithCodeProps) -> Element {
    let title = props.title;
    let body_text = props.body;
    let code_text = props.code;
    let code_style = CodeBlockSheet();
    let code_text_style = CodeBlockText();

    ui! {
        Card {
            Typography(content = title, kind = idea_ui::typography_kind::H2)
            Typography(content = body_text, muted = true)
            view(style = code_style) {
                text(style = code_text_style) { code_text }
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
pub fn CodeBlock(props: CodeBlockProps) -> Element {
    let code_text = props.code;
    let code_style = CodeBlockSheet();
    let code_text_style = CodeBlockText();
    ui! {
        view(style = code_style) {
            text(style = code_text_style) { code_text }
        }
    }
}

