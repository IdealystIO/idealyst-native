//! `website` — the idealyst-native marketing site.
//!
//! Web is the primary target. The shell is a swap navigator (outlet
//! model) wrapped in an `idea_ui_nav::AppShell`: the sidebar builds
//! ONCE and survives every navigation; the navigator swaps only the
//! outlet's screen. Pinned-sidebar ⇄ off-canvas drawer is static
//! breakpoint styling (real `@media` on web + SSR, viewport-correct
//! first paint), and the sidebar reads the navigator's reactive
//! `active_route` for the active-link highlight.
//!
//! Every backend renders the SAME author chrome — no per-platform
//! navigator chrome (CLAUDE.md §7).

use idea_ui_nav::AppShell;
use runtime_core::{
    component, effect, signal, ui, Breakpoint, Color, Element, Ref, Route, Signal, Tokenized,
};
use runtime_core::primitives::navigator::Screen;
use swap_navigator::{MountPolicy, SwapBuilder, SwapHandle, SwapNavigator};
use idea_ui::{install_idea_theme, light_theme};

/// Wrap a page's `Element` in a `Screen` whose nav-bar title is the
/// sidebar label resolved via [`routes::label_for_route`] (which
/// covers both `SECTIONS` entries and the off-sidebar tangents like
/// `/targets` and `/comparisons/*`). Drives both
/// `UINavigationItem.title` on iOS and Android's `Toolbar` title;
/// without this the bar renders empty since iOS no longer falls
/// back to `route.name()`.
fn titled(route: &'static Route<()>, el: Element) -> Screen {
    // The label is drawn by the shell's own header (`shell::mobile_header`
    // derives it reactively from `active_route`), so the Screen carries no
    // navigator-chrome options anymore.
    let _ = route;
    Screen::new(el)
}

// `#[macro_use]` lifts the `#[component]`-generated invocation macros
// from `components::simulator` (and any future siblings) up to
// crate-root scope, where the `ui!` DSL inside every page module can
// resolve `Simulator(...)` to the local `simulator!` macro. Must
// come BEFORE `mod pages` so the page modules see the lifted macros.
#[macro_use]
mod components;
mod branding;
mod pages;
mod responsive;
mod routes;
mod shell;
mod styles;
mod typeface;

use routes::{
    AGENTIC_ROUTE, ARCHITECTURE_ROUTE, BACKENDS_ROUTE, CODE_SPLITTING_ROUTE, COMPARE_DIOXUS_ROUTE,
    COMPARE_ELECTRON_ROUTE, COMPARE_FLUTTER_ROUTE, COMPARE_REACT_ROUTE,
    COMPARE_WEB_FRAMEWORKS_ROUTE, COMPARE_WHEN_NOT_ROUTE, COMPARISONS_ROUTE, CONCEPTS_ROUTE,
    CROSS_PLATFORM_ROUTE, DEMO_ROUTE, FEATURES_ROUTE, FURTHER_READING_ROUTE, HOME_ROUTE,
    INSTALL_ROUTE, NAVIGATION_ROUTE, PERFORMANCE_ROUTE, PRIMITIVES_ROUTE, QUICKSTART_ROUTE,
    REACTIVITY_ROUTE, ROADMAP_ROUTE, SERVER_FUNCTIONS_ROUTE, SSR_ROUTE, STYLING_ROUTE,
    TARGETS_ROUTE, TYPE_SAFETY_ROUTE, WHY_RUST_ROUTE,
};

