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

use runtime_core::primitives::navigator::Screen;
use runtime_core::{component, effect, signal, ui, Breakpoint, Element, Ref, Signal};
use swap_navigator::{MountPolicy, SwapBuilder, SwapHandle, SwapNavigator};
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
pub fn register_extensions(backend: &mut backend_web::WebBackend) {
    backend_web::install_viewport_observer();
    // Register the swap navigator handler on the web backend. The
    // crate self-registers via `inventory`, but under `--local` (no
    // runtime-server) the linker can dead-strip that submission since
    // nothing else pulls in the web module's object; the explicit call
    // forces linkage + registration so `SwapPresentation` resolves.
    swap_navigator::register(backend);
    // Same story for the `table` External SDK (idea-ui's Table / the docs
    // PropsTable lower to it): it's only referenced indirectly via idea-ui,
    // so under `--local` its inventory registrar gets stripped and Table
    // renders the "not supported on web" placeholder. Register it explicitly.
    // (`codeblock` is fine — `shell::CodePanel` calls `codeblock::code_block`
    // directly, which keeps its registrar linked.)
    table::register(backend);
}

// `codeblock` and `table` self-register via `inventory`; `use table as _`
// pins the crate in case the only reference is via idea-ui's re-export.
use table as _;

#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub fn register_extensions(_backend: &mut backend_ios::IosBackend) {}

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
pub fn register_extensions(_backend: &mut backend_android::AndroidBackend) {}

#[cfg(all(target_os = "macos", not(target_arch = "wasm32"), not(feature = "terminal")))]
pub fn register_extensions(_backend: &mut backend_macos::MacosBackend) {}

#[cfg(feature = "terminal")]
pub fn register_extensions(_backend: &mut backend_terminal::TerminalBackend) {}

#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(backend: &mut dev_server::WireRecordingBackend) {
    swap_navigator::recording::register(backend);
}

#[component]
pub fn app() -> Element {
    install_idea_theme(light_theme());

    let nav: Ref<SwapHandle> = Ref::new();
    // App-level state surviving navigation: dark-mode flag for the
    // header toggle, and the sidebar search query.
    let is_dark: Signal<bool> = signal(false);
    let q: Signal<String> = signal(String::new());
    // Drawer-open state for narrow viewports — author-owned (the
    // AppShell scrim + the auto-close effect below close it; the header
    // hamburger opens it). Pinned widths ignore it entirely.
    let drawer_open: Signal<bool> = signal(false);

    // Align the framework's `Lg` breakpoint with the docs' 900 px
    // collapse point so `AppShell(pin_at = Lg)`, the header hamburger's
    // visibility, and the auto-close effect all flip at the SAME width.
    // First-install wins — must run before any breakpoint-keyed sheet
    // resolves. (Replaces the legacy `install_navigator_pin_width(900.0)`.)
    let _ = runtime_core::install_breakpoints(runtime_core::Breakpoints {
        lg_min: 900.0,
        ..Default::default()
    });

    // Fold the catalog into one screen per entry. Each screen wraps the
    // entry's body in the central page frame — except the Overview
    // landing, which renders full-bleed via `landing_frame` (no title
    // block / Usage panel).
    let mut builder = SwapNavigator::new(DEFAULT_ROUTE);
    for group in CATALOG {
        for entry in group.entries {
            let route = entry.route.clone();
            let is_landing = entry.route.name() == routes::OVERVIEW_ROUTE.name();
            builder = builder.screen(route, move |_| {
                let content = if is_landing {
                    shell::landing_frame(entry)
                } else {
                    shell::page_frame(entry)
                };
                Screen::new(content)
            });
        }
    }

    let builder = builder
        // One screen resident at a time; switching away disposes the
        // screen's scope and a return rebuilds it fresh — matching the
        // old drawer-on-web engine (and browser semantics) exactly.
        .mount_policy(MountPolicy::LazyDisposing)
        // The shell: AppShell packages pinned-sidebar ⇄ drawer around
        // the one-shot outlet; the custom header (hamburger + brand +
        // token hint + Light/Dark toggle) sits above the outlet.
        .layout(move |nav_ctx| {
            // Auto-close the drawer when a sidebar link navigates while
            // unpinned (the legacy web drawer engine did this in its
            // Select arm; author-owned now). Reading `active_route`
            // inside the effect subscribes it to every navigation. The
            // layout closure runs inside the navigator's retained chrome
            // scope, so the effect is owned by (and freed with) the
            // navigator.
            let active_route = nav_ctx.active_route;
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
                    width = 252.0,
                ) {
                    { content }
                }
            }
        });

    ui! { builder.bind(nav) }
}
