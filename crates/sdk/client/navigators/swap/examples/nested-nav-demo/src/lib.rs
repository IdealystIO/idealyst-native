//! `nested-nav-demo` — nested navigators with TWO simultaneous sidebars.
//!
//! The outer [`SwapNavigator`] is the dashboard's primary sidebar (left):
//! `Overview` / `Reports` / `Settings`. Its `Reports` screen BODY is a second
//! `SwapNavigator` that carries its OWN secondary sidebar (right): `Summary` /
//! `Trends` / `Exports`. Each navigator wraps only its own `{nav.outlet}` in
//! its own `.layout(...)`, so when you're inside `Reports` BOTH sidebars are
//! on screen at once, wrapping the innermost tool screen:
//!
//! ```text
//! [ primary sidebar ][ ....... [ tool work area ] [ secondary sidebar ] ]
//!   Overview                     (Summary/Trends/…)   Summary
//!   Reports  ◄ active                                 Trends
//!   Settings                                          Exports
//! ```
//!
//! Switching the primary sidebar to `Overview`/`Settings` swaps the whole
//! nested navigator out, so the secondary sidebar only appears inside
//! `Reports`. Switching the secondary sidebar swaps only the tool work area —
//! both sidebars stay put.

use runtime_core::{
    cached_stylesheet, ui, Color, Element, FlexDirection, Length, Ref, Route, Screen,
    StyleApplication, StyleRules, StyleSheet,
};
use std::rc::Rc;
use swap_navigator::{SwapBuilder, SwapHandle, SwapNavigator};

/// Navigators self-register via `inventory::submit!`; nothing to do here. The
/// CLI bootstrap still calls this hook.
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(backend: &mut dev_server::WireRecordingBackend) {
    swap_navigator::recording::register(backend);
}

// Outer (primary sidebar) routes.
const OVERVIEW: Route<()> = Route::<()>::new("overview", "/");
const REPORTS: Route<()> = Route::<()>::new("reports", "/reports");
const SETTINGS: Route<()> = Route::<()>::new("settings", "/settings");

// Inner (secondary sidebar) routes — RELATIVE to the /reports base. "" is the
// index tool shown when you first enter Reports.
const SUMMARY: Route<()> = Route::<()>::new("summary", "");
const TRENDS: Route<()> = Route::<()>::new("trends", "/trends");
const EXPORTS: Route<()> = Route::<()>::new("exports", "/exports");

#[runtime_core::component]
pub fn app() -> Element {
    let outer: Ref<SwapHandle> = Ref::new();

    let builder = SwapNavigator::new(&OVERVIEW)
        .screen(OVERVIEW, |_| {
            Screen::new(page("Overview", "Your dashboard at a glance."))
        })
        .screen(REPORTS, |_| Screen::new(reports_section()))
        .screen(SETTINGS, |_| {
            Screen::new(page("Settings", "Preferences and configuration."))
        })
        // PRIMARY sidebar (left) wrapping the outer outlet.
        .layout(|nav| {
            let (a, b, c) = (nav.on_select.clone(), nav.on_select.clone(), nav.on_select.clone());
            ui! {
                view(style = row_fill) {
                    view(style = primary_sidebar) {
                        text(style = sidebar_title) { "Sections" }
                        button(label = "Overview", on_click = move || a("overview"))
                        button(label = "Reports", on_click = move || b("reports"))
                        button(label = "Settings", on_click = move || c("settings"))
                    }
                    nav.outlet
                }
            }
        });

    ui! { builder.bind(outer) }
}

