//! The persistent sidebar (built once into the `AppShell` panel), the
//! mobile header, and the per-screen layout wrapper.
//!
//! The sidebar walks `course::CHAPTERS` to render numbered chapter
//! headers and links to the written lessons. Each link reads the navigator's reactive `active_route`
//! inside its style closure, so the active highlight updates reactively
//! without rebuilding the tree — the same pattern the marketing site
//! uses.

use std::rc::Rc;

use idea_ui::IdeaThemeRef;
use runtime_core::{
    derived, pressable, stylesheet, text, ui, Element, IntoElement, Signal, StyleApplication,
};
use idea_ui::{dark_theme, light_theme, set_idea_theme, Switch, Typography};

use crate::course::{self, CHAPTERS};
use crate::routes::HOME_ROUTE;
use crate::styles::{
    MobileHeader, MobileHeaderButton, MobileHeaderTitle, MobileHeaderTitleWrap, NavLink,
    NavLinkActive, ScreenScroll, SidebarBody, SidebarFooter, SidebarHeader, SidebarScroll,
    SidebarSection, SidebarSoon, SidebarToggle, SidebarToggleGlyph,
};

/// Wrap a step page in its own scroll surface (background, text color).
/// The navigator outlet does not scroll — the screen owns scroll, so
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

// The content area behind the page card: the secondary background the
// sidebar shares. The card is inset only on the top and left — it runs off
// the window's right and bottom edges, so only its top-left corner shows
// the framing. Tighter on a narrow screen.
stylesheet! {
    pub ContentBackdrop<IdeaThemeRef> {
        base(t) {
            flex_direction: runtime_core::FlexDirection::Column,
            flex_grow: 1.0,
            min_height: 0.0,
            padding_top: t.spacing.sm(),
            padding_left: t.spacing.sm(),
            background: t.color.surface_alt(),
        }
        breakpoint lg(t) {
            padding_top: t.spacing.md(),
            padding_left: t.spacing.md(),
        }
        transitions {
            background: 250ms EaseInOut,
        }
    }
}

/// The per-instance override that turns idea-ui's elevated `Card` into
/// the page surface: the white `surface` fill (Elevated's own fill is
/// `surface_alt`, the backdrop's color), no border, square corners except
/// the top-left (the others sit on the window's edges), clipping for the
/// page's scrollers, and the content column's full size.
pub fn content_card_rules() -> runtime_core::StyleRules {
    let t = idea_ui::tokens();
    runtime_core::StyleRules {
        background: Some(t.color.surface()),
        border_top_width: Some(0.0.into()),
        border_right_width: Some(0.0.into()),
        border_bottom_width: Some(0.0.into()),
        border_left_width: Some(0.0.into()),
        border_top_right_radius: Some(runtime_core::Length::Px(0.0).into()),
        border_bottom_left_radius: Some(runtime_core::Length::Px(0.0).into()),
        border_bottom_right_radius: Some(runtime_core::Length::Px(0.0).into()),
        overflow: Some(runtime_core::Overflow::Hidden),
        flex_grow: Some(1.0f32.into()),
        min_height: Some(runtime_core::Length::Px(0.0).into()),
        ..Default::default()
    }
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
        crate::label_for_route(route).unwrap_or_else(|| "The Idealyst Course".to_string())
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

/// The pinned sidebar's width. Hidden, it is 0 and the page card takes
/// the whole window.
pub const SIDEBAR_WIDTH: f32 = 280.0;

/// Whether the pinned sidebar is hidden — app state, provided as context
/// so the toggle beside each page's title reaches it. A newtype so the
/// context key is this, not every `Signal<bool>`.
#[derive(Clone, Copy)]
pub struct SidebarHidden(pub Signal<bool>);

/// The sidebar's current width. Hiding only applies while it is pinned:
/// below the pin width it is the off-canvas drawer, which always opens
/// full width. Reactive — call it in `rx!`.
pub fn sidebar_width(hidden: Signal<bool>) -> f32 {
    if hidden.get() && idea_ui_nav::sidebar_pinned(runtime_core::Breakpoint::Lg) {
        0.0
    } else {
        SIDEBAR_WIDTH
    }
}

/// The «/» button beside a page's title that hides or shows the pinned
/// sidebar. Below the pin width it measures to nothing (`SidebarToggle`'s
/// breakpoint), since the hamburger opens the drawer there.
pub fn sidebar_toggle() -> Element {
    let Some(SidebarHidden(hidden)) = runtime_core::inject::<SidebarHidden>() else {
        return ui! { view {} };
    };
    let glyph = text(move || if hidden.get() { "\u{00bb}" } else { "\u{00ab}" }.to_string())
        .with_style(move || StyleApplication::new(SidebarToggleGlyph::sheet()))
        .into_element();
    pressable(vec![glyph], move || hidden.update(|h| !h))
        .with_style(SidebarToggle())
        .a11y_label("Show or hide the sidebar")
        .into_element()
}

/// Build the persistent sidebar. Runs once (the AppShell panel mounts
/// it a single time); its reactive scope lives for the navigator's
/// whole lifetime.
pub fn sidebar(active_route: Signal<&'static str>, is_dark: Signal<bool>) -> Element {
    ui! {
        // The WHOLE sidebar scrolls as one — header, nav list, and the theme
        // toggle all move together when the content is taller than the panel.
        scroll_view(style = SidebarScroll()) {
            view(style = SidebarBody()) {
                view(style = SidebarHeader()) {
                    Typography(
                        content = "The Idealyst Course".to_string(),
                        kind = idea_ui::typography_kind::H3,
                    )
                    Typography(
                        content = "From zero to a shipping app, in Rust.".to_string(),
                        muted = true,
                    )
                }
                nav_link(&HOME_ROUTE, "Course home".to_string(), active_route)
                for (c, chapter) in CHAPTERS.iter().enumerate() {
                    text(style = SidebarSection()) {
                        format!("{} \u{00b7} {}", course::chapter_number(c), chapter.title)
                    }
                    for (i, lesson) in chapter.lessons.iter().enumerate() {
                        if let Some(route) = lesson.route {
                            nav_link(
                                route,
                                format!("{}  {}", course::lesson_number(c, i), lesson.title),
                                active_route,
                            )
                        }
                    }
                    if chapter.lessons.iter().all(|l| l.route.is_none()) {
                        text(style = SidebarSoon()) { "Coming soon" }
                    }
                }
                theme_toggle(SidebarFooter(), is_dark)
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

    ui! {
        view(style = footer_style) {
            Switch(
                label = Some("Dark mode".to_string()),
                value = is_dark,
                on_change = on_change,
            )
        }
    }
}

/// One sidebar nav link. The `active` axis is derived reactively from
/// `active_route`, so the highlight flips without rebuilding the link.
fn nav_link(
    route: &'static runtime_core::Route<()>,
    label: String,
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
    let label_text = label;

    ui! {
        link(route = route, params = ()) {
            text(style = style) { label_text }
        }
    }
}
