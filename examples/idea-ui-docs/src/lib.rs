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

use runtime_core::{component, signal, ui, Element, Ref, Signal};
use drawer_navigator::{install_navigator_pin_width, DrawerBuilder, DrawerHandle, DrawerNavigator};
use idea_ui::{install_idea_theme, light_theme};

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

use routes::{
    ALERT_ROUTE, AVATAR_ROUTE, BADGE_ROUTE, BUTTON_ROUTE, CARD_ROUTE, CENTER_ROUTE,
    COLLAPSIBLE_ROUTE, CUSTOM_THEME_ROUTE, DIVIDER_ROUTE, DRAWER_ROUTE, EXT_BUILD_COMPONENT_ROUTE,
    EXT_CUSTOM_TONE_ROUTE, EXT_CUSTOM_VARIANT_ROUTE, EXT_DOC_CONTROLS_ROUTE, FIELD_ROUTE,
    HELLO_ROUTE, ICON_BUTTON_ROUTE, INSTALL_ROUTE, INTENTS_ROUTE, LIGHT_DARK_ROUTE,
    MODAL_ROUTE, MODIFIERS_ROUTE, OVERVIEW_ROUTE, POPOVER_ROUTE, SELECT_ROUTE, SKELETON_ROUTE,
    SPACER_ROUTE, SPINNER_ROUTE, STACK_ROUTE, SWITCH_ROUTE, TABLE_ROUTE, TABS_ROUTE, TAG_ROUTE,
    TOKENS_ROUTE, TYPOGRAPHY_ROUTE,
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
        // Getting Started
        .screen(OVERVIEW_ROUTE, move |_| pages::overview::page())
        .screen(INSTALL_ROUTE, move |_| pages::install::page())
        .screen(HELLO_ROUTE, move |_| pages::hello::page())
        // Theming
        .screen(TOKENS_ROUTE, move |_| pages::theming::tokens())
        .screen(INTENTS_ROUTE, move |_| pages::theming::intents())
        .screen(LIGHT_DARK_ROUTE, move |_| pages::theming::light_dark(is_dark))
        .screen(CUSTOM_THEME_ROUTE, move |_| pages::theming::custom_theme())
        .screen(MODIFIERS_ROUTE, move |_| pages::theming::modifiers())
        // Layout
        .screen(STACK_ROUTE, move |_| pages::layout::stack())
        .screen(CARD_ROUTE, move |_| pages::layout::card())
        .screen(TABLE_ROUTE, move |_| pages::layout::table())
        .screen(DIVIDER_ROUTE, move |_| pages::layout::divider())
        .screen(CENTER_ROUTE, move |_| pages::layout::center())
        .screen(SPACER_ROUTE, move |_| pages::layout::spacer())
        // Typography
        .screen(TYPOGRAPHY_ROUTE, move |_| pages::typography::page())
        // Actions
        .screen(BUTTON_ROUTE, move |_| pages::actions::button())
        .screen(ICON_BUTTON_ROUTE, move |_| pages::actions::icon_button())
        .screen(BADGE_ROUTE, move |_| pages::actions::badge())
        .screen(TAG_ROUTE, move |_| pages::actions::tag())
        // Inputs
        .screen(FIELD_ROUTE, move |_| pages::inputs::field())
        .screen(SWITCH_ROUTE, move |_| pages::inputs::switch())
        .screen(SELECT_ROUTE, move |_| pages::inputs::select())
        // Feedback
        .screen(ALERT_ROUTE, move |_| pages::feedback::alert())
        .screen(SPINNER_ROUTE, move |_| pages::feedback::spinner())
        .screen(SKELETON_ROUTE, move |_| pages::feedback::skeleton())
        .screen(AVATAR_ROUTE, move |_| pages::feedback::avatar())
        // Overlays
        .screen(MODAL_ROUTE, move |_| pages::overlays::modal())
        .screen(POPOVER_ROUTE, move |_| pages::overlays::popover())
        .screen(DRAWER_ROUTE, move |_| pages::overlays::drawer())
        // Stateful
        .screen(TABS_ROUTE, move |_| pages::stateful::tabs())
        .screen(COLLAPSIBLE_ROUTE, move |_| pages::stateful::collapsible())
        // Extending
        .screen(EXT_CUSTOM_TONE_ROUTE, move |_| pages::extending::custom_tone())
        .screen(EXT_CUSTOM_VARIANT_ROUTE, move |_| pages::extending::custom_variant())
        .screen(EXT_BUILD_COMPONENT_ROUTE, move |_| pages::extending::build_component())
        .screen(EXT_DOC_CONTROLS_ROUTE, move |_| pages::extending::doc_controls())
        .drawer_width(280.0)
        .leading_with(move |slot| shell::sidebar(slot, is_dark));

    ui! { builder.bind(nav) }
}
