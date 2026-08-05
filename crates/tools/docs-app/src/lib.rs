//! docs-app — a catalog-driven documentation app.
//!
//! Where idea-ui-docs hand-writes one page per component, this app reads
//! the framework's **catalog** at runtime — every `#[component]`,
//! primitive, utility, type, and bundled guide the build links — and
//! auto-generates the entire docs site from it. The sidebar groups
//! entries by kind; each detail page is generated from the catalog
//! record (docs, props/fields table, composes graph, methods,
//! animations) using the same visual language as idea-ui-docs
//! (swap-navigator + idea-ui-nav's `AppShell` + idea-ui + codeblock +
//! the `table` SDK + icons-lucide).
//!
//! ## How the catalog data flows in
//!
//! `runtime-core` is depended on with the `catalog` feature, which
//! flips on the `#[component]` emission gate across the whole dep graph
//! and pulls `mcp-catalog` in transitively. At startup `CatalogModel::build`
//! calls `mcp_catalog::ResolvedCatalog::build()` — reading the app's OWN
//! in-process `inventory` catalog (no file, no codegen step). idea-ui's
//! components show up because this crate links idea-ui and references
//! its components in the shell, which brings idea-ui's
//! `inventory::submit!` ctors into the binary (the linker-section
//! concern flagged in `crates/mcp/examples/mcp-demo/Cargo.toml` — validated by the
//! `idea_ui_components_are_present_in_runtime_catalog` unit test in
//! `catalog.rs`).
//!
//! ## Crate layout
//!
//! - `lib.rs` (this) — `app()` entry + the boot registration seams.
//! - `catalog.rs` — pure catalog → `CatalogModel` view-model mapping (unit-tested).
//! - `routes.rs` — the single catalog root route + URL-encoded entry routing.
//! - `shell.rs` — sidebar + detail-page components (`EntryPage`, `FieldsTable`, `CodePanel`, …).
//! - `styles.rs` — local chrome stylesheets (lifted from idea-ui-docs).

use std::cell::RefCell;
use std::rc::Rc;

use idea_ui_nav::AppShell;
use runtime_core::primitives::navigator::Screen;
use runtime_core::{effect, signal, ui, Breakpoint, Element, Ref, Signal};
use swap_navigator::{MountPolicy, SwapBuilder, SwapHandle, SwapNavigator};

mod catalog;
mod icons;
mod routes;
mod shell;
mod styles;
mod theme;

use catalog::CatalogModel;
use routes::{decode_entry_route, ENTRY_ROUTE, OVERVIEW_ROUTE};

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
// Boot-time SDK-handler registration. Called by the CLI-generated
// wrappers (web `start_in`/`hydrate_in`, macOS/GPU `run_with`, iOS
// `run_in_view`, Android `start`, terminal `run`, the SSG crawl) with the
// fresh scene registry, after `runtime_vocabulary::register_builtins`.
// Mirrors websites/idea-ui-docs.
// =============================================================================

/// Register this app's third-party payload handlers on a scene registry.
///
/// - `codeblock::register` so `shell::CodePanel` renders the SDK's
///   `<pre>`/span (web/SSR) or single-node native code block.
/// - `markdown::register` so guide bodies and entry docs render real
///   markdown DOM.
/// - `table::register` so `FieldsTable` / the variants +
///   animations tables render the SDK's `<table>`/`<tr>`/`<td>`.
///
/// Registration is mandatory: an unregistered payload **panics at
/// realize** (the scene contract fails loud — the pre-v2 walker rendered
/// a placeholder box instead).
///
/// Registry-generic over the scene `Host`, so ONE seam serves the web
/// boot, the SSG crawl, the native hosts, and the GPU desktop host — each
/// wrapper's call site pins `H` to its concrete backend. All three
/// handlers are caps-generic, which is why this app needs no backend
/// dependency of its own.
///
/// The navigator needs nothing here: swap navigators are vocabulary
/// built-ins installed by `register_builtins` on every host.
pub fn register_scene_extensions<H>(registry: &mut runtime_scene::Registry<H>)
where
    H: runtime_vocabulary::style_attach::StyleServices
        + runtime_vocabulary::caps::TextOps
        + runtime_vocabulary::caps::InputOps
        + 'static,
{
    codeblock::register(registry);
    markdown::register(registry);
    table::register(registry);
}

