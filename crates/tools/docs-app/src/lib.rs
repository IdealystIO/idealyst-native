//! docs-app — a catalog-driven documentation app.
//!
//! Where idea-ui-docs hand-writes one page per component, this app reads
//! the framework's **catalog** at runtime — every `#[component]`,
//! primitive, utility, type, and bundled guide the build links — and
//! auto-generates the entire docs site from it. The sidebar groups
//! entries by kind; each detail page is generated from the catalog
//! record (docs, props/fields table, composes graph, methods,
//! animations) using the same visual language as idea-ui-docs
//! (drawer-navigator + idea-ui + codeblock + the `table` SDK +
//! icons-lucide).
//!
//! ## How the catalog data flows in
//!
//! `runtime-core` is depended on with the `catalog` feature, which flips
//! on the `#[component]` emission gate across the whole dep graph and
//! pulls `mcp-catalog` in transitively. At startup `CatalogModel::build`
//! calls `mcp_catalog::ResolvedCatalog::build()` — reading the app's OWN
//! in-process `inventory` catalog (no file, no codegen step). idea-ui's
//! components show up because this crate links idea-ui and references
//! its components in the shell, which brings idea-ui's
//! `inventory::submit!` ctors into the binary (the linker-section
//! concern flagged in `examples/mcp-demo/Cargo.toml` — validated by the
//! `idea_ui_components_are_present_in_runtime_catalog` unit test in
//! `catalog.rs`).
//!
//! ## Crate layout
//!
//! - `lib.rs` (this) — `app()` entry + per-target extension registration.
//! - `catalog.rs` — pure catalog → `CatalogModel` view-model mapping (unit-tested).
//! - `routes.rs` — the single catalog root route + URL-encoded entry routing.
//! - `shell.rs` — sidebar + detail-page components (`EntryPage`, `FieldsTable`, `CodePanel`, …).
//! - `styles.rs` — local chrome stylesheets (lifted from idea-ui-docs).

use std::cell::RefCell;
use std::rc::Rc;

use drawer_navigator::{
    install_navigator_pin_width, DrawerBuilder, DrawerHandle, DrawerNavigator, DrawerScreenExt,
    HeaderStyle,
};
use idea_ui::idea_color;
use runtime_core::primitives::navigator::Screen;
use runtime_core::{component, ui, Element, Ref};

mod catalog;
mod icons;
mod routes;
mod shell;
mod styles;
mod theme;

use catalog::CatalogModel;
use routes::{decode_entry_route, ENTRY_ROUTE, OVERVIEW_ROUTE};

// `codeblock` / `markdown` are referenced directly in page code (`shell.rs`),
// but the `table` SDK is only reached transitively via idea-ui's `Table`
// re-export — pin it so its `inventory` self-registration ctors link into
// the binary now that the explicit `table::register` calls are gone.
use table as _;

thread_local! {
    /// The catalog is built once and shared by every screen closure. A
    /// thread-local `Rc` keeps the model alive across navigator screen
    /// swaps without threading it through every closure capture (the
    /// navigator's per-screen scopes drop, but this outlives them).
    static MODEL: RefCell<Option<Rc<CatalogModel>>> = const { RefCell::new(None) };
}

fn model() -> Rc<CatalogModel> {
    MODEL.with(|m| {
        if m.borrow().is_none() {
            *m.borrow_mut() = Some(Rc::new(CatalogModel::build()));
        }
        m.borrow().as_ref().unwrap().clone()
    })
}

// =============================================================================
// Per-target SDK-handler registration. Called by the CLI-generated
// wrapper before mount. Mirrors examples/idea-ui-docs.
// =============================================================================

#[cfg(target_arch = "wasm32")]
pub fn register_extensions(_backend: &mut backend_web::WebBackend) {
    backend_web::install_viewport_observer();
}

#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub fn register_extensions(_backend: &mut backend_ios::IosBackend) {}

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
pub fn register_extensions(_backend: &mut backend_android::AndroidBackend) {}

// `codeblock` / `markdown` / `table` are converted-External SDKs that now
// self-register via `inventory`, so there are no per-backend register
// calls in any of these arms. docs-app ships web/ios/android (see
// `[package.metadata.idealyst.app].targets`); the macOS/terminal arms
// exist only for the CLI-generated wrappers.
// macOS native — but NOT when the `terminal` feature is on. The terminal
// target builds for the macOS host triple, so without `not(feature =
// "terminal")` this and the terminal arm below would both compile on a
// macOS host and collide as duplicate definitions.
#[cfg(all(target_os = "macos", not(target_arch = "wasm32"), not(feature = "terminal")))]
pub fn register_extensions(_backend: &mut backend_macos::MacosBackend) {}

// Terminal — selected by the `terminal` feature (the CLI's terminal
// wrapper enables it), not a `target_os` cfg, because the terminal target
// builds for the host triple and would otherwise be shadowed by the host's
// native backend (macOS).
#[cfg(feature = "terminal")]
pub fn register_extensions(_backend: &mut backend_terminal::TerminalBackend) {}

// Recorder-side registration for the runtime-server sidecar.
#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(backend: &mut dev_server::WireRecordingBackend) {
    drawer_navigator::recording::register(backend);
}

/// Wrap a screen body with its display title for the iOS nav bar /
/// Android toolbar.
fn titled(title: String, el: Element) -> Screen {
    Screen::new(el).title(title)
}

#[component]
pub fn app() -> Element {
    theme::install_initial_theme();
    install_navigator_pin_width(960.0);

    let nav: Ref<DrawerHandle> = Ref::new();
    let cat = model();

    // The overview screen lists every kind + count. Detail screens are
    // dispatched by a single parameterized route whose params encode
    // `kind/slug`, so we don't need one `.screen(...)` per catalog entry
    // (there are hundreds, and they change as the framework grows).
    let cat_overview = cat.clone();
    let cat_entry = cat.clone();

    let builder = DrawerNavigator::new(&OVERVIEW_ROUTE)
        .header(|| HeaderStyle {
            background: Some((idea_color(|c| c.surface.clone()))()),
            title: Some((idea_color(|c| c.text.clone()))()),
            tint: Some((idea_color(|c| c.text.clone()))()),
            body_background: Some((idea_color(|c| c.background.clone()))()),
        })
        .screen(OVERVIEW_ROUTE, move |_| {
            titled("Catalog".to_string(), shell::overview_page(&cat_overview))
        })
        .screen(ENTRY_ROUTE, move |params| {
            // `params` carries the URL-encoded `kind/slug`. Resolve it
            // against the model; an unknown route falls back to a
            // "not found" page rather than panicking.
            let (kind, slug) = decode_entry_route(&params);
            let title = cat_entry
                .find(kind, &slug)
                .map(|e| e.name.clone())
                .unwrap_or_else(|| "Not found".to_string());
            titled(title, shell::entry_page(&cat_entry, kind, &slug))
        })
        .drawer_width(300.0)
        .leading_with(move |slot| shell::sidebar(slot, model()));

    ui! { builder.bind(nav) }
}
