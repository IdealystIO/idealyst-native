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
    component, signal, ui, DrawerHandle, DrawerItem, DrawerNavigator, Primitive, Ref, Signal,
};
use idea_ui::{install_idea_theme, light_theme};

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
use shell::sidebar_builder;
#[cfg(target_arch = "wasm32")]
use shell::web_layout;

#[component]
pub fn app() -> Primitive {
    install_idea_theme(light_theme());

    // Theme flag the sidebar's dark-mode toggle drives. Owned at
    // the root so it survives across screen pushes.
    let is_dark: Signal<bool> = signal!(false);
    let drawer: Ref<DrawerHandle> = Ref::new();

    // Each `.item(...)` registers a drawer/sidebar entry; each
    // `.screen(...)` wires the matching page renderer. Both lists
    // share the same routes (every item has a screen) but they are
    // declared separately so screens can be deep-linkable without
    // appearing in the drawer.
    let builder = DrawerNavigator::new(&OVERVIEW_ROUTE)
        .item(OVERVIEW_ROUTE, DrawerItem::new("Overview"))
        .item(QUICKSTART_ROUTE, DrawerItem::new("Quickstart"))
        .item(COMPONENTS_ROUTE, DrawerItem::new("Components"))
        .item(REACTIVITY_ROUTE, DrawerItem::new("Reactivity"))
        .item(UI_DSL_ROUTE, DrawerItem::new("UI DSL"))
        .item(PRIMITIVES_ROUTE, DrawerItem::new("Primitives"))
        .item(STYLES_ROUTE, DrawerItem::new("Styles & Themes"))
        .item(NAVIGATION_ROUTE, DrawerItem::new("Navigation"))
        .item(MACROS_ROUTE, DrawerItem::new("Macros"))
        .item(CLI_ROUTE, DrawerItem::new("CLI"))
        .item(PLATFORMS_ROUTE, DrawerItem::new("Platforms"))
        .screen(OVERVIEW_ROUTE, move |_| pages::overview::page())
        .screen(QUICKSTART_ROUTE, move |_| pages::quickstart::page())
        .screen(COMPONENTS_ROUTE, move |_| pages::components::page())
        .screen(REACTIVITY_ROUTE, move |_| pages::reactivity::page())
        .screen(UI_DSL_ROUTE, move |_| pages::ui_dsl::page())
        .screen(PRIMITIVES_ROUTE, move |_| pages::primitives::page())
        .screen(STYLES_ROUTE, move |_| pages::styles::page())
        .screen(NAVIGATION_ROUTE, move |_| pages::navigation::page())
        .screen(MACROS_ROUTE, move |_| pages::macros_page::page())
        .screen(CLI_ROUTE, move |_| pages::cli::page())
        .screen(PLATFORMS_ROUTE, move |_| pages::platforms::page())
        // Pin the drawer above 900px (sidebar mode on desktop web);
        // below that it slides in as an overlay.
        .pinned_above(900)
        .sidebar(sidebar_builder(is_dark))
        .bind(drawer);

    // Web layout places the pre-built sidebar beside the outlet;
    // native backends draw their own drawer chrome and ignore this.
    #[cfg(target_arch = "wasm32")]
    let builder = builder.layout(web_layout());

    ui! {
        builder
    }
}
