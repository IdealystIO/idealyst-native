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

// Web SDK-handler registration. Called by the auto-generated wrapper
// in `target/idealyst/<name>/web/wrapper/src/lib.rs` before `mount`,
// while it still has the bare backend. Anything that needs to install
// per-backend state (navigator handlers, external-primitive handlers,
// custom assets) goes here. Cross-platform tree itself lives in
// `app()` — this hook is purely for backend wiring.
#[cfg(target_arch = "wasm32")]
pub fn register_extensions(backend: &mut backend_web::WebBackend) {
    stack_navigator::register(backend);
}

use routes::{
    ACTIONS_ROUTE, FEEDBACK_ROUTE, INPUTS_ROUTE, LAYOUT_ROUTE, OVERLAYS_ROUTE, OVERVIEW_ROUTE,
    STATEFUL_ROUTE, THEMES_ROUTE, TYPOGRAPHY_ROUTE,
};
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

    // `.layout(...)` was dropped from stack-navigator's surface — on
    // web, screens render raw inside the navigator container without
    // author-supplied chrome. Future: use `drawer_navigator` if you
    // want a persistent web sidebar.
    let _ = is_dark;

    ui! {
        { builder.bind(nav) }
    }
}
