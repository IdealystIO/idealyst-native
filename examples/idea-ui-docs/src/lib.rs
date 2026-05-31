//! idea-ui-docs — the self-referencing documentation app for idea-ui.
//!
//! Persistent drawer sidebar lists every page (grouped by section);
//! the navigator swaps only the page body. Every page documents the
//! component library *with the component library* — control panels,
//! props tables, callouts, and the sidebar itself are all idea-ui +
//! local `#[component]` wrappers around it.
//!
//! ## Crate layout
//!
//! - `lib.rs` (this) — `app()` entry: theme install + DrawerNavigator wiring.
//! - `routes.rs` — Route constants + `SECTIONS` index the sidebar walks.
//! - `shell.rs` — Sidebar + page-template components (`ComponentPage`,
//!   `Demo`, `PropsTable`, `CodePanel`, `Callout`, …).
//! - `styles.rs` — Local stylesheets for chrome only.
//! - `pages/*.rs` — One file per category; each module exports one
//!   `page()` per documented surface.

use runtime_core::{component, signal, ui, Element, Ref, Signal, Route};
use runtime_core::primitives::navigator::Screen;
use drawer_navigator::{
    install_navigator_pin_width, DrawerBuilder, DrawerHandle, DrawerNavigator, DrawerScreenExt,
    HeaderStyle,
};
use idea_ui::{idea_color, install_idea_theme, light_theme};

/// Wrap a page's `Element` in a `Screen` whose title is the
/// human-readable `IndexEntry::label` for the route. Drives both the
/// iOS `UINavigationItem.title` and the Android `Toolbar` title via
/// the per-screen `header_options.title` field — without this the iOS
/// fallback shows `route.name()` (`"overview"`, `"tokens"`, …) and
/// the Android Toolbar renders no title text at all.
fn titled(route: &'static Route<()>, el: Element) -> Screen {
    let label = label_for_route(route.name()).unwrap_or_else(|| route.name());
    Screen::new(el).title(label)
}

/// Look up the human-readable sidebar label for a route. Walks
/// `routes::SECTIONS` (the single source of truth for both the
/// sidebar list and the navigator wiring). Returns `None` for routes
/// that aren't in the sidebar — callers fall back to the route name.
fn label_for_route(route_name: &'static str) -> Option<&'static str> {
    for section in routes::SECTIONS {
        for entry in section.entries {
            if entry.route.name() == route_name {
                return Some(entry.label);
            }
        }
    }
    None
}

mod pages;
mod routes;
mod shell;
mod styles;

// =============================================================================
// Per-target SDK-handler registration. Called by the CLI-generated
// wrapper before mount.
// =============================================================================

#[cfg(target_arch = "wasm32")]
pub fn register_extensions(backend: &mut backend_web::WebBackend) {
    drawer_navigator::register(backend);
    idea_codeblock::register(backend);
    table::register(backend);
    backend_web::install_viewport_observer();
}

// On native, `idea_codeblock::register` and `table::register` are
// generic no-ops over any `Backend` — the SDKs build their node trees
// directly from view + text primitives instead of via `Element::External`,
// so there's nothing per-backend to install. The calls are kept for
// symmetry with the web path; if either SDK ever grows native-specific
// setup, this is where it lands.

#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub fn register_extensions(backend: &mut backend_ios::IosBackend) {
    drawer_navigator::register(backend);
    idea_codeblock::register(backend);
    table::register(backend);
}

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
pub fn register_extensions(backend: &mut backend_android::AndroidBackend) {
    drawer_navigator::register(backend);
    idea_codeblock::register(backend);
    table::register(backend);
}

#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
pub fn register_extensions(backend: &mut backend_macos::MacosBackend) {
    drawer_navigator::register(backend);
    idea_codeblock::register(backend);
    table::register(backend);
}

#[cfg(all(
    not(target_arch = "wasm32"),
    not(target_os = "ios"),
    not(target_os = "android"),
    not(target_os = "macos"),
))]
pub fn register_extensions(backend: &mut backend_terminal::TerminalBackend) {
    drawer_navigator::register(backend);
    idea_codeblock::register(backend);
    table::register(backend);
}

