//! `tutorial` — a step-by-step teacher for idealyst-native, built with
//! the framework itself.
//!
//! It opens hands-on: **Getting started** (install the CLI, scaffold a
//! project, run it), **Building UI** (primitives, `ui!`, components,
//! state, control flow), **Styling** (`stylesheet!`, layout, themes) and
//! **Component stylesheets** (variants, interaction states, overrides,
//! `component_styles!` slots, restyling and extending idea-ui). Then the
//! deeper tracks — Foundations, Architecture, Reactivity, Media queries,
//! Accessibility — teach how the engine works underneath.
//!
//! The hands-on lessons are built around [`fiddle::Fiddle`]: an editable
//! `app.rs` whose preview is that code compiled by the fiddle server
//! (`examples/fiddle`, `idealyst build --web`) and framed live. Each
//! fiddle's starting code is a module in [`fiddles`], so it compiles as
//! part of this crate — a starter that drifts from the framework's surface
//! fails `cargo check -p tutorial` before it reaches a reader.
//!
//! Every other Rust snippet is `include_str!`-ed from a real module in
//! [`samples`], under the same gate. The interactive panels in [`demo`]
//! run the reactivity mechanisms live, which is how a reader can watch a
//! staged write land at the flush instead of taking the prose's word for
//! it.
//!
//! The shell is a swap navigator (outlet model) wrapped in an
//! `idea_ui_nav::AppShell`: the sidebar lists the tracks and steps,
//! builds ONCE, and survives every navigation; the navigator swaps only
//! the outlet's screen. Pinned-sidebar ⇄ off-canvas drawer is static
//! breakpoint styling (real `@media` on web + SSR). Each step ends with
//! a prev/next bar derived from the linear order in `routes`.

