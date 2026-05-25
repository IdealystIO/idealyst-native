//! idea-ui-docs — the self-referencing documentation app.
//!
//! Sidebar nav on the left, navigator-routed pages on the right.
//! Every page demos a component category; many of those demos use
//! the `DocControls` derive to auto-generate a live "twiddle the
//! props" panel built from idea-ui components itself.
//!
//! ## Crate layout
//!
//! - `lib.rs` (this) — entry point: theme install + Navigator wiring.
//! - `routes.rs` — Route constants + INDEX list the sidebar walks.
//! - `shell.rs` — Persistent layout chrome (sidebar + outlet).
//! - `styles.rs` — Local stylesheets for chrome only (not idea-ui).
//! - `pages/*.rs` — One file per category page.

use runtime_core::{component, signal, ui, Primitive, Ref, Signal};
use idea_ui::{install_idea_theme, light_theme};
use stack_navigator::{Navigator, StackBuilder, StackHandle};

mod pages;
mod routes;
mod shell;
mod styles;
#[cfg(target_arch = "wasm32")]
mod web;

use routes::{
    ACTIONS_ROUTE, FEEDBACK_ROUTE, INPUTS_ROUTE, LAYOUT_ROUTE, OVERLAYS_ROUTE, OVERVIEW_ROUTE,
    STATEFUL_ROUTE, THEMES_ROUTE, TYPOGRAPHY_ROUTE,
};
#[cfg(target_arch = "wasm32")]
use shell::web_layout;

#[component]
pub fn app() -> Primitive {
    install_idea_theme(light_theme());

    // App-level state survives navigation: theme flag for the
    // sidebar's dark-mode toggle. Owned at the top so each pushed
    // screen's per-screen scope can drop without losing it.
    let is_dark: Signal<bool> = signal!(false);
    let nav: Ref<StackHandle> = Ref::new();

    // Sidebar chrome is web-only: on mobile/desktop-native, navigation
    // is a stack with platform-native back gestures, not a persistent
    // side nav. The sidebar layout would clash with native nav idioms,
    // so we cfg-gate it to wasm and let native builds render pages
    // raw inside Navigator's default stack.
    let builder = Navigator::new(&OVERVIEW_ROUTE)
        .screen(OVERVIEW_ROUTE, move |_| pages::overview::page())
        .screen(THEMES_ROUTE, move |_| pages::themes::page(is_dark))
        .screen(LAYOUT_ROUTE, move |_| pages::layout::page())
        .screen(TYPOGRAPHY_ROUTE, move |_| pages::typography::page())
        .screen(ACTIONS_ROUTE, move |_| pages::actions::page())
        .screen(INPUTS_ROUTE, move |_| pages::inputs::page())
        .screen(FEEDBACK_ROUTE, move |_| pages::feedback::page())
        .screen(OVERLAYS_ROUTE, move |_| pages::overlays::page())
        .screen(STATEFUL_ROUTE, move |_| pages::stateful::page());

    #[cfg(target_arch = "wasm32")]
    let builder = builder.layout(web_layout(is_dark));

    ui! {
        { builder.bind(nav) }
    }
}