// Recorder-side registration for the runtime-server sidecar. A distinct
// fn name (not an overload of `register_extensions`) so it never
// collides with the host target's per-backend overload above when both
// compile in the sidecar build. Only the drawer navigator needs a
// recording handler: `idea_codeblock` / `table` build their trees from
// view+text primitives on non-web backends (no `Element::External`), so
// the recorder captures them as ordinary primitives — nothing to
// register. Gated by `sidecar` (set only by the generated sidecar
// wrapper) so device/web builds never pull `dev-server`.
#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(backend: &mut dev_server::WireRecordingBackend) {
    drawer_navigator::recording::register(backend);
}

use routes::{
    ALERT_ROUTE, AVATAR_ROUTE, BADGE_ROUTE, BUTTON_ROUTE, CARD_ROUTE, CENTER_ROUTE,
    COLLAPSIBLE_ROUTE, COMBOS_ROUTE, CONTROLS_ROUTE, CUSTOM_THEME_ROUTE, DATA_ROUTE, DIVIDER_ROUTE,
    DRAWER_ROUTE, EXT_BUILD_COMPONENT_ROUTE, EXT_CUSTOM_TONE_ROUTE, EXT_CUSTOM_VARIANT_ROUTE,
    EXT_DOC_CONTROLS_ROUTE, FIELD_ROUTE, HELLO_ROUTE, ICON_BUTTON_ROUTE, INSTALL_ROUTE,
    INTENTS_ROUTE, LIGHT_DARK_ROUTE, MENUS_ROUTE, MODAL_ROUTE, MODIFIERS_ROUTE, OVERVIEW_ROUTE,
    POPOVER_ROUTE, SELECT_ROUTE, SKELETON_ROUTE, SPACER_ROUTE, SPINNER_ROUTE, STACK_ROUTE,
    SWITCH_ROUTE, TABLE_ROUTE, TABS_ROUTE, TAG_ROUTE, TOKENS_ROUTE, TYPOGRAPHY_ROUTE,
};

