//! `tutorial` — a step-by-step teacher for idealyst-native's core
//! concepts, built with the framework itself.
//!
//! Three core tracks (Reactivity, Stylesheets, Media queries) teach the
//! runtime-v2 author surface *directly* — signals, the flush boundary,
//! effects, `stylesheet!`, breakpoint overlays — without leaning on the
//! `idea-ui` component kit for the concepts themselves. (The tutorial's
//! own chrome does use idea-ui; that's the shell, not the lesson.) An
//! Advanced track is scaffolded for the deeper topics (custom backends,
//! interactive CLIs, embedded rendering) that come later.
//!
//! Every Rust snippet in the Reactivity and Foundations tracks is
//! `include_str!`-ed from a real module in [`samples`], so `cargo check`
//! is the gate on the teaching material: a sample that stops compiling
//! stops shipping. The interactive panels in [`demo`] run the same
//! mechanisms live, which is how a reader can watch a staged write land
//! at the flush instead of taking the prose's word for it.
//!
//! The shell is a swap navigator (outlet model) wrapped in an
//! `idea_ui_nav::AppShell`: the sidebar lists the tracks and steps,
//! builds ONCE, and survives every navigation; the navigator swaps only
//! the outlet's screen. Pinned-sidebar ⇄ off-canvas drawer is static
//! breakpoint styling (real `@media` on web + SSR). Each step ends with
//! a prev/next bar derived from the linear order in `routes`.

use idea_ui::{install_idea_theme, light_theme};
use idea_ui_nav::AppShell;
use runtime_core::{
    component, effect, signal, ui, Breakpoint, Element, Ref, Route, Screen, Signal,
};
use swap_navigator::{MountPolicy, SwapBuilder, SwapHandle, SwapNavigator};

/// Wrap a lesson's `Element` in a `Screen`. The label is drawn by the
/// shell's own header (`shell::mobile_header` derives it reactively
/// from `active_route` via [`label_for_route`]), so the Screen carries
/// no navigator-chrome options anymore.
fn titled(route: &'static Route<()>, el: Element) -> Screen {
    let _ = route;
    Screen::new(el)
}