/// The `Reports` section: a whole second navigator with its own secondary
/// sidebar on the RIGHT. This element is the outer `Reports` screen's body.
fn reports_section() -> Element {
    let inner: Ref<SwapHandle> = Ref::new();

    let builder = SwapNavigator::new(&SUMMARY)
        .screen(SUMMARY, |_| {
            Screen::new(page("Summary", "Headline report figures."))
        })
        .screen(TRENDS, |_| {
            Screen::new(page("Trends", "How the numbers moved over time."))
        })
        .screen(EXPORTS, |_| {
            Screen::new(page("Exports", "Download the underlying data."))
        })
        // SECONDARY sidebar (right): outlet first, tool sidebar after.
        .layout(|nav| {
            let (a, b, c) = (nav.on_select.clone(), nav.on_select.clone(), nav.on_select.clone());
            ui! {
                view(style = row_fill) {
                    nav.outlet
                    view(style = secondary_sidebar) {
                        text(style = sidebar_title) { "Report tools" }
                        button(label = "Summary", on_click = move || a("summary"))
                        button(label = "Trends", on_click = move || b("trends"))
                        button(label = "Exports", on_click = move || c("exports"))
                    }
                }
            }
        });

    ui! { builder.bind(inner) }
}

fn page(title: &str, body: &str) -> Element {
    let (title, body) = (title.to_string(), body.to_string());
    ui! {
        view(style = page_style) {
            text(style = page_title) { move || title.clone() }
            text(style = page_body) { move || body.clone() }
        }
    }
}

// ---------------------------------------------------------------------------
// Local styles (a demo — a real app would use idea-ui layout + theme tokens).
// ---------------------------------------------------------------------------

fn base() -> Rc<StyleSheet> {
    static KEY: u8 = 0;
    cached_stylesheet(&KEY as *const u8 as usize, || {
        Rc::new(StyleSheet::r#static(StyleRules::default()))
    })
}

/// A full-height row: children lay out left-to-right, filling the viewport.
fn row_fill() -> StyleApplication {
    StyleApplication::new(base()).with_computed("nnd_row_fill", || StyleRules {
        flex_direction: Some(FlexDirection::Row),
        width: Some(Length::Percent(100.0).into()),
        height: Some(Length::Percent(100.0).into()),
        ..Default::default()
    })
}

fn sidebar_common() -> StyleRules {
    StyleRules {
        flex_direction: Some(FlexDirection::Column),
        flex_grow: Some(0.0.into()),
        flex_shrink: Some(0.0.into()),
        width: Some(Length::Px(200.0).into()),
        height: Some(Length::Percent(100.0).into()),
        gap: Some(Length::Px(8.0).into()),
        padding_top: Some(Length::Px(16.0).into()),
        padding_bottom: Some(Length::Px(16.0).into()),
        padding_left: Some(Length::Px(16.0).into()),
        padding_right: Some(Length::Px(16.0).into()),
        ..Default::default()
    }
}

fn primary_sidebar() -> StyleApplication {
    StyleApplication::new(base()).with_computed("nnd_primary_sidebar", || StyleRules {
        background: Some(Color("#e6e9f5".into()).into()),
        ..sidebar_common()
    })
}

fn secondary_sidebar() -> StyleApplication {
    StyleApplication::new(base()).with_computed("nnd_secondary_sidebar", || StyleRules {
        background: Some(Color("#f5e9e6".into()).into()),
        ..sidebar_common()
    })
}

fn sidebar_title() -> StyleApplication {
    StyleApplication::new(base()).with_computed("nnd_sidebar_title", || StyleRules {
        font_size: Some(Length::Px(13.0).into()),
        padding_bottom: Some(Length::Px(4.0).into()),
        ..Default::default()
    })
}

fn page_style() -> StyleApplication {
    StyleApplication::new(base()).with_computed("nnd_page", || StyleRules {
        flex_direction: Some(FlexDirection::Column),
        flex_grow: Some(1.0.into()),
        gap: Some(Length::Px(12.0).into()),
        padding_top: Some(Length::Px(32.0).into()),
        padding_bottom: Some(Length::Px(32.0).into()),
        padding_left: Some(Length::Px(32.0).into()),
        padding_right: Some(Length::Px(32.0).into()),
        background: Some(Color("#ffffff".into()).into()),
        ..Default::default()
    })
}

fn page_title() -> StyleApplication {
    StyleApplication::new(base()).with_computed("nnd_page_title", || StyleRules {
        font_size: Some(Length::Px(28.0).into()),
        ..Default::default()
    })
}

fn page_body() -> StyleApplication {
    StyleApplication::new(base()).with_computed("nnd_page_body", || StyleRules {
        font_size: Some(Length::Px(15.0).into()),
        ..Default::default()
    })
}