use idea_ui::{install_idea_theme, light_theme, Card, CardPadding, CardSlotStyles, CardVariant};
use idea_ui_nav::AppShell;
use runtime_core::{
    component, effect, rx, signal, ui, Breakpoint, Element, Ref, Route, Screen, Signal,
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

/// A lesson's numbered title (`1.2 Components & props`) for a route name
/// — the mobile header's reactive title (`shell::mobile_header`).
pub(crate) fn label_for_route(route_name: &'static str) -> Option<String> {
    let (c, i, lesson) = course::find(route_name)?;
    Some(format!("{} {}", course::lesson_number(c, i), lesson.title))
}

mod chart;
mod common;
mod course;
mod demo;
mod fiddle;
mod fiddles;
mod terminal;
mod lessons;
mod routes;
mod samples;
mod shell;
mod styles;

use routes::{
    A11Y_DEFAULTS_ROUTE, A11Y_MODEL_ROUTE, ADV_BACKENDS_ROUTE, ADV_CLI_ROUTE, ADV_EMBEDDED_ROUTE,
    ARCH_BACKENDS_ROUTE, ARCH_CATALOG_ROUTE, ARCH_OVERVIEW_ROUTE, ARCH_SDKS_ROUTE,
    CORE_ENGINE_ROUTE, CORE_FLUSH_ROUTE, CS_OVERRIDES_ROUTE, CS_RESTYLE_ROUTE, CS_SLOTS_ROUTE,
    CS_STATES_ROUTE, CS_VARIANTS_ROUTE, HOME_ROUTE, MQ_BREAKPOINTS_ROUTE, MQ_CONTAINER_ROUTE,
    MQ_MOBILE_FIRST_ROUTE, MQ_SIGNAL_ROUTE, RX_DERIVED_ROUTE, RX_EFFECTS_ROUTE, RX_FLUSH_ROUTE,
    RX_SIGNALS_ROUTE, START_ANATOMY_ROUTE, START_CREATE_ROUTE, START_EDITOR_ROUTE,
    START_INSTALL_ROUTE, START_MCP_ROUTE, START_RUN_ROUTE, STY_LAYOUT_ROUTE,
    STY_SHEETS_ROUTE, STY_THEMES_ROUTE, UI_CHILDREN_ROUTE, UI_COMPONENTS_ROUTE, UI_FLOW_ROUTE,
    UI_PRIMITIVES_ROUTE, UI_REACTIVE_PROPS_ROUTE, UI_STATE_ROUTE,
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
    // The reader's background, picked on the course landing page. Owned
    // here so the choice survives navigating into a lesson and back.
    let background: Signal<String> = signal(String::from("new"));
    // Whether the pinned sidebar is hidden — app-level, so it survives
    // navigation; provided so the toggle beside each page's title
    // (`shell::sidebar_toggle`) reaches it.
    let sidebar_hidden: Signal<bool> = signal(false);
    runtime_core::provide(shell::SidebarHidden(sidebar_hidden));

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
        .screen(HOME_ROUTE, move |_| titled(&HOME_ROUTE, lessons::home::page(background)))
        // Getting started
        .screen(START_INSTALL_ROUTE, move |_| titled(&START_INSTALL_ROUTE, lessons::getting_started::install()))
        .screen(START_CREATE_ROUTE, move |_| titled(&START_CREATE_ROUTE, lessons::getting_started::create()))
        .screen(START_RUN_ROUTE, move |_| titled(&START_RUN_ROUTE, lessons::getting_started::run()))
        .screen(START_EDITOR_ROUTE, move |_| titled(&START_EDITOR_ROUTE, lessons::getting_started::editor()))
        .screen(START_MCP_ROUTE, move |_| titled(&START_MCP_ROUTE, lessons::getting_started::mcp()))
        .screen(START_ANATOMY_ROUTE, move |_| titled(&START_ANATOMY_ROUTE, lessons::getting_started::anatomy()))
        // Building UI
        .screen(UI_PRIMITIVES_ROUTE, move |_| titled(&UI_PRIMITIVES_ROUTE, lessons::building_ui::primitives()))
        .screen(UI_COMPONENTS_ROUTE, move |_| titled(&UI_COMPONENTS_ROUTE, lessons::building_ui::components()))
        .screen(UI_CHILDREN_ROUTE, move |_| titled(&UI_CHILDREN_ROUTE, lessons::building_ui::children()))
        .screen(UI_STATE_ROUTE, move |_| titled(&UI_STATE_ROUTE, lessons::building_ui::state()))
        .screen(UI_FLOW_ROUTE, move |_| titled(&UI_FLOW_ROUTE, lessons::building_ui::control_flow()))
        .screen(UI_REACTIVE_PROPS_ROUTE, move |_| titled(&UI_REACTIVE_PROPS_ROUTE, lessons::building_ui::reactive_props()))
        // Styling
        .screen(STY_SHEETS_ROUTE, move |_| titled(&STY_SHEETS_ROUTE, lessons::styling::stylesheets()))
        .screen(STY_LAYOUT_ROUTE, move |_| titled(&STY_LAYOUT_ROUTE, lessons::styling::layout()))
        .screen(STY_THEMES_ROUTE, move |_| titled(&STY_THEMES_ROUTE, lessons::styling::themes()))
        // Component stylesheets
        .screen(CS_VARIANTS_ROUTE, move |_| titled(&CS_VARIANTS_ROUTE, lessons::component_styles::variants()))
        .screen(CS_STATES_ROUTE, move |_| titled(&CS_STATES_ROUTE, lessons::component_styles::states()))
        .screen(CS_OVERRIDES_ROUTE, move |_| titled(&CS_OVERRIDES_ROUTE, lessons::component_styles::overrides()))
        .screen(CS_SLOTS_ROUTE, move |_| titled(&CS_SLOTS_ROUTE, lessons::component_styles::slots()))
        .screen(CS_RESTYLE_ROUTE, move |_| titled(&CS_RESTYLE_ROUTE, lessons::component_styles::restyle()))
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
            // The page sits on a white elevated card, inset from the
            // secondary-colored backdrop the sidebar shares.
            let body: Element = ui! {
                view(style = shell::ContentBackdrop()) {
                    Card(
                        variant = CardVariant::Elevated,
                        padding = CardPadding::None,
                        styles = CardSlotStyles::default().root(shell::content_card_rules()),
                    ) {
                        { nav_ctx.outlet }
                    }
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
                    width = rx!(shell::sidebar_width(sidebar_hidden)),
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
// The scene `Registry` treats primitives and third-party payloads
// uniformly, so an SDK registers its handler exactly the way
// `register_builtins` registers a core primitive. The tutorial renders two
// SDK payloads: `codeblock` (the code panels and the fiddle's editor) and
// `webview` (the fiddle's preview frame). Both `register`s are generic and
// pick a backend's real leg by registry type, so one seam serves every
// platform. A payload with no registered handler panics at realize, so a
// missing line fails loudly instead of silently drawing a placeholder.
// =============================================================================

/// Web / macOS / iOS / terminal wrappers: `start_in`, `hydrate_in`,
/// `newcore::run_with`, `newcore::run_in_view`, `newcore::run` all take
/// this as their `register` argument (invoked after
/// `runtime_vocabulary::register_builtins`).
pub fn register_scene_extensions<H>(registry: &mut runtime_scene::Registry<H>)
where
    H: runtime_scene::Host
        + runtime_vocabulary::caps::TextOps
        + runtime_vocabulary::caps::ExternalOps
        + runtime_vocabulary::style_attach::StyleServices
        + 'static,
{
    codeblock::register(registry);
    webview::register(registry);
}

/// SSR / SSG wrappers (`backend_ssr::newcore::render_path_with`,
/// `render_all`). The same seam: the code panels render the same
/// `<pre>`/span DOM server-side as in the browser, and the preview frame
/// takes webview's placeholder until the client mounts.
pub use register_scene_extensions as register_ssr_scene_handlers;

/// The `idealyst dev` sidecar's wire recorder
/// (`dev_server::sidecar::run_newcore`). Same seam again, specialised to
/// the recording backend, so a dev session serializes real code panels
/// over the wire.
#[cfg(feature = "sidecar")]
pub fn register_scene_extensions_recorder(registry: &mut dev_server::newcore::SceneRegistry) {
    register_scene_extensions(registry);
}

/// Android: the generated wrapper's attach branch mounts `scene_app()`
/// through `backend_android::newcore::start`. Under the facade alias
/// `app()` already returns the scene `Element`, so this is the
/// conventionally-named shim.
pub fn scene_app() -> Element {
    app()
}