/// Sidebar `IndexEntry::label` for a route name — drives the mobile
/// header's reactive title (`shell::mobile_header`).
pub(crate) fn label_for_route(route_name: &'static str) -> Option<&'static str> {
    for section in routes::SECTIONS {
        for entry in section.entries {
            if entry.route.name() == route_name {
                return Some(entry.label);
            }
        }
    }
    None
}

mod chart;
mod common;
mod demo;
mod lessons;
mod routes;
mod samples;
mod shell;
mod styles;

use routes::{
    A11Y_DEFAULTS_ROUTE, A11Y_MODEL_ROUTE, ADV_BACKENDS_ROUTE, ADV_CLI_ROUTE, ADV_EMBEDDED_ROUTE,
    ARCH_BACKENDS_ROUTE, ARCH_CATALOG_ROUTE, ARCH_OVERVIEW_ROUTE, ARCH_SDKS_ROUTE,
    CORE_ENGINE_ROUTE, CORE_FLUSH_ROUTE, HOME_ROUTE, MQ_BREAKPOINTS_ROUTE, MQ_CONTAINER_ROUTE,
    MQ_MOBILE_FIRST_ROUTE, MQ_SIGNAL_ROUTE, RX_DERIVED_ROUTE, RX_EFFECTS_ROUTE, RX_FLUSH_ROUTE,
    RX_SIGNALS_ROUTE, ST_STYLESHEETS_ROUTE, ST_TOKENS_ROUTE, ST_VARIANTS_ROUTE,
};

#[component]
pub fn app() -> Element {
    // Start light and let the reader flip the sidebar's Dark switch.
    //
    // Deliberate, not a limitation: the tutorial's prose and screenshots
    // are written against the light theme, so a reader on a dark-mode OS
    // should still land on what the text describes. `color_scheme()` does
    // report the real platform preference here — the boot seam installs it
    // before the build (`runtime_vocabulary::backend::install_env_services`)
    // — so seeding from it would be a one-line change if that ever became
    // the wanted behavior. `examples/whiteboard-demo` is the app that does.
    install_idea_theme(light_theme());

    let nav: Ref<SwapHandle> = Ref::new();
    // Drawer-open state for narrow viewports — author-owned (the
    // AppShell scrim + the auto-close effect close it; the mobile
    // header's hamburger opens it). Pinned widths ignore it entirely.
    let drawer_open: Signal<bool> = signal(false);
    // App-level dark-mode state — lifted out of any screen scope so it
    // survives navigation. Captured by the sidebar builder below.
    let is_dark: Signal<bool> = signal(false);

    // Pin the sidebar at wide viewports: align the framework's `Lg`
    // breakpoint with the tutorial's 900-dp collapse point so
    // `AppShell(pin_at = Lg)` and the mobile header's `breakpoint lg`
    // overlay flip at the SAME width. First-install wins — must run
    // before any breakpoint-keyed sheet resolves.
    let _ = runtime_core::install_breakpoints(runtime_core::Breakpoints {
        lg_min: 900.0,
        ..Default::default()
    });

    let builder = SwapNavigator::new(&HOME_ROUTE)
        .screen(HOME_ROUTE, move |_| titled(&HOME_ROUTE, lessons::home::page()))
        // Foundations
        .screen(CORE_ENGINE_ROUTE, move |_| titled(&CORE_ENGINE_ROUTE, lessons::foundations::engine()))
        .screen(CORE_FLUSH_ROUTE, move |_| titled(&CORE_FLUSH_ROUTE, lessons::foundations::flush_boundary()))
        // Architecture
        .screen(ARCH_OVERVIEW_ROUTE, move |_| titled(&ARCH_OVERVIEW_ROUTE, lessons::architecture::overview()))
        .screen(ARCH_BACKENDS_ROUTE, move |_| titled(&ARCH_BACKENDS_ROUTE, lessons::architecture::backends()))
        .screen(ARCH_CATALOG_ROUTE, move |_| titled(&ARCH_CATALOG_ROUTE, lessons::architecture::catalog()))
        .screen(ARCH_SDKS_ROUTE, move |_| titled(&ARCH_SDKS_ROUTE, lessons::architecture::sdks()))
        // Reactivity
        .screen(RX_SIGNALS_ROUTE, move |_| titled(&RX_SIGNALS_ROUTE, lessons::reactivity::signals()))
        .screen(RX_FLUSH_ROUTE, move |_| titled(&RX_FLUSH_ROUTE, lessons::reactivity::flush()))
        .screen(RX_EFFECTS_ROUTE, move |_| titled(&RX_EFFECTS_ROUTE, lessons::reactivity::effects()))
        .screen(RX_DERIVED_ROUTE, move |_| titled(&RX_DERIVED_ROUTE, lessons::reactivity::derived()))
        // Stylesheets
        .screen(ST_TOKENS_ROUTE, move |_| titled(&ST_TOKENS_ROUTE, lessons::stylesheets::tokens()))
        .screen(ST_STYLESHEETS_ROUTE, move |_| titled(&ST_STYLESHEETS_ROUTE, lessons::stylesheets::stylesheets()))
        .screen(ST_VARIANTS_ROUTE, move |_| titled(&ST_VARIANTS_ROUTE, lessons::stylesheets::variants()))
        // Media queries
        .screen(MQ_BREAKPOINTS_ROUTE, move |_| titled(&MQ_BREAKPOINTS_ROUTE, lessons::media_queries::breakpoints()))
        .screen(MQ_MOBILE_FIRST_ROUTE, move |_| titled(&MQ_MOBILE_FIRST_ROUTE, lessons::media_queries::mobile_first()))
        .screen(MQ_SIGNAL_ROUTE, move |_| titled(&MQ_SIGNAL_ROUTE, lessons::media_queries::signal_escape()))
        .screen(MQ_CONTAINER_ROUTE, move |_| titled(&MQ_CONTAINER_ROUTE, lessons::media_queries::container_queries()))
        // Accessibility
        .screen(A11Y_DEFAULTS_ROUTE, move |_| titled(&A11Y_DEFAULTS_ROUTE, lessons::accessibility::defaults()))
        .screen(A11Y_MODEL_ROUTE, move |_| titled(&A11Y_MODEL_ROUTE, lessons::accessibility::model()))
        // Advanced (scaffolded)
        .screen(ADV_BACKENDS_ROUTE, move |_| titled(&ADV_BACKENDS_ROUTE, lessons::advanced::custom_backends()))
        .screen(ADV_CLI_ROUTE, move |_| titled(&ADV_CLI_ROUTE, lessons::advanced::interactive_cli()))
        .screen(ADV_EMBEDDED_ROUTE, move |_| titled(&ADV_EMBEDDED_ROUTE, lessons::advanced::embedded()))
        // One screen resident at a time; switching away drops the
        // screen's realized scope (and with it every signal and effect
        // its lesson created) and a return rebuilds it fresh. That is
        // drop-as-teardown, and it is why the interactive demos start
        // over when you navigate away and back.
        .mount_policy(MountPolicy::LazyDisposing)
        // The shell: AppShell packages pinned-sidebar ⇄ drawer around the
        // one-shot outlet; the mobile header (hamburger + reactive title)
        // collapses in below the pin width.
        .layout(move |nav_ctx| {
            // Auto-close the drawer when a sidebar link navigates while
            // unpinned. Reading `active_route` inside the effect
            // subscribes it to every navigation.
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
                    width = 280.0,
                ) {
                    { content }
                }
            }
        });

    ui! { builder.bind(nav) }
}

// =============================================================================
// Registration seams the CLI-generated wrappers call before mount.
//
// Runtime v2 has no separate "external primitive" concept: the scene
// `Registry` treats primitives and third-party payloads uniformly, so an
// SDK registers its handler exactly the way `register_builtins` registers
// a core primitive. The tutorial's one such SDK is `codeblock` (the
// syntax-tinted code panels), and its `register` IS the seam — no wrapper
// fn needed. A payload with no registered handler panics at realize, so a
// missing seam fails loudly instead of silently drawing a placeholder.
// =============================================================================

/// Web / macOS / iOS / terminal wrappers: `start_in`, `hydrate_in`,
/// `newcore::run_with`, `newcore::run_in_view`, `newcore::run` all take
/// this as their `register` argument (invoked after
/// `runtime_vocabulary::register_builtins`).
pub use codeblock::register as register_scene_extensions;

/// SSR / SSG wrappers (`backend_ssr::newcore::render_path_with`,
/// `render_all`). Same generic handler — the code panels render the same
/// `<pre>`/span DOM server-side as they do in the browser.
pub use codeblock::register as register_ssr_scene_handlers;

/// The `idealyst dev` sidecar's wire recorder
/// (`dev_server::sidecar::run_newcore`). Same handler again, specialised
/// to the recording backend, so a dev session serializes real code
/// panels over the wire.
#[cfg(feature = "sidecar")]
pub fn register_scene_extensions_recorder(registry: &mut dev_server::newcore::SceneRegistry) {
    codeblock::register(registry);
}

/// Android: the generated wrapper's attach branch mounts `scene_app()`
/// through `backend_android::newcore::start`. Under the facade alias
/// `app()` already returns the scene `Element`, so this is the
/// conventionally-named shim.
pub fn scene_app() -> Element {
    app()
}
