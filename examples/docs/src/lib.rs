//! Idealyst documentation site.
//!
//! Single, platform-agnostic crate: [`app`] returns a `Primitive`
//! tree that runs unchanged on every backend the framework supports.
//! The per-platform glue (wasm-bindgen entry for web, etc.) is the
//! responsibility of the `idealyst` CLI, which materializes those
//! wrappers into `target/idealyst/<platform>/` at build time.
//!
//! The `web` module's `wasm-bindgen` `start()` is transitional — it
//! lives here until `idealyst dev` / `idealyst build web` generates
//! that wrapper crate.
//!
//! # Module ordering
//!
//! `mod shell;` is declared with `#[macro_use]` and BEFORE `mod
//! pages;`. The `#[component]` attribute on `PageHeader` / `Section`
//! / `CodeBlock` / `SectionWithCode` generates local `macro_rules!`;
//! `#[macro_use]` lifts those to crate-root scope so page modules
//! can invoke them via the `ui!` DSL.

use framework_core::{
    component, signal, ui, DrawerHandle, DrawerNavigator, HeaderStyle, Primitive, Ref, Screen,
    Signal,
};
use idea_ui::{idea_header, install_idea_theme, light_theme, IdeaTheme};

mod routes;
mod styles;

// Order matters: `#[macro_use] mod shell;` must precede `mod pages;`
// so the `#[component]`-generated invocation macros are visible at
// the crate root and therefore inside every page module.
#[macro_use]
mod shell;
mod pages;

#[cfg(target_arch = "wasm32")]
mod web;

use routes::{
    CLI_ROUTE, COMPONENTS_ROUTE, MACROS_ROUTE, NAVIGATION_ROUTE, OVERVIEW_ROUTE, PLATFORMS_ROUTE,
    PRIMITIVES_ROUTE, QUICKSTART_ROUTE, REACTIVITY_ROUTE, STYLES_ROUTE, UI_DSL_ROUTE,
};
use shell::content_builder;
#[cfg(target_arch = "wasm32")]
use shell::web_layout;

#[component]
pub fn app() -> Primitive {
    install_idea_theme(light_theme());

    // Theme flag the sidebar's dark-mode toggle drives. Owned at
    // the root so it survives across screen pushes.
    let is_dark: Signal<bool> = signal!(false);
    let drawer: Ref<DrawerHandle> = Ref::new();

    // Header + body background configured navigator-wide via a
    // single `.header(idea_header(...))` call. The closure receives
    // the current `IdeaTheme` and returns a `HeaderStyle` bundle;
    // the drawer's per-drawer Effect re-runs it on every theme
    // swap, so flipping the sidebar's dark-mode toggle re-tints the
    // bar and the body surface without per-screen wiring.
    // Per-screen `Screen::new(...).header_background(...)` still
    // overrides any of these defaults when set.
    let builder = DrawerNavigator::new(&OVERVIEW_ROUTE)
        .header(idea_header(|t| HeaderStyle {
            background: Some(t.colors().surface.value().clone()),
            title: Some(t.colors().text.value().clone()),
            tint: Some(t.colors().text.value().clone()),
            body_background: Some(t.colors().background.value().clone()),
        }))
        .screen(OVERVIEW_ROUTE, |_| Screen::new(pages::overview::page()).title("Overview"))
        .screen(QUICKSTART_ROUTE, |_| Screen::new(pages::quickstart::page()).title("Quickstart"))
        .screen(COMPONENTS_ROUTE, |_| Screen::new(pages::components::page()).title("Components"))
        .screen(REACTIVITY_ROUTE, |_| Screen::new(pages::reactivity::page()).title("Reactivity"))
        .screen(UI_DSL_ROUTE, |_| Screen::new(pages::ui_dsl::page()).title("UI DSL"))
        .screen(PRIMITIVES_ROUTE, |_| Screen::new(pages::primitives::page()).title("Primitives"))
        .screen(STYLES_ROUTE, |_| Screen::new(pages::styles::page()).title("Styles & Themes"))
        .screen(NAVIGATION_ROUTE, |_| Screen::new(pages::navigation::page()).title("Navigation"))
        .screen(MACROS_ROUTE, |_| Screen::new(pages::macros_page::page()).title("Macros"))
        .screen(CLI_ROUTE, |_| Screen::new(pages::cli::page()).title("CLI"))
        .screen(PLATFORMS_ROUTE, |_| Screen::new(pages::platforms::page()).title("Platforms"))
        .content(content_builder(is_dark))
        .bind(drawer);

    // Web is the exception: native backends render their own drawer
    // chrome (UINavigationBar / ActionBar + drawer panel) from the
    // screen-level header config above. On web, we place the
    // pre-built drawer-content beside the outlet ourselves.
    #[cfg(target_arch = "wasm32")]
    let builder = builder.layout(web_layout());

    ui! {
        builder
    }
}
