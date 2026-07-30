//! idea-ui-docs — the self-referencing component reference for idea-ui.
//!
//! A single [`routes::CATALOG`] table drives everything: the grouped,
//! searchable sidebar, the custom header bar with the Light/Dark toggle,
//! and the per-screen page frame (`shell::page_frame`) that renders the
//! group overline, title, status badge, lead, body, and Usage panel.
//!
//! Navigation is the outlet model: a [`SwapNavigator`] owns the screens
//! and swaps the one visible screen into its outlet; the chrome is plain
//! author layout built ONCE in `.layout(...)` — an `idea_ui_nav::AppShell`
//! (pinned sidebar at ≥ 900 px, off-canvas drawer below) around a
//! header-over-outlet column. The same chrome renders on every backend
//! (CLAUDE.md §7) — there is no per-platform navigator chrome anymore.
//!
//! ## Crate layout
//!
//! - `lib.rs` (this) — `app()` entry: theme install + SwapNavigator wiring.
//! - `routes.rs` — the `CATALOG` (groups → entries) + route constants.
//! - `shell.rs` — header / sidebar / `page_frame` + page-template helpers.
//! - `styles.rs` — local stylesheets for chrome only.
//! - `pages/*.rs` — one module per design group; each exports body-only
//!   `pub fn name() -> Element`.

use runtime_core::{effect, signal, ui, Breakpoint, Element, Signal};
use idea_ui_nav::AppShell;
use idea_ui::{install_idea_theme, light_theme};

mod pages;
mod routes;
mod shell;
mod styles;

use routes::{CATALOG, DEFAULT_ROUTE};

// =============================================================================
// Per-target SDK-handler registration. Called by the CLI-generated
// wrapper before mount.
// =============================================================================

#[cfg(target_arch = "wasm32")]
pub fn register_extensions(_backend: &mut backend_web::WebBackend) {
    // Push the initial window size + wire a resize listener into the
    // framework's reactive viewport signal, so the AppShell's
    // pin/drawer split follows a live resize.
    backend_web::install_viewport_observer();
}

/// Boot-time scene-registry seam — the CLI-generated wrapper calls this
/// with the fresh registry after `runtime_vocabulary::register_builtins`.
///
/// `codeblock::register` so `shell::CodePanel` renders the SDK's
/// `<pre>`/span handler, and `table::register_handlers` so PropsTable /
/// the Table pages render the SDK's real `<table>`/`<tr>`/`<td>`.
/// Without a registration those items have no registry entry and
/// realization panics (the scene contract fails loud — the old core
/// rendered a placeholder box instead).
///
/// Registry-generic: the same fn serves the web boot, the native hosts'
/// `newcore::run_with`, and the GPU desktop host
/// (`websites/idea-ui-docs-gpu`).
pub fn register_scene_extensions<H>(registry: &mut runtime_scene::Registry<H>)
where
    H: runtime_vocabulary::style_attach::StyleServices
        + runtime_vocabulary::caps::TextOps
        + runtime_vocabulary::caps::InputOps
        + 'static,
{
    codeblock::register(registry);
    table::register_handlers(registry);
}

#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub fn register_extensions(_backend: &mut backend_ios::IosBackend) {}

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
pub fn register_extensions(_backend: &mut backend_android::AndroidBackend) {}

#[cfg(all(target_os = "macos", not(target_arch = "wasm32"), not(feature = "terminal")))]
pub fn register_extensions(_backend: &mut backend_macos::MacosBackend) {}

#[cfg(feature = "terminal")]
pub fn register_extensions(_backend: &mut backend_terminal::TerminalBackend) {}

/// Recorder-side registration for the runtime-server sidecar
/// (`dev_server::sidecar::run_newcore`).
#[cfg(feature = "sidecar")]
pub fn register_scene_extensions_recorder(registry: &mut dev_server::newcore::SceneRegistry) {
    register_scene_extensions(registry);
}

// =============================================================================
// The app — the whole catalog/chrome over the vocabulary swap navigator
// (`runtime_vocabulary::builders::swap_navigator`). The layout closure
// takes no context argument: the navigator provides `SwapNav` into the
// world context (`runtime_world::inject`), and the outlet is
// `runtime_vocabulary::navigator_outlet()`.
// =============================================================================

/// Root entry — the symbol every boot path mounts (the CLI-generated
/// per-platform wrappers, the sidecar, and `standalone.html`'s
/// `web_entry` below).
pub fn app() -> Element {
    install_idea_theme(light_theme());

    // App-level state surviving navigation — created here because
    // `app` runs inside `World::enter` (the boot entry enters the world
    // around the build), so these are world-root-owned. Signals are Copy
    // handles; the layout closure captures them by value.
    let is_dark: Signal<bool> = signal(false);
    let q: Signal<String> = signal(String::new());
    let drawer_open: Signal<bool> = signal(false);

    let _ = runtime_core::install_breakpoints(runtime_core::Breakpoints {
        lg_min: 900.0,
        ..Default::default()
    });

    // One screen per catalog entry — same page/landing frames; the shell
    // helpers are core-agnostic through the facade alias.
    let mut builder = runtime_vocabulary::builders::swap_navigator(DEFAULT_ROUTE);
    for group in CATALOG {
        for entry in group.entries {
            let is_landing = entry.route.name() == routes::OVERVIEW_ROUTE.name();
            builder = builder.screen(entry.route.clone(), move |_| {
                if is_landing {
                    shell::landing_frame(entry)
                } else {
                    shell::page_frame(entry)
                }
            });
        }
    }

    builder
        // One screen resident at a time: switching away disposes the
        // screen's scope and a return rebuilds it fresh — browser
        // semantics.
        .mount_policy(runtime_vocabulary::prims::MountPolicy::LazyDisposing)
        .layout(move || {
            // The navigator provides `SwapNav` for the layout build
            // window. Route links need no `on_select` stash —
            // `link(route=)` resolves the navigator's `LinkActivator`
            // context.
            let nav = runtime_world::inject::<runtime_vocabulary::prims::SwapNav>()
                .expect("SwapNav provided by the swap navigator mount");
            let active_route = nav.active_route;

            // Auto-close the drawer when a sidebar link navigates while
            // unpinned. Reading `active_route` inside the effect
            // subscribes it to every navigation; the layout closure runs
            // inside the navigator's retained chrome scope, so the
            // effect is owned by (and freed with) the navigator.
            effect!({
                let _ = active_route.get();
                if !idea_ui_nav::sidebar_pinned(Breakpoint::Lg) {
                    drawer_open.set(false);
                }
            });

            let sidebar_el = shell::sidebar(active_route, q);
            let header = shell::header(active_route, is_dark, drawer_open);
            let body: Element = ui! {
                view(style = shell::outlet_grow_style) {
                    { runtime_vocabulary::navigator_outlet().build() }
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
                    width = 252.0,
                ) {
                    { content }
                }
            }
        })
        .build()
}

// Standalone web boot: `wasm-pack build --target web` produces a module
// whose own start fn mounts into `#app` (see `standalone.html` for the
// build command). NON-default — the CLI web wrapper generates its own
// `#[wasm_bindgen(start)]`, and two starts in one module is a
// wasm-bindgen error.
#[cfg(all(target_arch = "wasm32", feature = "standalone-web"))]
mod web_entry {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen(start)]
    pub fn boot() {
        console_error_panic_hook::set_once();
        // Console logger so framework log lines reach devtools (the CLI
        // wrapper normally installs this).
        backend_web::install_logger();
        backend_web::newcore::start_in(
            "#app",
            crate::register_scene_extensions,
            crate::app,
        );
    }
}