#[component]
pub fn app() -> Element {
    // Override idea-ui's default type scale for the website. The
    // marketing pages want bigger section headings than idea-ui's
    // defaults (which are tuned for the dense docs app). Same
    // theme trait, same downstream token names \u{2014} we just
    // mutate the values before `install_idea_theme` registers
    // them.
    let mut theme = light_theme();
    theme.typography.h1_size = 40.0;
    theme.typography.h2_size = 34.0;
    theme.typography.h3_size = 22.0;
    theme.typography.body_lg_size = 19.0;
    install_idea_theme(theme);

    let nav: Ref<SwapHandle> = Ref::new();
    // Drawer-open state for narrow viewports — author-owned now (the
    // AppShell scrim + the sidebar links close it; the hamburger opens
    // it). Pinned widths ignore it entirely.
    let drawer_open: Signal<bool> = signal(false);
    // App-level theme-toggle state — lives here (not inside a
    // per-screen scope) so flipping dark mode persists across
    // navigation. Captured by the sidebar builder closure below.
    let is_dark: Signal<bool> = signal(false);

    // Keep the document `<body>` background in sync with the active
    // theme's `color-background` token. The framework owns `#app`
    // and below \u{2014} the body element sits outside that subtree
    // and would otherwise stay at whatever index.html stamped it,
    // showing through on overscroll (mac touchpad bounce, mobile
    // rubber-banding) and any layout gap.
    //
    // The `Tokenized::token(...).resolve()` read inside the effect
    // subscribes the surrounding reactive scope to the token's
    // signal, so swapping themes re-fires this effect and the body
    // bg follows.
    sync_body_background_to_theme();

    // Align the framework's `Lg` breakpoint with the site's collapse
    // point so `AppShell(pin_at = Lg)`, the mobile-header collapse
    // (`collapse_responsive_style`), and the sidebar all flip at the
    // SAME width. The AppShell's pin/drawer split is static breakpoint
    // styling (`@media` on web + SSR), so the collapse is correct on the
    // static first paint. First-install wins — must run before any
    // breakpoint-keyed sheet resolves, on EVERY target (SSR included).
    let _ = runtime_core::install_breakpoints(runtime_core::Breakpoints {
        lg_min: responsive::SIDEBAR_COLLAPSE_PX as f32,
        ..Default::default()
    });

    // Site-only responsive chrome: narrow-screen `<pre>` wrapping (the
    // sidebar collapse + scrim are AppShell-owned now). No-op off web.
    responsive::install_responsive_css();

    let builder = SwapNavigator::new(&HOME_ROUTE)
        // Home embeds a wgpu-backed `Simulator`; the navigator-wide
        // `LazyDisposing` mount policy below tears its scope (and the
        // render loop) down on navigate-away, exactly as the legacy
        // per-screen policy did.
        .screen(HOME_ROUTE, move |_| titled(&HOME_ROUTE, pages::home::page()))
        .screen(FEATURES_ROUTE, move |_| titled(&FEATURES_ROUTE, pages::features::page()))
        .screen(CROSS_PLATFORM_ROUTE, move |_| titled(&CROSS_PLATFORM_ROUTE, pages::cross_platform::page()))
        .screen(PERFORMANCE_ROUTE, move |_| titled(&PERFORMANCE_ROUTE, pages::performance::page()))
        .screen(TYPE_SAFETY_ROUTE, move |_| titled(&TYPE_SAFETY_ROUTE, pages::type_safety::page()))
        .screen(SSR_ROUTE, move |_| titled(&SSR_ROUTE, pages::ssr::page()))
        .screen(INSTALL_ROUTE, move |_| titled(&INSTALL_ROUTE, pages::install::page()))
        .screen(QUICKSTART_ROUTE, move |_| titled(&QUICKSTART_ROUTE, pages::quickstart::page()))
        .screen(CONCEPTS_ROUTE, move |_| titled(&CONCEPTS_ROUTE, pages::concepts::page()))
        .screen(REACTIVITY_ROUTE, move |_| titled(&REACTIVITY_ROUTE, pages::reactivity::page()))
        .screen(STYLING_ROUTE, move |_| titled(&STYLING_ROUTE, pages::styling::page()))
        .screen(NAVIGATION_ROUTE, move |_| titled(&NAVIGATION_ROUTE, pages::navigation::page()))
        .screen(WHY_RUST_ROUTE, move |_| titled(&WHY_RUST_ROUTE, pages::why_rust::page()))
        .screen(DEMO_ROUTE, move |_| titled(&DEMO_ROUTE, pages::demo::page()))
        .screen(PRIMITIVES_ROUTE, move |_| titled(&PRIMITIVES_ROUTE, pages::primitives::page()))
        .screen(ARCHITECTURE_ROUTE, move |_| titled(&ARCHITECTURE_ROUTE, pages::architecture::page()))
        .screen(BACKENDS_ROUTE, move |_| titled(&BACKENDS_ROUTE, pages::backends::page()))
        .screen(SERVER_FUNCTIONS_ROUTE, move |_| titled(&SERVER_FUNCTIONS_ROUTE, pages::server_functions::page()))
        .screen(CODE_SPLITTING_ROUTE, move |_| titled(&CODE_SPLITTING_ROUTE, pages::code_splitting::page()))
        .screen(AGENTIC_ROUTE, move |_| titled(&AGENTIC_ROUTE, pages::agentic::page()))
        .screen(ROADMAP_ROUTE, move |_| titled(&ROADMAP_ROUTE, pages::roadmap::page()))
        .screen(FURTHER_READING_ROUTE, move |_| titled(&FURTHER_READING_ROUTE, pages::further_reading::page()))
        .screen(TARGETS_ROUTE, move |_| titled(&TARGETS_ROUTE, pages::targets::page()))
        .screen(COMPARISONS_ROUTE, move |_| titled(&COMPARISONS_ROUTE, pages::comparisons::page()))
        .screen(COMPARE_ELECTRON_ROUTE, move |_| titled(&COMPARE_ELECTRON_ROUTE, pages::comparisons::electron::page()))
        .screen(COMPARE_REACT_ROUTE, move |_| titled(&COMPARE_REACT_ROUTE, pages::comparisons::react::page()))
        .screen(COMPARE_DIOXUS_ROUTE, move |_| titled(&COMPARE_DIOXUS_ROUTE, pages::comparisons::dioxus::page()))
        .screen(COMPARE_FLUTTER_ROUTE, move |_| titled(&COMPARE_FLUTTER_ROUTE, pages::comparisons::flutter::page()))
        .screen(COMPARE_WEB_FRAMEWORKS_ROUTE, move |_| titled(&COMPARE_WEB_FRAMEWORKS_ROUTE, pages::comparisons::web_frameworks::page()))
        .screen(COMPARE_WHEN_NOT_ROUTE, move |_| titled(&COMPARE_WHEN_NOT_ROUTE, pages::comparisons::when_not::page()))
        // Legacy web behavior: one screen resident at a time; switching
        // away disposes the screen's scope (which is also what tears
        // down Home's embedded wgpu Simulator + its render loop) and a
        // return rebuilds it fresh. Matches browser semantics and the
        // old drawer-on-web engine exactly.
        .mount_policy(MountPolicy::LazyDisposing)
        // The shell: AppShell packages pinned-sidebar ⇄ drawer around
        // the one-shot outlet; the mobile header (hamburger + reactive
        // title) collapses in at narrow widths.
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
                AppShell(
                    sidebar = vec![sidebar_el],
                    is_open = drawer_open,
                    pin_at = Breakpoint::Lg,
                    width = 260.0,
                ) {
                    { content }
                }
            }
        });

    ui! { builder.bind(nav) }
}