/// Recorder-side registration for the runtime-server sidecar
/// (`dev_server::sidecar::run_newcore`) — the recorder's scene-registry
/// twin of [`register_scene_extensions`].
#[cfg(feature = "sidecar")]
pub fn register_scene_extensions_recorder(registry: &mut dev_server::newcore::SceneRegistry) {
    register_scene_extensions(registry);
}

/// Android entry: the generated Android wrapper's `attach` mounts
/// `scene_app()` through `backend_android::newcore::start` (see
/// crates/tools/build/android). `app()` already returns the scene
/// `Element`, so this is a plain shim with the conventional name.
pub fn scene_app() -> Element {
    app()
}

/// Wrap a screen body in a `Screen`. The `title` used to drive the drawer
/// chrome's native header bar; the swap/outlet model has no navigator
/// chrome (the app owns its layout), so the Screen carries no options —
/// the helper is kept so screen closures still document their display
/// title next to the body they build.
fn titled(title: String, el: Element) -> Screen {
    let _ = title;
    Screen::new(el)
}

/// Root entry — the symbol every boot path mounts (the CLI-generated
/// per-platform wrappers, the SSG crawl, and the sidecar).
pub fn app() -> Element {
    // Mode signal + reactive theme install. MUST run here, in the build
    // window: signal creation outside `World::enter` panics, and the
    // sidebar's toggle only writes the signal from its handler.
    theme::init();
    // Align the framework's `Lg` breakpoint with the sidebar pin width the
    // old drawer chrome used (`install_navigator_pin_width(960.0)`), so
    // `AppShell(pin_at = Lg)` and the mobile hamburger flip at the same
    // 960-px width the drawer collapsed at. First-install wins — must run
    // before any breakpoint-keyed sheet resolves.
    let _ = runtime_core::install_breakpoints(runtime_core::Breakpoints {
        lg_min: 960.0,
        ..Default::default()
    });

    let nav: Ref<SwapHandle> = Ref::new();
    // Drawer-open state for narrow viewports — author-owned now (the
    // AppShell scrim closes it, the hamburger opens it, and the layout
    // effect below closes it after a sidebar navigation). Pinned widths
    // ignore it entirely.
    let drawer_open: Signal<bool> = signal(false);
    let cat = model();

    // The overview screen lists every kind + count. Detail screens are
    // dispatched by a single parameterized route whose params encode
    // `kind/slug`, so we don't need one `.screen(...)` per catalog entry
    // (there are hundreds, and they change as the framework grows).
    let cat_overview = cat.clone();
    let cat_entry = cat.clone();

    let builder = SwapNavigator::new(&OVERVIEW_ROUTE)
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
        // Legacy drawer-on-web behavior: one screen resident at a time;
        // switching away disposes the screen's scope and a return rebuilds
        // it fresh (matches browser semantics; screens are generated from
        // the catalog so remounting is cheap).
        .mount_policy(MountPolicy::LazyDisposing)
        // The shell: AppShell packages pinned-sidebar ⇄ drawer around the
        // one-shot outlet; the mobile hamburger bar collapses in at narrow
        // widths. The sidebar builds ONCE and survives every navigation.
        .layout(move |nav_ctx| {
            // Auto-close the drawer when a sidebar link navigates while
            // unpinned (the legacy web drawer engine did this in its
            // Select arm; author-owned now). Watch `active_path` — not
            // `active_route` — because every catalog entry shares the one
            // parameterized `entry` route name, so only the path changes
            // on entry→entry navigation.
            let active_path = nav_ctx.active_path;
            effect!({
                let _ = active_path.get();
                if !idea_ui_nav::sidebar_pinned(Breakpoint::Lg) {
                    drawer_open.set(false);
                }
            });

            let sidebar_el = shell::sidebar(active_path, model());
            let header = shell::mobile_header(drawer_open);
            // The outlet is a one-shot, non-`Clone` element — bound to a
            // local so the `ui!` child below is a bare-identifier splat.
            let outlet = nav_ctx.outlet;
            let body: Element = ui! {
                view(style = shell::outlet_grow_style) {
                    outlet
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
                    width = 300.0,
                ) {
                    content
                }
            }
        });

    ui! { builder.bind(nav) }
}
