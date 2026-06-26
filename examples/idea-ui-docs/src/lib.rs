//! idea-ui-docs — the self-referencing component reference for idea-ui.
//!
//! A single [`routes::CATALOG`] table drives everything: the grouped,
//! searchable sidebar (`leading_with`), the custom header bar with the
//! Light/Dark toggle (`top_with`), and the per-screen page frame
//! (`shell::page_frame`) that renders the group overline, title, status
//! badge, lead, body, and Usage panel. `DrawerNavigator` still owns
//! navigation + cross-platform chrome; we just drive its slots.
//!
//! ## Crate layout
//!
//! - `lib.rs` (this) — `app()` entry: theme install + DrawerNavigator wiring.
//! - `routes.rs` — the `CATALOG` (groups → entries) + route constants.
//! - `shell.rs` — header / sidebar / `page_frame` + page-template helpers.
//! - `styles.rs` — local stylesheets for chrome only.
//! - `pages/*.rs` — one module per design group; each exports body-only
//!   `pub fn name() -> Element`.

use runtime_core::primitives::navigator::Screen;
use runtime_core::{component, platform, signal, ui, Element, Platform, Ref, Signal};
use drawer_navigator::{
    install_navigator_pin_width, DrawerBuilder, DrawerHandle, DrawerNavigator, DrawerScreenExt,
    TopSlot,
};
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
    // Register the drawer navigator handler on the web backend. The
    // crate self-registers via `inventory`, but under `--local` (no
    // runtime-server) the linker can dead-strip that submission since
    // nothing else pulls in the web module's object; the explicit call
    // forces linkage + registration so `DrawerPresentation` resolves.
    drawer_navigator::register(backend);
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
    drawer_navigator::recording::register(backend);
}

#[component]
pub fn app() -> Element {
    install_idea_theme(light_theme());

    let nav: Ref<DrawerHandle> = Ref::new();
    // App-level state surviving navigation: dark-mode flag for the
    // header toggle, and the sidebar search query.
    let is_dark: Signal<bool> = signal!(false);
    let q: Signal<String> = signal!(String::new());

    // Pin the sidebar (vs. modal slide-in) at wide viewports.
    install_navigator_pin_width(900.0);

    // Chrome strategy is per-platform:
    //  - Web: own the chrome with a custom responsive header (brand +
    //    Light/Dark toggle). `TopSlot::Custom` only materializes on web,
    //    and the sidebar pins at wide widths / collapses to a modal below
    //    `install_navigator_pin_width`.
    //  - iOS/Android: use the NATIVE header (UINavigationController /
    //    Toolbar). The drawer SDK auto-injects a leading hamburger that
    //    opens the drawer, and each screen's `.title(...)` shows in the
    //    bar — so `native_header(true)` is the whole story. (`native_header
    //    (false)` would force-hide the bar AND its auto-hamburger, leaving
    //    no way to open the drawer — the bug this fixes.)
    //  - macOS: always-pinned sidebar, no header — navigate via the panel.
    let mut builder = DrawerNavigator::new(DEFAULT_ROUTE);
    builder = match platform() {
        // Desktop (web/browser AND native macOS): the custom header bar +
        // pinned sidebar. macOS now renders `TopSlot::Custom` above its
        // sidebar+outlet, matching the browser layout.
        Platform::Web | Platform::MacOs => builder
            .native_header(false)
            .top_with(TopSlot::Custom(Box::new(move |slot| shell::header(slot, is_dark)))),
        // Mobile: the native header bar (UINavigationController / Toolbar)
        // with its auto-injected drawer hamburger.
        _ => builder.native_header(true),
    };
    builder = builder.leading_with(move |slot| shell::sidebar(slot, q)).drawer_width(252.0);

    // Fold the catalog into one screen per entry. Each screen wraps the
    // entry's body in the central page frame — except the Overview
    // landing, which renders full-bleed via `landing_frame` (no title
    // block / Usage panel).
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
                Screen::new(content).title(entry.name)
            });
        }
    }

    ui! { builder.bind(nav) }
}
