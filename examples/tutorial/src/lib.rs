//! `tutorial` — a step-by-step teacher for idealyst-native's core
//! concepts, built with the framework itself.
//!
//! Three core tracks (Reactivity, Stylesheets, Media queries) teach the
//! `runtime-core` surface *directly* — signals, effects, `stylesheet!`,
//! breakpoint overlays — without leaning on the `idea-ui` component kit
//! for the concepts themselves. (The tutorial's own chrome does use
//! idea-ui; that's just the shell, not the lesson.) An Advanced track
//! is scaffolded for the deeper topics (custom backends, interactive
//! CLIs, embedded rendering) that come later.
//!
//! The shell is a `DrawerNavigator`: a persistent sidebar lists the
//! tracks and steps and survives navigation; the navigator swaps only
//! the step body. Each step ends with a prev/next bar derived from the
//! linear order in `routes`.

use runtime_core::{component, signal, ui, Element, Ref, Route, Signal};
use runtime_core::primitives::navigator::Screen;
use drawer_navigator::{
    install_navigator_pin_width, DrawerBuilder, DrawerHandle, DrawerNavigator, DrawerScreenExt,
};
use idea_ui::{install_idea_theme, light_theme};

/// Wrap a lesson's `Element` in a `Screen` whose nav-bar title is
/// the sidebar `IndexEntry::label` for the route. Without this the
/// iOS `UINavigationItem.title` / Android `Toolbar` title render
/// empty (iOS no longer falls back to `route.name()`).
fn titled(route: &'static Route<()>, el: Element) -> Screen {
    let label = label_for_route(route.name()).unwrap_or_else(|| route.name());
    Screen::new(el).title(label)
}

fn label_for_route(route_name: &'static str) -> Option<&'static str> {
    for section in routes::SECTIONS {
        for entry in section.entries {
            if entry.route.name() == route_name {
                return Some(entry.label);
            }
        }
    }
    None
}

mod common;
mod lessons;
mod routes;
mod shell;
mod styles;

use routes::{
    A11Y_DEFAULTS_ROUTE, A11Y_MODEL_ROUTE, ADV_BACKENDS_ROUTE, ADV_CLI_ROUTE, ADV_EMBEDDED_ROUTE,
    HOME_ROUTE, MQ_BREAKPOINTS_ROUTE, MQ_MOBILE_FIRST_ROUTE, MQ_SIGNAL_ROUTE, RX_BATCHING_ROUTE,
    RX_DERIVED_ROUTE, RX_EFFECTS_ROUTE, RX_SIGNALS_ROUTE, ST_STYLESHEETS_ROUTE, ST_TOKENS_ROUTE,
    ST_VARIANTS_ROUTE,
};

#[component]
pub fn app() -> Element {
    install_idea_theme(light_theme());

    let nav: Ref<DrawerHandle> = Ref::new();
    // App-level dark-mode state — lifted out of any screen scope so it
    // survives navigation. Captured by the sidebar builder below.
    let is_dark: Signal<bool> = signal!(false);

    // Pin the sidebar (vs. modal slide-in) at wide viewports.
    install_navigator_pin_width(900.0);

    let builder = DrawerNavigator::new(&HOME_ROUTE)
        .screen(HOME_ROUTE, move |_| titled(&HOME_ROUTE, lessons::home::page()))
        // Reactivity
        .screen(RX_SIGNALS_ROUTE, move |_| titled(&RX_SIGNALS_ROUTE, lessons::reactivity::signals()))
        .screen(RX_EFFECTS_ROUTE, move |_| titled(&RX_EFFECTS_ROUTE, lessons::reactivity::effects()))
        .screen(RX_DERIVED_ROUTE, move |_| titled(&RX_DERIVED_ROUTE, lessons::reactivity::derived()))
        .screen(RX_BATCHING_ROUTE, move |_| titled(&RX_BATCHING_ROUTE, lessons::reactivity::batching()))
        // Stylesheets
        .screen(ST_TOKENS_ROUTE, move |_| titled(&ST_TOKENS_ROUTE, lessons::stylesheets::tokens()))
        .screen(ST_STYLESHEETS_ROUTE, move |_| titled(&ST_STYLESHEETS_ROUTE, lessons::stylesheets::stylesheets()))
        .screen(ST_VARIANTS_ROUTE, move |_| titled(&ST_VARIANTS_ROUTE, lessons::stylesheets::variants()))
        // Media queries
        .screen(MQ_BREAKPOINTS_ROUTE, move |_| titled(&MQ_BREAKPOINTS_ROUTE, lessons::media_queries::breakpoints()))
        .screen(MQ_MOBILE_FIRST_ROUTE, move |_| titled(&MQ_MOBILE_FIRST_ROUTE, lessons::media_queries::mobile_first()))
        .screen(MQ_SIGNAL_ROUTE, move |_| titled(&MQ_SIGNAL_ROUTE, lessons::media_queries::signal_escape()))
        // Accessibility
        .screen(A11Y_DEFAULTS_ROUTE, move |_| titled(&A11Y_DEFAULTS_ROUTE, lessons::accessibility::defaults()))
        .screen(A11Y_MODEL_ROUTE, move |_| titled(&A11Y_MODEL_ROUTE, lessons::accessibility::model()))
        // Advanced (scaffolded)
        .screen(ADV_BACKENDS_ROUTE, move |_| titled(&ADV_BACKENDS_ROUTE, lessons::advanced::custom_backends()))
        .screen(ADV_CLI_ROUTE, move |_| titled(&ADV_CLI_ROUTE, lessons::advanced::interactive_cli()))
        .screen(ADV_EMBEDDED_ROUTE, move |_| titled(&ADV_EMBEDDED_ROUTE, lessons::advanced::embedded()))
        .drawer_width(280.0)
        .leading_with(move |slot| shell::sidebar(slot, is_dark));

    ui! { builder.bind(nav) }
}

// =============================================================================
// Per-target SDK-handler registration. Called by the CLI-generated
// wrapper before mount.
// =============================================================================

#[cfg(target_arch = "wasm32")]
pub fn register_extensions(backend: &mut backend_web::WebBackend) {
    drawer_navigator::register(backend);
    idea_codeblock::register(backend);
    // Wire the framework's reactive viewport signal so `current_breakpoint()`
    // (taught in the Media queries track) actually updates on resize.
    backend_web::install_viewport_observer();
}

#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub fn register_extensions(backend: &mut backend_ios::IosBackend) {
    drawer_navigator::register(backend);
}

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
pub fn register_extensions(backend: &mut backend_android::AndroidBackend) {
    drawer_navigator::register(backend);
}

#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
pub fn register_extensions(backend: &mut backend_macos::MacosBackend) {
    drawer_navigator::register(backend);
}

#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android", target_os = "macos")))]
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
