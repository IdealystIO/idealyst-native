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

use runtime_core::{
    component, signal, ui, DrawerHandle, DrawerNavigator, HeaderStyle, Primitive, Ref, Screen,
    Signal,
};
use idea_ui::{idea_header, install_idea_theme, light_theme, IdeaTheme};

pub mod meta;
pub mod registry;
mod routes;
mod styles;

// Order matters: `#[macro_use]` lifts the `#[component]`-generated
// invocation macros (from `shell` and `components`) to crate-root
// scope, where the `ui!` DSL inside every page module can find them.
#[macro_use]
mod shell;
#[macro_use]
mod components;
mod pages;

// Declared AFTER the `#[macro_use]` mods above so it can see the
// shell + idea-ui invocation macros that the `docs!` emission
// references.
#[cfg(test)]
mod macro_test;

#[cfg(target_arch = "wasm32")]
mod web;

use routes::{
    ANIMATION_ROUTE, BACKENDS_ROUTE, CLI_ROUTE, COMPONENTS_ROUTE, DEV_TOOLS_ROUTE, ICONS_ROUTE,
    INTRODUCTION_ROUTE, LISTS_ROUTE, MACROS_ROUTE, NAVIGATION_ROUTE, OVERVIEW_ROUTE,
    PLATFORMS_ROUTE, PRIMITIVES_ROUTE, QUICKSTART_ROUTE, REACTIVITY_ROUTE, REFS_ROUTE,
    ROBOT_ROUTE, SIMULATOR_ROUTE, STYLES_ROUTE, BUILDING_A_THEME_SYSTEM_ROUTE, PORTAL_ROUTE,
    REACTIVE_TEXT_BINDINGS_ROUTE, THIRD_PARTY_PRIMITIVES_ROUTE, UI_DSL_ROUTE,
    WGPU_NATIVE_API_ROUTE, WRITING_A_BACKEND_ROUTE,
};
use shell::{content_builder, web_layout};

#[component]
pub fn app() -> Primitive {
    install_idea_theme(light_theme());

    // Theme flag the sidebar's dark-mode toggle drives. Owned at
    // the root so it survives across screen pushes.
    let is_dark: Signal<bool> = signal!(false);
    let drawer: Ref<DrawerHandle> = Ref::new();

    // Builder-pattern form so the typed `Bound<DrawerHandle>` flows
    // through and we can call `.layout(web_layout())` without losing
    // the type after `IntoPrimitive` coercion. The layout closure
    // applies on both the local-render path (wasm in-browser) and
    // the runtime-server-replay path (recording backend serializes the layout
    // subtree + ships `AttachNavigatorLayout` over the wire).
    let builder = DrawerNavigator::new(&OVERVIEW_ROUTE)
        .header(idea_header(|t| HeaderStyle {
            background: Some(t.colors().surface.value().clone()),
            title: Some(t.colors().text.value().clone()),
            tint: Some(t.colors().text.value().clone()),
            body_background: Some(t.colors().background.value().clone()),
        }))
        .screen(INTRODUCTION_ROUTE, |_| {
            Screen::new(pages::introduction::page()).title("Introduction")
        })
        .screen(OVERVIEW_ROUTE, |_| Screen::new(pages::overview::page()).title("Overview"))
        .screen(QUICKSTART_ROUTE, |_| {
            Screen::new(pages::quickstart::page()).title("Getting Started")
        })
        .screen(COMPONENTS_ROUTE, |_| {
            Screen::new(pages::components::page()).title("Components")
        })
        .screen(REACTIVITY_ROUTE, |_| {
            Screen::new(pages::reactivity::page()).title("Reactivity")
        })
        .screen(UI_DSL_ROUTE, |_| Screen::new(pages::ui_dsl::page()).title("UI DSL"))
        .screen(PRIMITIVES_ROUTE, |_| {
            Screen::new(pages::primitives::page()).title("Primitives")
        })
        .screen(STYLES_ROUTE, |_| Screen::new(pages::styles::page()).title("Styles & Themes"))
        .screen(ANIMATION_ROUTE, |_| {
            Screen::new(pages::animation::page()).title("Animation")
        })
        .screen(NAVIGATION_ROUTE, |_| {
            Screen::new(pages::navigation::page()).title("Navigation")
        })
        .screen(LISTS_ROUTE, |_| Screen::new(pages::lists::page()).title("Lists"))
        .screen(ICONS_ROUTE, |_| Screen::new(pages::icons::page()).title("Icons"))
        .screen(REFS_ROUTE, |_| Screen::new(pages::refs::page()).title("Refs"))
        .screen(PORTAL_ROUTE, |_| {
            Screen::new(pages::portal::page()).title("Portal & Overlays")
        })
        .screen(ROBOT_ROUTE, |_| Screen::new(pages::robot::page()).title("Robot"))
        .screen(DEV_TOOLS_ROUTE, |_| {
            Screen::new(pages::dev_tools::page()).title("Dev Tools")
        })
        .screen(BACKENDS_ROUTE, |_| Screen::new(pages::backends::page()).title("Backends"))
        .screen(WRITING_A_BACKEND_ROUTE, |_| {
            Screen::new(pages::writing_a_backend::page()).title("Writing a Backend")
        })
        .screen(THIRD_PARTY_PRIMITIVES_ROUTE, |_| {
            Screen::new(pages::third_party_primitives::page())
                .title("Third-party Primitives")
        })
        .screen(BUILDING_A_THEME_SYSTEM_ROUTE, |_| {
            Screen::new(pages::building_a_theme_system::page())
                .title("Building a Theme System")
        })
        .screen(REACTIVE_TEXT_BINDINGS_ROUTE, |_| {
            Screen::new(pages::reactive_text_bindings::page())
                .title("Reactive Text Bindings")
        })
        .screen(WGPU_NATIVE_API_ROUTE, |_| {
            Screen::new(pages::wgpu_native_api::page()).title("wgpu Native API")
        })
        .screen(SIMULATOR_ROUTE, |_| {
            Screen::new(pages::simulator_demo::page()).title("Simulator")
        })
        .screen(MACROS_ROUTE, |_| Screen::new(pages::macros_page::page()).title("Macros"))
        .screen(CLI_ROUTE, |_| Screen::new(pages::cli::page()).title("CLI"))
        .screen(PLATFORMS_ROUTE, |_| {
            Screen::new(pages::platforms::page()).title("Platforms")
        })
        .content(content_builder(is_dark))
        .bind(drawer);

    let builder = builder.layout(web_layout());

    ui! { builder }
}