/// On web, mirror the active theme's `color-background` onto
/// `document.body.style.background` reactively. On native targets,
/// no `<body>` exists \u{2014} the framework's root view fills the
/// platform window, so this is a no-op.
fn sync_body_background_to_theme() {
    #[cfg(target_arch = "wasm32")]
    {
        effect!({
            let bg: Color = Tokenized::<Color>::token(
                "color-background",
                Color("#ffffff".into()),
            )
            .resolve();
            if let Some(window) = web_sys::window() {
                if let Some(doc) = window.document() {
                    if let Some(body) = doc.body() {
                        let _ = body.style().set_property("background", &bg.0);
                    }
                }
            }
        });
    }
}

// =============================================================================
// Per-target SDK-handler registration. Called by the CLI-generated
// wrapper before mount.
// =============================================================================

#[cfg(target_arch = "wasm32")]
pub fn register_extensions(_backend: &mut backend_web::WebBackend) {
    // Push the initial window size + wire a resize listener into the
    // framework's reactive viewport signal. The CLI-generated wrapper
    // installs scheduler/time-source/render-loop here too, but the
    // shipped version of the CLI pre-dates `install_viewport_observer`.
    // Calling it from the user crate keeps the responsive layout
    // working without re-installing the CLI; once the CLI is bumped,
    // this line is harmless (the install is idempotent).
    backend_web::install_viewport_observer();
}

#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub fn register_extensions(_backend: &mut backend_ios::IosBackend) {
    // Install the iOS render-loop driver so the embedded `Simulator`
    // component's wgpu host gets per-frame ticks. The driver is the
    // NSTimer at ~60 Hz from `backend-ios-core::render_loop`; the
    // `backend-ios-mobile` re-export is gated on the `async-driver`
    // feature (enabled in this crate's `Cargo.toml`). Same role as
    // `backend_web::install_async_executor` + `install_render_loop`
    // in the wasm32 register_extensions above.
    backend_ios::install_render_loop();
}

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
pub fn register_extensions(_backend: &mut backend_android::AndroidBackend) {}

// macOS native — but NOT when the `terminal` feature is on. The terminal
// target builds for the macOS host triple, so without `not(feature =
// "terminal")` this and the terminal arm below would both compile on a
// macOS host and collide as duplicate definitions.
#[cfg(all(target_os = "macos", not(target_arch = "wasm32"), not(feature = "terminal")))]
pub fn register_extensions(_backend: &mut backend_macos::MacosBackend) {}

// Terminal — selected by the `terminal` feature (the CLI's terminal
// wrapper enables it), not a `target_os` cfg, because the terminal target
// builds for the host triple and would otherwise be shadowed by the host's
// native backend (macOS). Still excluded under `ssr` (SSR uses the separate
// `register_ssr_extensions` symbol below).
#[cfg(all(not(feature = "ssr"), feature = "terminal"))]
pub fn register_extensions(_backend: &mut backend_terminal::TerminalBackend) {}

// Recorder-side registration for the runtime-server sidecar. Distinct fn
// name (not an overload of `register_extensions`) so it never collides
// with the host target's per-backend overload when both compile in the
// sidecar build. Gated by `sidecar` (set only by the generated sidecar
// wrapper) so device/web builds never pull `dev-server`.
#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(backend: &mut dev_server::WireRecordingBackend) {
    swap_navigator::recording::register(backend);
}

/// SSR build path. The CLI's `idealyst dev --ssr` / `--static` wrapper
/// calls this once per request to install the SDK chrome handlers
/// `backend-ssr` invokes when it renders a navigator / external. Gated
/// by the `ssr` cargo feature so non-SSR builds don't pay the
/// `backend-ssr` dep cost. Separate symbol name (not
/// `register_extensions`) so it coexists with the per-platform impls
/// above without a cfg-collision on the wrapper's host triple.
#[cfg(feature = "ssr")]
pub fn register_ssr_extensions(backend: &mut backend_ssr::SsrBackend) {
    swap_navigator::register_generic(backend);
}
