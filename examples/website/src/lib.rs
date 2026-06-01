//! `website` — the idealyst-native marketing site.
//!
//! Web is the primary target. The shell is a `DrawerNavigator` — its
//! sidebar slot mounts ONCE and survives every navigation; the
//! navigator only swaps the screen-body. The sidebar reads
//! `DrawerSlotProps::active_route` so the active-link highlight is
//! reactive without any per-screen wiring.
//!
//! iOS/Android: the same drawer SDK drives a slide-in side panel
//! using native chrome (`UINavigationController` / Android's
//! drawer). Terminal: drawer SDK no-ops the sidebar (per the
//! repo's terminal-minimalism convention).

use runtime_core::{component, effect, signal, ui, Color, Element, Ref, Route, Signal, Tokenized};
use runtime_core::primitives::navigator::Screen;
use drawer_navigator::{
    DrawerBuilder, DrawerHandle, DrawerNavigator, DrawerScreenExt, HeaderStyle, MountPolicy,
    TopSlot,
};
use idea_ui::{idea_color, install_idea_theme, light_theme};

/// Wrap a page's `Element` in a `Screen` whose nav-bar title is the
/// sidebar label resolved via [`routes::label_for_route`] (which
/// covers both `SECTIONS` entries and the off-sidebar tangents like
/// `/targets` and `/comparisons/*`). Drives both
/// `UINavigationItem.title` on iOS and Android's `Toolbar` title;
/// without this the bar renders empty since iOS no longer falls
/// back to `route.name()`.
fn titled(route: &'static Route<()>, el: Element) -> Screen {
    Screen::new(el).title(routes::label_for_route(route.name()))
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
    AGENTIC_ROUTE, BACKENDS_ROUTE, CODE_SPLITTING_ROUTE, COMPARE_DIOXUS_ROUTE,
    COMPARE_ELECTRON_ROUTE, COMPARE_FLUTTER_ROUTE, COMPARE_REACT_ROUTE,
    COMPARE_WEB_FRAMEWORKS_ROUTE, COMPARE_WHEN_NOT_ROUTE, COMPARISONS_ROUTE, CONCEPTS_ROUTE,
    CROSS_PLATFORM_ROUTE, DEMO_ROUTE, FEATURES_ROUTE, FURTHER_READING_ROUTE, HOME_ROUTE,
    INSTALL_ROUTE, PERFORMANCE_ROUTE, QUICKSTART_ROUTE, ROADMAP_ROUTE, SERVER_FUNCTIONS_ROUTE,
    SSR_ROUTE, TARGETS_ROUTE, TYPE_SAFETY_ROUTE, WHY_RUST_ROUTE,
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

    let nav: Ref<DrawerHandle> = Ref::new();
    // App-level theme-toggle state — lives here (not inside a
    // per-screen scope) so flipping dark mode persists across
    // navigation. Captured by the sidebar builder closure below.
    let is_dark: Signal<bool> = signal!(false);

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

    // Pin the drawer sidebar's modal↔pinned breakpoint to the site's
    // existing collapse point. This is the SINGLE threshold now: the
    // navigator's shared stylesheet (web + SSR) carries the responsive
    // `@media` query, so the collapse is correct on the static first
    // paint — and it matches the mobile-header collapse
    // (`collapse_responsive_style`, also keyed off `SIDEBAR_COLLAPSE_PX`).
    // Must run before the navigator registers its sheet below — and on
    // EVERY target (not just web), since SSR ships the same sheet.
    drawer_navigator::install_navigator_pin_width(responsive::SIDEBAR_COLLAPSE_PX as f32);

    // Site-only responsive chrome: the backdrop overlay + narrow-screen
    // `<pre>` wrapping (the sidebar collapse itself is now navigator-owned,
    // above). The CSS injection is a no-op on non-web targets.
    responsive::install_responsive_css();

    let builder = DrawerNavigator::new(&HOME_ROUTE)
        // Navigator-level header theming: closures re-resolve their
        // token reads every time the iOS slot-style Effect / Android
        // slot_styles dispatcher re-fires on `set_idea_theme` token
        // swaps. Without this the iOS UINavigationController bar /
        // Android Toolbar keeps the platform default that was
        // installed once at create_drawer time and never re-tints.
        .header(|| HeaderStyle {
            background: Some((idea_color(|c| c.surface.clone()))()),
            title: Some((idea_color(|c| c.text.clone()))()),
            tint: Some((idea_color(|c| c.text.clone()))()),
            body_background: Some((idea_color(|c| c.background.clone()))()),
        })
        // Home embeds a wgpu-backed `Simulator` running the welcome
        // demo. Mark it `LazyDisposing` so navigating away tears
        // down the home screen's reactive scope (and with it the
        // `IosHostHandle` driving the embedded wgpu device + render
        // loop + welcome's `raf_loop`). Navigating back rebuilds
        // the screen from scratch — a fresh `Graphics::on_ready`
        // mounts a new host, and the welcome timeline replays from
        // initial state. Models the intended pattern for any
        // graphics-heavy screen in a multi-screen navigator.
        .screen(HOME_ROUTE, move |_| {
            titled(&HOME_ROUTE, pages::home::page()).mount_policy(MountPolicy::LazyDisposing)
        })
        .screen(FEATURES_ROUTE, move |_| titled(&FEATURES_ROUTE, pages::features::page()))
        .screen(CROSS_PLATFORM_ROUTE, move |_| titled(&CROSS_PLATFORM_ROUTE, pages::cross_platform::page()))
        .screen(PERFORMANCE_ROUTE, move |_| titled(&PERFORMANCE_ROUTE, pages::performance::page()))
        .screen(TYPE_SAFETY_ROUTE, move |_| titled(&TYPE_SAFETY_ROUTE, pages::type_safety::page()))
        .screen(SSR_ROUTE, move |_| titled(&SSR_ROUTE, pages::ssr::page()))
        .screen(INSTALL_ROUTE, move |_| titled(&INSTALL_ROUTE, pages::install::page()))
        .screen(QUICKSTART_ROUTE, move |_| titled(&QUICKSTART_ROUTE, pages::quickstart::page()))
        .screen(CONCEPTS_ROUTE, move |_| titled(&CONCEPTS_ROUTE, pages::concepts::page()))
        .screen(WHY_RUST_ROUTE, move |_| titled(&WHY_RUST_ROUTE, pages::why_rust::page()))
        .screen(DEMO_ROUTE, move |_| titled(&DEMO_ROUTE, pages::demo::page()))
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
        .drawer_width(260.0)
        // Leading slot — the persistent sidebar. Runs ONCE at
        // navigator init; survives every screen swap.
        .leading_with(move |slot| {
            // The slot builder runs inside an active reactive scope
            // (the navigator's leading-slot scope). Install the
            // backdrop class-toggle observer here so it anchors to
            // a scope that lives as long as the navigator does.
            responsive::install_drawer_open_observer(slot.is_open, nav);
            shell::sidebar(slot, is_dark)
        })
        // Top slot — mobile header. Persistent across screens, so
        // the menu icon, title, etc. don't rebuild on every nav.
        // The closure renders a reactive `when()` that mounts the
        // header only at narrow widths and empty otherwise.
        .top_with(TopSlot::Custom(Box::new(|slot| shell::mobile_header(slot))));
    // No `bottom_with(...)`: the site footer is no longer a pinned
    // navigator slot. The drawer navigator no longer owns scroll, so
    // the footer lives as the last child of each page's own
    // `scroll_view` (see `shell::layout` / `layout_with_toc`) and
    // scrolls with the page content.

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
pub fn register_extensions(backend: &mut backend_web::WebBackend) {
    drawer_navigator::register(backend);
    idea_codeblock::register(backend);
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
pub fn register_extensions(backend: &mut backend_ios::IosBackend) {
    drawer_navigator::register(backend);
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
pub fn register_extensions(backend: &mut backend_android::AndroidBackend) {
    drawer_navigator::register(backend);
}

#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
pub fn register_extensions(backend: &mut backend_macos::MacosBackend) {
    drawer_navigator::register(backend);
}

#[cfg(all(not(feature = "ssr"), not(any(target_arch = "wasm32", target_os = "ios", target_os = "android", target_os = "macos"))))]
pub fn register_extensions(backend: &mut backend_terminal::TerminalBackend) {
    drawer_navigator::register(backend);
}

// Recorder-side registration for the runtime-server sidecar. Distinct fn
// name (not an overload of `register_extensions`) so it never collides
// with the host target's per-backend overload when both compile in the
// sidecar build. Only the drawer navigator needs a recording handler.
// Gated by `sidecar` (set only by the generated sidecar wrapper) so
// device/web builds never pull `dev-server`.
#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(backend: &mut dev_server::WireRecordingBackend) {
    drawer_navigator::recording::register(backend);
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
    drawer_navigator::chrome::register(backend);
    idea_codeblock::register(backend);
}
