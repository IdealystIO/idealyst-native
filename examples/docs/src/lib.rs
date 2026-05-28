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

use runtime_core::{component, signal, ui, Primitive, Ref, Screen, Signal};
use drawer_navigator::{
    DrawerBuilder, DrawerHandle, DrawerNavigator, DrawerScreenExt, HeaderStyle,
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

// Web SDK-handler registration called by the CLI-generated wrapper
// before mount. Used here to register the drawer navigator SDK; any
// other per-backend extensions (external primitives, custom assets)
// would also hook in here.
#[cfg(target_arch = "wasm32")]
pub fn register_extensions(backend: &mut backend_web::WebBackend) {
    drawer_navigator::register(backend);
}

// iOS equivalent — same name + role, different backend type. The
// iOS wrapper template invokes this before `mount`. Without it, the
// drawer navigator's factory isn't in `IosBackend::navigator_handlers`
// and the first `Backend::create_navigator` call panics —
// abort-trapped because it bubbles up through `ios_main`'s C-ABI.
#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub fn register_extensions(backend: &mut backend_ios::IosBackend) {
    drawer_navigator::register(backend);
}

// Android equivalent — invoked from the generated Android wrapper
// before `mount`. Same registration contract: without it, the first
// `Backend::create_navigator` call panics because the drawer's
// factory isn't installed in `AndroidBackend::navigator_handlers`.
#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
pub fn register_extensions(backend: &mut backend_android::AndroidBackend) {
    drawer_navigator::register(backend);
}

// macOS desktop (AppKit) equivalent — invoked from the macOS host
// wrapper before mount. `drawer_navigator::register` resolves to its
// macOS impl (`&mut MacosBackend`) on this host, so the signature must
// match or the drawer factory never lands in the backend's handlers.
#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
pub fn register_extensions(backend: &mut backend_macos::MacosBackend) {
    drawer_navigator::register(backend);
}

// Terminal (TTY) equivalent — invoked from the CLI-generated terminal
// wrapper before mount. Without it the first `create_navigator` call
// panics because the drawer factory isn't in
// `TerminalBackend::navigator_handlers`.
#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android", target_os = "macos")))]
pub fn register_extensions(backend: &mut backend_terminal::TerminalBackend) {
    drawer_navigator::register(backend);
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
use shell::content_builder;

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
        .screen(ASYNC_REACTIVITY_ROUTE, |_| {
            Screen::new(pages::async_reactivity::page()).title("Async Reactivity")
        })
        .screen(SERVER_FUNCTIONS_ROUTE, |_| {
            Screen::new(pages::server_functions::page()).title("Server Functions")
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
        .screen(NET_ROUTE, |_| Screen::new(pages::net::page()).title("Net"))
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
        .sidebar_with(content_builder(is_dark))
        .bind(drawer);

    ui! { builder }
}
