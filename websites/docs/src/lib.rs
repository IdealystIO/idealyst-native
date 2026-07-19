//! Idealyst documentation site.
//!
//! Single, platform-agnostic crate: [`app`] returns a `Element`
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

use idea_ui_nav::AppShell;
use runtime_core::{component, effect, signal, ui, Breakpoint, Element, Ref, Screen, Signal};
use swap_navigator::{MountPolicy, SwapBuilder, SwapHandle, SwapNavigator};
use idea_ui::{install_idea_theme, light_theme};

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

// Navigators + externals self-register at backend construction via
// `inventory::submit!` inside their SDK crates — the app just uses them, no
// per-platform registration. The hook remains for app-local externals; the CLI
// bootstrap still calls it. See [[project_inventory_self_registration]].
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

// Recorder-side registration for the runtime-server sidecar. Distinct fn
// name (not an overload of `register_extensions`) so it never collides
// with the host target's per-backend overload when both compile in the
// sidecar build. Gated by `sidecar` (set only by the generated sidecar
// wrapper) so device/web builds never pull `dev-server`.
#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(backend: &mut dev_server::WireRecordingBackend) {
    swap_navigator::recording::register(backend);
}

use routes::{
    ANIMATION_ROUTE, ASYNC_REACTIVITY_ROUTE, BACKENDS_ROUTE, CLI_ROUTE, COMPONENTS_ROUTE,
    DEV_TOOLS_ROUTE, ICONS_ROUTE, INTRODUCTION_ROUTE, LISTS_ROUTE, MACROS_ROUTE,
    NAVIGATION_ROUTE, NET_ROUTE, OVERVIEW_ROUTE, PLATFORMS_ROUTE, PRIMITIVES_ROUTE,
    QUICKSTART_ROUTE, REACTIVITY_ROUTE, REFS_ROUTE, ROBOT_ROUTE, SERVER_FUNCTIONS_ROUTE,
    SIMULATOR_ROUTE, STYLES_ROUTE, BUILDING_A_THEME_SYSTEM_ROUTE, PORTAL_ROUTE,
    REACTIVE_TEXT_BINDINGS_ROUTE, THIRD_PARTY_PRIMITIVES_ROUTE, UI_DSL_ROUTE,
    WGPU_NATIVE_API_ROUTE, WRITING_A_BACKEND_ROUTE,
};
#[component]
pub fn app() -> Element {
    install_idea_theme(light_theme());

    // Theme flag the sidebar's dark-mode toggle drives. Owned at
    // the root so it survives across screen pushes.
    let is_dark: Signal<bool> = signal(false);
    // Drawer-open state for narrow viewports — author-owned now (the
    // AppShell scrim + the auto-close effect close it; the mobile
    // header's hamburger opens it). Pinned widths ignore it entirely.
    let drawer_open: Signal<bool> = signal(false);
    let nav: Ref<SwapHandle> = Ref::new();

    // Screen titles are gone from the builder: the shell's own mobile
    // header derives the label reactively from the navigator's
    // `active_route` (see `shell::mobile_header`). The sidebar pins at
    // the framework's default `Lg` breakpoint — the same width the
    // legacy drawer chrome used (its default pin width was
    // `breakpoints().lg_min`), so the collapse point is unchanged.
    let builder = SwapNavigator::new(&OVERVIEW_ROUTE)
        .screen(INTRODUCTION_ROUTE, |_| Screen::new(pages::introduction::page()))
        .screen(OVERVIEW_ROUTE, |_| Screen::new(pages::overview::page()))
        .screen(QUICKSTART_ROUTE, |_| Screen::new(pages::quickstart::page()))
        .screen(COMPONENTS_ROUTE, |_| Screen::new(pages::components::page()))
        .screen(REACTIVITY_ROUTE, |_| Screen::new(pages::reactivity::page()))
        .screen(ASYNC_REACTIVITY_ROUTE, |_| Screen::new(pages::async_reactivity::page()))
        .screen(SERVER_FUNCTIONS_ROUTE, |_| Screen::new(pages::server_functions::page()))
        .screen(UI_DSL_ROUTE, |_| Screen::new(pages::ui_dsl::page()))
        .screen(PRIMITIVES_ROUTE, |_| Screen::new(pages::primitives::page()))
        .screen(STYLES_ROUTE, |_| Screen::new(pages::styles::page()))
        .screen(ANIMATION_ROUTE, |_| Screen::new(pages::animation::page()))
        .screen(NAVIGATION_ROUTE, |_| Screen::new(pages::navigation::page()))
        .screen(LISTS_ROUTE, |_| Screen::new(pages::lists::page()))
        .screen(ICONS_ROUTE, |_| Screen::new(pages::icons::page()))
        .screen(REFS_ROUTE, |_| Screen::new(pages::refs::page()))
        .screen(PORTAL_ROUTE, |_| Screen::new(pages::portal::page()))
        .screen(NET_ROUTE, |_| Screen::new(pages::net::page()))
        .screen(ROBOT_ROUTE, |_| Screen::new(pages::robot::page()))
        .screen(DEV_TOOLS_ROUTE, |_| Screen::new(pages::dev_tools::page()))
        .screen(BACKENDS_ROUTE, |_| Screen::new(pages::backends::page()))
        .screen(WRITING_A_BACKEND_ROUTE, |_| Screen::new(pages::writing_a_backend::page()))
        .screen(THIRD_PARTY_PRIMITIVES_ROUTE, |_| {
            Screen::new(pages::third_party_primitives::page())
        })
        .screen(BUILDING_A_THEME_SYSTEM_ROUTE, |_| {
            Screen::new(pages::building_a_theme_system::page())
        })
        .screen(REACTIVE_TEXT_BINDINGS_ROUTE, |_| {
            Screen::new(pages::reactive_text_bindings::page())
        })
        .screen(WGPU_NATIVE_API_ROUTE, |_| Screen::new(pages::wgpu_native_api::page()))
        .screen(SIMULATOR_ROUTE, |_| Screen::new(pages::simulator_demo::page()))
        .screen(MACROS_ROUTE, |_| Screen::new(pages::macros_page::page()))
        .screen(CLI_ROUTE, |_| Screen::new(pages::cli::page()))
        .screen(PLATFORMS_ROUTE, |_| Screen::new(pages::platforms::page()))
        // Legacy web behavior: one screen resident at a time; switching
        // away disposes the screen's scope (which also tears down the
        // Simulator page's embedded wgpu host + render loop) and a
        // return rebuilds it fresh. Matches browser semantics and the
        // old drawer-on-web engine exactly.
        .mount_policy(MountPolicy::LazyDisposing)
        // The shell: AppShell packages pinned-sidebar ⇄ drawer around
        // the one-shot outlet; the mobile header (hamburger + reactive
        // title) collapses in below the pin width.
        .layout(move |nav_ctx| {
            // Auto-close the drawer when a sidebar link navigates while
            // unpinned (the legacy web drawer engine did this in its
            // Select arm; author-owned now). Reading `active_route`
            // inside the effect subscribes it to every navigation.
            let active_route = nav_ctx.active_route;
            effect!({
                let _ = active_route.get();
                if !idea_ui_nav::sidebar_pinned(Breakpoint::Lg) {
                    drawer_open.set(false);
                }
            });

            let sidebar_el = shell::sidebar(active_route, is_dark);
            let header = shell::mobile_header(active_route, drawer_open);
            let body: Element = ui! {
                view(style = shell::outlet_grow_style) {
                    { nav_ctx.outlet }
                }
            };
            let content: Element = ui! {
                view(style = shell::shell_column_style) {
                    header
                    body
                }
            };
            ui! {
                // Width matches the docs `Sidebar` sheet (260) so the
                // panel and its content agree exactly.
                AppShell(
                    sidebar = vec![sidebar_el],
                    is_open = drawer_open,
                    pin_at = Breakpoint::Lg,
                    width = 260.0,
                ) {
                    { content }
                }
            }
        })
        .bind(nav);

    ui! { builder }
}