#[component]
pub fn app() -> Element {
    install_idea_theme(light_theme());

    let nav: Ref<DrawerHandle> = Ref::new();
    // App-level dark-mode state survives navigation: theme flag for
    // the sidebar's dark-mode toggle, owned at the top so each
    // pushed screen's per-screen scope can drop without losing it.
    let is_dark: Signal<bool> = signal!(false);

    // Pin the sidebar (vs. modal slide-in) at wide viewports.
    install_navigator_pin_width(900.0);

    let builder = DrawerNavigator::new(&OVERVIEW_ROUTE)
        // Navigator-level header theming: closures re-resolve their
        // token reads every time the iOS slot-style Effect / Android
        // slot_styles dispatcher re-fires, which the framework's
        // reactive plumbing wires automatically on `set_idea_theme`
        // token swaps. Without this the UINavigationController bar /
        // Android Toolbar stay at the platform default that was
        // installed once at create_drawer time.
        .header(|| HeaderStyle {
            background: Some((idea_color(|c| c.surface.clone()))()),
            title: Some((idea_color(|c| c.text.clone()))()),
            tint: Some((idea_color(|c| c.text.clone()))()),
            body_background: Some((idea_color(|c| c.background.clone()))()),
        })
        // Getting Started
        .screen(OVERVIEW_ROUTE, move |_| titled(&OVERVIEW_ROUTE, pages::overview::page()))
        .screen(INSTALL_ROUTE, move |_| titled(&INSTALL_ROUTE, pages::install::page()))
        .screen(HELLO_ROUTE, move |_| titled(&HELLO_ROUTE, pages::hello::page()))
        // Theming
        .screen(TOKENS_ROUTE, move |_| titled(&TOKENS_ROUTE, pages::theming::tokens()))
        .screen(INTENTS_ROUTE, move |_| titled(&INTENTS_ROUTE, pages::theming::intents()))
        .screen(LIGHT_DARK_ROUTE, move |_| titled(&LIGHT_DARK_ROUTE, pages::theming::light_dark(is_dark)))
        .screen(CUSTOM_THEME_ROUTE, move |_| titled(&CUSTOM_THEME_ROUTE, pages::theming::custom_theme()))
        .screen(MODIFIERS_ROUTE, move |_| titled(&MODIFIERS_ROUTE, pages::theming::modifiers()))
        // Layout
        .screen(STACK_ROUTE, move |_| titled(&STACK_ROUTE, pages::layout::stack()))
        .screen(CARD_ROUTE, move |_| titled(&CARD_ROUTE, pages::layout::card()))
        .screen(TABLE_ROUTE, move |_| titled(&TABLE_ROUTE, pages::layout::table()))
        .screen(DIVIDER_ROUTE, move |_| titled(&DIVIDER_ROUTE, pages::layout::divider()))
        .screen(CENTER_ROUTE, move |_| titled(&CENTER_ROUTE, pages::layout::center()))
        .screen(SPACER_ROUTE, move |_| titled(&SPACER_ROUTE, pages::layout::spacer()))
        // Typography
        .screen(TYPOGRAPHY_ROUTE, move |_| titled(&TYPOGRAPHY_ROUTE, pages::typography::page()))
        // Actions
        .screen(BUTTON_ROUTE, move |_| titled(&BUTTON_ROUTE, pages::actions::button()))
        .screen(ICON_BUTTON_ROUTE, move |_| titled(&ICON_BUTTON_ROUTE, pages::actions::icon_button()))
        .screen(BADGE_ROUTE, move |_| titled(&BADGE_ROUTE, pages::actions::badge()))
        .screen(TAG_ROUTE, move |_| titled(&TAG_ROUTE, pages::actions::tag()))
        // Inputs
        .screen(FIELD_ROUTE, move |_| titled(&FIELD_ROUTE, pages::inputs::field()))
        .screen(SWITCH_ROUTE, move |_| titled(&SWITCH_ROUTE, pages::inputs::switch()))
        .screen(SELECT_ROUTE, move |_| titled(&SELECT_ROUTE, pages::inputs::select()))
        // Feedback
        .screen(ALERT_ROUTE, move |_| titled(&ALERT_ROUTE, pages::feedback::alert()))
        .screen(SPINNER_ROUTE, move |_| titled(&SPINNER_ROUTE, pages::feedback::spinner()))
        .screen(SKELETON_ROUTE, move |_| titled(&SKELETON_ROUTE, pages::feedback::skeleton()))
        .screen(AVATAR_ROUTE, move |_| titled(&AVATAR_ROUTE, pages::feedback::avatar()))
        // Overlays
        .screen(MODAL_ROUTE, move |_| titled(&MODAL_ROUTE, pages::overlays::modal()))
        .screen(POPOVER_ROUTE, move |_| titled(&POPOVER_ROUTE, pages::overlays::popover()))
        .screen(DRAWER_ROUTE, move |_| titled(&DRAWER_ROUTE, pages::overlays::drawer()))
        // Stateful
        .screen(TABS_ROUTE, move |_| titled(&TABS_ROUTE, pages::stateful::tabs()))
        .screen(COLLAPSIBLE_ROUTE, move |_| titled(&COLLAPSIBLE_ROUTE, pages::stateful::collapsible()))
        // New components & patterns
        .screen(CONTROLS_ROUTE, move |_| titled(&CONTROLS_ROUTE, pages::controls::controls()))
        .screen(DATA_ROUTE, move |_| titled(&DATA_ROUTE, pages::controls::data()))
        .screen(MENUS_ROUTE, move |_| titled(&MENUS_ROUTE, pages::patterns::menus()))
        .screen(COMBOS_ROUTE, move |_| titled(&COMBOS_ROUTE, pages::patterns::combos()))
        // Extending
        .screen(EXT_CUSTOM_TONE_ROUTE, move |_| titled(&EXT_CUSTOM_TONE_ROUTE, pages::extending::custom_tone()))
        .screen(EXT_CUSTOM_VARIANT_ROUTE, move |_| titled(&EXT_CUSTOM_VARIANT_ROUTE, pages::extending::custom_variant()))
        .screen(EXT_BUILD_COMPONENT_ROUTE, move |_| titled(&EXT_BUILD_COMPONENT_ROUTE, pages::extending::build_component()))
        .screen(EXT_DOC_CONTROLS_ROUTE, move |_| titled(&EXT_DOC_CONTROLS_ROUTE, pages::extending::doc_controls()))
        .drawer_width(280.0)
        .leading_with(move |slot| shell::sidebar(slot, is_dark));

    ui! { builder.bind(nav) }
}
