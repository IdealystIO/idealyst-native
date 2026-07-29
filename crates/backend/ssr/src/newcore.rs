//! New-core adoption for the SSR backend (idea-lite migration, P5:
//! per-request worlds).
//!
//! Implements [`runtime_scene::Host`] plus **all 30** capability traits
//! (`runtime_vocabulary::caps`) directly on [`SsrBackend`] — the
//! production shape of the migration (the same choice web/macOS/iOS
//! made): no `LegacyBridge` wrapper in the render path. Every trait
//! method delegates via UFCS (`<SsrBackend as Backend>::method(self, …)`)
//! to the existing `Backend` impl, so the HTML mechanism code (node
//! building, class minting, `<head>` accumulation) is REUSED verbatim.
//! The impl bodies are generated mechanically from
//! `runtime_vocabulary::bridge` (the compile-time proof of the signature
//! freeze) with `LegacyBridge<B>` → `SsrBackend` and `&mut self.0` →
//! `self`.
//!
//! **Why direct impls, not the bridge**: `LegacyBridge<SsrBackend>`
//! would work with zero code (the corpus tests would still pass), but
//! the bridge is the compatibility shim for *unported* backends; adopted
//! backends implement the caps directly so (a) the render path has no
//! wrapper layer to unwrap when reading the accumulated HTML back out
//! (`into_html`/`head_css` sit on the same value the realize path
//! borrowed), and (b) a future SSR-specific capability override (e.g. a
//! streaming `TextOps`) lands on the concrete impl instead of forking
//! the generic bridge. There is no behavioral difference today — the
//! choice is structural, matching the house pattern.
//!
//! **30/30 direct, 0 adapted, 0 stubbed.** Where a `Backend` method is
//! not overridden by `SsrBackend` (handles, batching, wire recording,
//! introspection), the UFCS call resolves to the same trait-default the
//! old walker hits — behavior identical by construction.
//!
//! # SSR is ANCHORED — the hydration contract
//!
//! [`Host::supports_splice`] is **hard-coded `false`** (not delegated):
//! every reactive region (`Dyn`, `Keyed`) nests under a
//! `create_reactive_anchor` `<div style="display: contents">`. The web
//! backend's hydration boot (`backend-web`'s `newcore_hydrate`) adopts
//! SSR DOM cursor-style in creation order and gates its own splice
//! support on `!is_hydrating()` — it can only adopt what an anchored
//! render produced. Delegating to `Backend::supports_child_splice`
//! (currently the `false` default) would silently flip every hole to
//! anchorless if old-core SSR ever opted in, breaking adoption; the
//! override plus the regression test pins the invariant independently.
//!
//! # Per-request worlds — the multi-world payoff
//!
//! [`render_path`]/[`render_to_string`] create a **fresh
//! [`World`] per request**: enter, realize through
//! [`runtime_vocabulary::register_builtins`], flush once, serialize,
//! drop. The kernel routes handles to their OWN world, so any number of
//! requests can render on one thread — interleaved or nested — with
//! fully independent signals, effects, and theme state
//! (`runtime_vocabulary::theme` keeps the token table / cohort / version
//! signal in world context, not thread-locals). Dropping the `Realized`
//! runs every cleanup; dropping the `World` removes its TLS registry
//! entry — nothing accumulates across requests
//! (`tests/newcore_isolation.rs` pins this with weak-probe assertions).
//!
//! # Byte-identity with old-core SSR
//!
//! The same app rendered through [`crate::render_path`] (old core) and
//! [`render_path`] (this module) emits **byte-identical** `html` and
//! `head_css` — pinned by `tests/newcore_byte_identity.rs` across
//! static trees, sheet/token styling, state overlays, dyn branches,
//! keyed lists, styled-text runs, and a swap navigator with chrome.
//! This is the hydration acceptance proof for this native crate: the
//! browser adoption path already adopts old-core SSR output, so
//! byte-identical output adopts identically (the wasm hydrate fixtures
//! themselves live in backend-web).
//!
//! # What SSR deliberately does NOT install
//!
//! - **No flush driver / dispatch hook.** A request renders the
//!   committed initial state; there is no event dispatch, so the single
//!   post-realize `world.flush()` is the whole commit story.
//! - **No URL sync.** The initial path is seeded through the same
//!   `set_initial_path` slot the old render uses; the vocabulary
//!   navigator handlers `peek_initial_path()` during mount (deep links
//!   need no deferral on the new core).
//! - **Frames/timers are dropped** by the crate's queue-only scheduler
//!   (presence exit timers, repeat release deferrals): the live bundle
//!   drives animation after hydration, and the world drop frees
//!   everything a dropped timer would have released.
//!
//! # Server flows (SSG crawl + per-request serving)
//!
//! Both CLI-facing server flows have native new-core legs mirroring
//! the old entries 1:1:
//!
//! - **SSG crawl** ([`render_all`]): same hierarchy-driven loop as
//!   [`crate::render_all`], over the SAME route collector — the
//!   vocabulary navigator handlers publish their screen path patterns
//!   at mount via `record_route_paths` (the new-core twin of the old
//!   walker's `record_routes` hook), so nested navigators surface
//!   their routes when their parent screen mounts. No
//!   `reset_for_ssg_render` between pages: the per-world theme tables
//!   own registration/typeface dedup, so every fresh `World`
//!   re-registers against its fresh backend by construction.
//! - **Per-request serving** ([`serve`], feature `serve`): delegates to
//!   the same HTTP loop as the old [`crate::serve`]
//!   (`serve::serve_loop` — asset resolution, thread-per-request,
//!   panic fallback) with this module's [`render_path_with`] as the
//!   route renderer. The CLI's generated SSR wrapper selects the leg
//!   via its `new-core` feature (see `crates/tools/build/ssr`).
//!
//! # Residual seams (each named, none silent)
//!
//! - **Streaming SSR**: out of scope for both cores; the accumulated
//!   `HtmlNode` tree serializes at the end of the request.

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::accessibility::{AccessibilityProps, AccessibilityTree, LiveRegionPriority, Role};
use runtime_core::animation::AnimProp;
use runtime_core::assets::{
    AssetId, AssetSource, AssetTag, SystemFallback, TypefaceFace, TypefaceId,
};
use runtime_core::breakpoint::Breakpoint;
use runtime_core::introspect::NativeNode;
use runtime_core::primitives;
use runtime_core::primitives::portal::ViewportRect;
use runtime_core::styled_text::TextRun;
use runtime_core::{
    Action, Backend, BackendBatch, Color, ColorScheme, Easing, FileDropHandler, FontFamily,
    HoverHandler, ImageErrorHandler, ImageLoadHandler, PageMetadata, Platform, SafeAreaSides,
    Screenshot, StateBits, StyleApplication, StyleRules, TokenEntry, Tokenized, TouchHandler,
    TouchId, VirtualizerCallbacks, WheelHandler,
};
use runtime_scene::{realize, Element, Host, Registry};
use runtime_vocabulary::caps;
use runtime_world::World;

use crate::{scheduler, RenderedPage, SsrBackend, SSR_VIEWPORT};

// ===========================================================================
// Render entry points
// ===========================================================================

/// Render a new-core element tree to its body HTML string on a fresh
/// per-request world. The build closure runs inside `World::enter`, so
/// free `signal()`/`effect()`/`memo()` calls work; the world (and every
/// signal/effect the render created) is torn down before this returns.
///
/// This is the minimal per-request seam ([`render_path`] adds the
/// URL/`<head>` plumbing and returns the full [`RenderedPage`]).
pub fn render_to_string<F>(build: F) -> String
where
    F: FnOnce() -> Element,
{
    render_path("/", build).html
}

/// Render an app headlessly at a given URL path — the new-core mirror of
/// [`crate::render_path`] (same seeding: initial navigator path, the
/// [`SSR_VIEWPORT`] size), returning the same [`RenderedPage`] shape.
///
/// Per-request lifecycle: fresh backend + registry + `World`; realize
/// inside `enter`; `finish` the single root; one `flush`; serialize;
/// drop the realized tree, then the world.
pub fn render_path<F>(path: &str, build: F) -> RenderedPage
where
    F: FnOnce() -> Element,
{
    render_path_with(path, |_| {}, build)
}

/// [`render_path`] with a registration seam: `register` runs after
/// [`runtime_vocabulary::register_builtins`], so apps/SDKs can register
/// their own payload handlers on the same registry before the tree
/// realizes (the new-core analogue of the old `setup(&mut SsrBackend)`
/// hook, which existed to register navigator chrome handlers — those are
/// vocabulary built-ins now).
pub fn render_path_with<S, F>(path: &str, register: S, build: F) -> RenderedPage
where
    S: FnOnce(&mut Registry<SsrBackend>),
    F: FnOnce() -> Element,
{
    // Queue-only scheduler (shared with the old render path): microtasks
    // queue and drain below; frames/timers drop (module docs).
    scheduler::ensure_installed();
    // Seed the same slots the old render seeds. The vocabulary navigator
    // handlers peek the SAME `set_initial_path` slot; the viewport seed
    // keeps breakpoint folding + sheet closures at the identical
    // SSR-assumed size (the value the served page embeds as
    // `data-ssr-viewport` for the hydrating client). Seeding runs
    // OUTSIDE the world on purpose — it is old-core thread-level state,
    // exactly as backend-web's `newcore_hydrate` boot does it.
    runtime_core::primitives::navigator::set_initial_path(Some(path.to_string()));
    runtime_core::set_viewport_size(SSR_VIEWPORT);

    let backend = Rc::new(RefCell::new(SsrBackend::new()));
    let mut registry: Registry<SsrBackend> = Registry::new();
    runtime_vocabulary::register_builtins(&mut registry);
    register(&mut registry);
    let registry = Rc::new(registry);

    // THE per-request world. `World::new` registers it in the thread's
    // world table; the drop at the end of this function removes it —
    // N requests on one thread never accumulate reactive state.
    let world = World::new();
    let realized = world.enter(|| {
        let element = build();
        realize(&backend, &registry, element)
    });

    // Single-root contract, matching the old-core mount and the web
    // new-core boot: `finish` roots the HTML serialization.
    let mut roots = realized.collect_nodes();
    let root = match roots.len() {
        1 => roots.pop().expect("len checked"),
        n => panic!(
            "backend_ssr::newcore::render_path: the app root must contribute exactly one \
             top-level node (got {n}) — wrap fragment/multi-root trees in a view"
        ),
    };
    Backend::finish(&mut *backend.borrow_mut(), root);

    // Commit anything staged during mount (write-backs, driver-effect
    // state) — the request's one and only flush.
    world.flush();

    // Clear in case the tree had no navigator to consume it (the root
    // navigator clears it itself — old walker contract, ported).
    runtime_core::primitives::navigator::set_initial_path(None);
    // Run any queued microtasks with no backend borrow held (parity with
    // the old render path's post-mount drain).
    scheduler::drain();

    let page = {
        let b = backend.borrow();
        RenderedPage {
            html: b.into_html(),
            metadata: b.metadata.clone(),
            head_css: b.head_css(),
        }
    };

    // Teardown order matters: the realized tree unmounts (cleanups fire)
    // BEFORE the world — its slots' owner — dies. Then the world's drop
    // removes it from the thread's registry: no TLS accumulation.
    drop(realized);
    drop(world);
    page
}

/// Crawl every route reachable from the app's navigator hierarchy and
/// render each as an SSG'd page — the new-core leg of
/// [`crate::render_all`], driving `idealyst build --ssg` for new-core
/// apps.
///
/// Identical crawl contract: the route collector
/// (`runtime_core::primitives::navigator`, shared by both cores) is
/// enabled before each render; the vocabulary navigator handlers
/// publish every mounting navigator's `NavScreenEntry.path` set; the
/// loop drains discovered literal paths and queues the unrendered
/// ones, so nested navigators fall out of the same loop. Routes with
/// `:placeholder` segments are returned in
/// [`skipped_parameterized`](crate::CrawlResult::skipped_parameterized).
///
/// Unlike the old leg there is NO per-page `reset_for_ssg_render`:
/// registration/typeface dedup lives in the per-world theme context,
/// and each page renders on a fresh [`World`] (module docs).
pub fn render_all<S, F>(register: S, app: F) -> crate::CrawlResult
where
    S: Fn(&mut Registry<SsrBackend>),
    F: Fn() -> Element,
{
    use runtime_core::primitives::navigator::{enable_route_collector, take_route_collector};
    use std::collections::{HashMap, HashSet, VecDeque};

    let mut pages: HashMap<String, RenderedPage> = HashMap::new();
    let mut skipped: Vec<&'static str> = Vec::new();
    let mut queue: VecDeque<String> = VecDeque::from(["/".to_string()]);
    let mut seen: HashSet<String> = HashSet::new();
    seen.insert("/".to_string());

    while let Some(path) = queue.pop_front() {
        enable_route_collector();
        let page = render_path_with(&path, |r| register(r), || app());
        let discovered = take_route_collector().unwrap_or_default();
        pages.insert(path, page);

        for p in discovered {
            if p.contains(':') {
                if !skipped.contains(&p) {
                    skipped.push(p);
                }
                continue;
            }
            let ps = p.to_string();
            if seen.insert(ps.clone()) {
                queue.push_back(ps);
            }
        }
    }

    crate::CrawlResult { pages, skipped_parameterized: skipped }
}

/// Serve a new-core `app` over HTTP at `addr` — the new-core leg of
/// [`crate::serve`] (feature `serve`), sharing its HTTP loop
/// (`serve_loop`: static assets under `static_dir`, thread-per-request
/// render, panic → 500 fallback) with this module's
/// [`render_path_with`] as the per-request renderer. `register` is the
/// scene-registry seam ([`render_path_with`]'s `register`, cloned per
/// request).
#[cfg(feature = "serve")]
pub fn serve<A, R>(
    addr: &str,
    config: crate::ServeConfig,
    register: R,
    app: A,
) -> std::io::Result<()>
where
    A: Fn() -> Element + Send + Sync + Clone + 'static,
    R: Fn(&mut Registry<SsrBackend>) + Send + Sync + Clone + 'static,
{
    let bundle = config.bundle_module.clone();
    let extra_head = config.extra_head.clone();
    crate::serve::serve_loop(addr, config, move |path| {
        let page = render_path_with(path, |r| register(r), || app());
        crate::render_document(&page, bundle.as_deref(), extra_head.as_deref())
    })
}

// ===========================================================================
// Host — the structural seam
// ===========================================================================

impl Host for SsrBackend {
    type Node = <SsrBackend as Backend>::Node;

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        Backend::insert(self, parent, child)
    }

    fn insert_many(&mut self, parent: &mut Self::Node, children: Vec<Self::Node>) {
        Backend::insert_many(self, parent, children)
    }

    fn insert_at(&mut self, parent: &mut Self::Node, child: Self::Node, index: usize) {
        Backend::insert_at(self, parent, child, index)
    }

    fn remove_child(&mut self, parent: &Self::Node, child: &Self::Node) {
        Backend::remove_child(self, parent, child)
    }

    fn clear_children(&mut self, node: &Self::Node) {
        Backend::clear_children(self, node)
    }

    fn create_anchor(&mut self) -> Self::Node {
        Backend::create_reactive_anchor(self)
    }

    /// Hard `false`, NOT delegated to `supports_child_splice`: SSR must
    /// stay anchored regardless of the old-core default, because the web
    /// hydration boot adopts the anchors this render emits (module docs,
    /// "SSR is ANCHORED"). Regression-pinned by
    /// `newcore_host_is_anchored`.
    fn supports_splice(&self) -> bool {
        false
    }
}

// ===========================================================================
// App environment + lifecycle
// ===========================================================================

impl caps::AppEnvOps for SsrBackend {
    fn color_scheme(&self) -> ColorScheme {
        Backend::color_scheme(self)
    }

    fn platform(&self) -> Platform {
        Backend::platform(self)
    }

    fn url_opener(&self) -> Option<Rc<dyn Fn(&str)>> {
        Backend::url_opener(self)
    }

    fn fullscreen_setter(&self) -> Option<Rc<dyn Fn(bool)>> {
        Backend::fullscreen_setter(self)
    }

    fn set_page_metadata(&mut self, meta: &PageMetadata) {
        Backend::set_page_metadata(self, meta)
    }

    fn set_app_background(&mut self, color: &Tokenized<Color>) {
        Backend::set_app_background(self, color)
    }

    fn set_scrollbar_theme(&mut self, thumb: &Tokenized<Color>, track: &Tokenized<Color>) {
        Backend::set_scrollbar_theme(self, thumb, track)
    }

    fn set_app_key_handler(&mut self, handler: Option<primitives::key::KeyDownHandler>) {
        Backend::set_app_key_handler(self, handler)
    }
}

impl caps::LifecycleOps for SsrBackend {
    fn finish(&mut self, root: Self::Node) {
        Backend::finish(self, root)
    }

    fn run_layout(&mut self) {
        Backend::run_layout(self)
    }

    fn schedule_layout_pass() {
        <SsrBackend as Backend>::schedule_layout_pass()
    }

    fn is_hydrating(&self) -> bool {
        Backend::is_hydrating(self)
    }

    fn renders_lazy_chunks(&self) -> bool {
        Backend::renders_lazy_chunks(self)
    }
}

// ===========================================================================
// View + input + pressable
// ===========================================================================

impl caps::ViewOps for SsrBackend {
    fn create_view(&mut self, a11y: &AccessibilityProps) -> Self::Node {
        Backend::create_view(self, a11y)
    }

    fn make_view_handle(&self, node: &Self::Node) -> runtime_core::ViewHandle {
        Backend::make_view_handle(self, node)
    }
}

impl caps::InputOps for SsrBackend {
    fn install_touch_handler(&mut self, node: &Self::Node, handler: TouchHandler) {
        Backend::install_touch_handler(self, node, handler)
    }

    fn claim_touch(&mut self, node: &Self::Node, touch_id: TouchId) {
        Backend::claim_touch(self, node, touch_id)
    }

    fn install_wheel_handler(&mut self, node: &Self::Node, handler: WheelHandler) {
        Backend::install_wheel_handler(self, node, handler)
    }

    fn install_hover_handler(&mut self, node: &Self::Node, handler: HoverHandler) {
        Backend::install_hover_handler(self, node, handler)
    }

    fn mark_preserves_focus(&mut self, node: &Self::Node) {
        Backend::mark_preserves_focus(self, node)
    }

    fn install_file_drop_handler(&mut self, node: &Self::Node, handler: FileDropHandler) {
        Backend::install_file_drop_handler(self, node, handler)
    }
}

impl caps::PressableOps for SsrBackend {
    fn create_pressable(&mut self, on_click: Rc<dyn Fn()>, a11y: &AccessibilityProps) -> Self::Node {
        Backend::create_pressable(self, on_click, a11y)
    }

    fn make_pressable_handle(&self, node: &Self::Node) -> runtime_core::PressableHandle {
        Backend::make_pressable_handle(self, node)
    }
}

// ===========================================================================
// Text + button
// ===========================================================================

impl caps::TextOps for SsrBackend {
    fn create_text(&mut self, content: &str, a11y: &AccessibilityProps) -> Self::Node {
        Backend::create_text(self, content, a11y)
    }

    fn create_styled_text(&mut self, runs: &[TextRun], a11y: &AccessibilityProps) -> Self::Node {
        Backend::create_styled_text(self, runs, a11y)
    }

    fn update_styled_text(&mut self, node: &Self::Node, runs: &[TextRun]) {
        Backend::update_styled_text(self, node, runs)
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        Backend::update_text(self, node, content)
    }

    fn create_text_with_id(
        &mut self,
        content: &str,
        a11y: &AccessibilityProps,
    ) -> Option<(Self::Node, u32)> {
        Backend::create_text_with_id(self, content, a11y)
    }

    fn update_text_by_id(&mut self, id: u32, content: String) {
        Backend::update_text_by_id(self, id, content)
    }

    fn release_text_id(&mut self, id: u32) {
        Backend::release_text_id(self, id)
    }

    fn supports_js_text_bindings(&self) -> bool {
        Backend::supports_js_text_bindings(self)
    }

    fn register_reactive_text_binding(
        &mut self,
        text_id: u32,
        signal_ids: &[u64],
        template_parts: &[&str],
        initial_values: &[&str],
        stringifiers: &[Rc<dyn Fn() -> String>],
    ) {
        Backend::register_reactive_text_binding(
            self,
            text_id,
            signal_ids,
            template_parts,
            initial_values,
            stringifiers,
        )
    }

    fn release_reactive_text_binding(&mut self, text_id: u32) {
        Backend::release_reactive_text_binding(self, text_id)
    }

    fn make_text_handle(&self, node: &Self::Node) -> runtime_core::TextHandle {
        Backend::make_text_handle(self, node)
    }
}

impl caps::ButtonOps for SsrBackend {
    fn create_button(
        &mut self,
        label: &str,
        on_click: &Action,
        leading_icon: Option<&primitives::icon::IconData>,
        trailing_icon: Option<&primitives::icon::IconData>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        Backend::create_button(self, label, on_click, leading_icon, trailing_icon, a11y)
    }

    fn update_button_label(&mut self, node: &Self::Node, label: &str) {
        Backend::update_button_label(self, node, label)
    }

    fn make_button_handle(&self, node: &Self::Node) -> runtime_core::ButtonHandle {
        Backend::make_button_handle(self, node)
    }
}

// ===========================================================================
// Image + icon + link
// ===========================================================================

impl caps::ImageOps for SsrBackend {
    fn create_image(&mut self, src: &str, alt: Option<&str>, a11y: &AccessibilityProps) -> Self::Node {
        Backend::create_image(self, src, alt, a11y)
    }

    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        Backend::update_image_src(self, node, src)
    }

    fn update_image_alt(&mut self, node: &Self::Node, alt: Option<&str>) {
        Backend::update_image_alt(self, node, alt)
    }

    fn install_image_load_handler(&mut self, node: &Self::Node, handler: ImageLoadHandler) {
        Backend::install_image_load_handler(self, node, handler)
    }

    fn install_image_error_handler(&mut self, node: &Self::Node, handler: ImageErrorHandler) {
        Backend::install_image_error_handler(self, node, handler)
    }

    fn make_image_handle(&self, node: &Self::Node) -> primitives::image::ImageHandle {
        Backend::make_image_handle(self, node)
    }
}

impl caps::IconOps for SsrBackend {
    fn create_icon(
        &mut self,
        data: &primitives::icon::IconData,
        color: Option<&Color>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        Backend::create_icon(self, data, color, a11y)
    }

    fn update_icon_color(&mut self, node: &Self::Node, color: &Color) {
        Backend::update_icon_color(self, node, color)
    }

    fn update_icon_data(&mut self, node: &Self::Node, data: &primitives::icon::IconData) {
        Backend::update_icon_data(self, node, data)
    }

    fn update_icon_stroke(&mut self, node: &Self::Node, progress: f32) {
        Backend::update_icon_stroke(self, node, progress)
    }

    fn animate_icon_stroke(
        &mut self,
        node: &Self::Node,
        from: f32,
        to: f32,
        duration_ms: u32,
        easing: Easing,
        infinite: bool,
        autoreverses: bool,
    ) {
        Backend::animate_icon_stroke(self, node, from, to, duration_ms, easing, infinite, autoreverses)
    }

    fn make_icon_handle(&self, node: &Self::Node) -> primitives::icon::IconHandle {
        Backend::make_icon_handle(self, node)
    }
}

impl caps::LinkOps for SsrBackend {
    fn create_link(
        &mut self,
        config: primitives::link::LinkConfig,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        Backend::create_link(self, config, a11y)
    }

    fn update_link_url(&mut self, node: &Self::Node, url: &str) {
        Backend::update_link_url(self, node, url)
    }

    fn make_link_handle(&self, node: &Self::Node) -> primitives::link::LinkHandle {
        Backend::make_link_handle(self, node)
    }
}

// ===========================================================================
// Form widgets
// ===========================================================================

impl caps::TextInputOps for SsrBackend {
    fn create_text_input(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<primitives::key::KeyDownHandler>,
        on_blur: Option<primitives::text_input::BlurHandler>,
        secure: bool,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        Backend::create_text_input(
            self,
            initial_value,
            placeholder,
            on_change,
            on_key_down,
            on_blur,
            secure,
            a11y,
        )
    }

    fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {
        Backend::update_text_input_value(self, node, value)
    }

    fn update_text_input_secure(&mut self, node: &Self::Node, secure: bool) {
        Backend::update_text_input_secure(self, node, secure)
    }

    fn set_text_input_focus_handler(&mut self, node: &Self::Node, handler: Rc<dyn Fn(bool)>) {
        Backend::set_text_input_focus_handler(self, node, handler)
    }

    fn update_text_input_placeholder(&mut self, node: &Self::Node, placeholder: Option<&str>) {
        Backend::update_text_input_placeholder(self, node, placeholder)
    }

    fn create_text_area(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        wrap: bool,
        min_rows: Option<u32>,
        max_rows: Option<u32>,
        on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<primitives::key::KeyDownHandler>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        Backend::create_text_area(
            self,
            initial_value,
            placeholder,
            wrap,
            min_rows,
            max_rows,
            on_change,
            on_key_down,
            a11y,
        )
    }

    fn update_text_area_value(&mut self, node: &Self::Node, value: &str) {
        Backend::update_text_area_value(self, node, value)
    }

    fn make_text_input_handle(&self, node: &Self::Node) -> primitives::text_input::TextInputHandle {
        Backend::make_text_input_handle(self, node)
    }

    fn make_text_area_handle(&self, node: &Self::Node) -> primitives::text_area::TextAreaHandle {
        Backend::make_text_area_handle(self, node)
    }
}

impl caps::ToggleOps for SsrBackend {
    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        Backend::create_toggle(self, initial_value, on_change, a11y)
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        Backend::update_toggle_value(self, node, value)
    }

    fn make_toggle_handle(&self, node: &Self::Node) -> primitives::toggle::ToggleHandle {
        Backend::make_toggle_handle(self, node)
    }
}

impl caps::SliderOps for SsrBackend {
    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        Backend::create_slider(self, initial_value, min, max, step, on_change, a11y)
    }

    fn update_slider_value(&mut self, node: &Self::Node, value: f32) {
        Backend::update_slider_value(self, node, value)
    }

    fn make_slider_handle(&self, node: &Self::Node) -> primitives::slider::SliderHandle {
        Backend::make_slider_handle(self, node)
    }
}

impl caps::ActivityIndicatorOps for SsrBackend {
    fn create_activity_indicator(
        &mut self,
        size: primitives::activity_indicator::ActivityIndicatorSize,
        color: Option<&Color>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        Backend::create_activity_indicator(self, size, color, a11y)
    }

    fn update_activity_indicator_size(
        &mut self,
        node: &Self::Node,
        size: primitives::activity_indicator::ActivityIndicatorSize,
    ) {
        Backend::update_activity_indicator_size(self, node, size)
    }

    fn make_activity_indicator_handle(
        &self,
        node: &Self::Node,
    ) -> primitives::activity_indicator::ActivityIndicatorHandle {
        Backend::make_activity_indicator_handle(self, node)
    }
}

// ===========================================================================
// Scroll + safe area + virtualizer
// ===========================================================================

impl caps::ScrollOps for SsrBackend {
    fn create_scroll_view(
        &mut self,
        horizontal: bool,
        on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        Backend::create_scroll_view(self, horizontal, on_scroll, a11y)
    }

    fn node_scroll(&self, node: &Self::Node) -> (f32, f32) {
        Backend::node_scroll(self, node)
    }

    fn set_node_scroll(&mut self, node: &Self::Node, x: f32, y: f32) {
        Backend::set_node_scroll(self, node, x, y)
    }

    fn make_scroll_view_handle(&self, node: &Self::Node) -> primitives::scroll_view::ScrollViewHandle {
        Backend::make_scroll_view_handle(self, node)
    }
}

impl caps::SafeAreaOps for SsrBackend {
    fn apply_safe_area_padding(&mut self, node: &Self::Node, sides: SafeAreaSides) {
        Backend::apply_safe_area_padding(self, node, sides)
    }

    fn apply_scroll_view_safe_area_inset(&mut self, node: &Self::Node, sides: SafeAreaSides) {
        Backend::apply_scroll_view_safe_area_inset(self, node, sides)
    }
}

impl caps::VirtualizerOps for SsrBackend {
    fn create_virtualizer(
        &mut self,
        callbacks: VirtualizerCallbacks<Self::Node>,
        overscan: f32,
        layout: primitives::virtualizer::VirtualLayout,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        Backend::create_virtualizer(self, callbacks, overscan, layout, a11y)
    }

    fn virtualizer_data_changed(&mut self, node: &Self::Node) {
        Backend::virtualizer_data_changed(self, node)
    }

    fn release_virtualizer(&mut self, node: &Self::Node) {
        Backend::release_virtualizer(self, node)
    }

    fn make_virtualizer_handle(&self, node: &Self::Node) -> primitives::virtualizer::VirtualizerHandle {
        Backend::make_virtualizer_handle(self, node)
    }
}

// ===========================================================================
// Graphics + portal + presence + navigator
// ===========================================================================

impl caps::GraphicsOps for SsrBackend {
    fn create_graphics(
        &mut self,
        on_ready: primitives::graphics::OnReady,
        on_resize: primitives::graphics::OnResize,
        on_lost: primitives::graphics::OnLost,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        Backend::create_graphics(self, on_ready, on_resize, on_lost, a11y)
    }

    fn release_graphics(&mut self, node: &Self::Node) {
        Backend::release_graphics(self, node)
    }

    fn make_graphics_handle(&self, node: &Self::Node) -> primitives::graphics::GraphicsHandle {
        Backend::make_graphics_handle(self, node)
    }
}

impl caps::PortalOps for SsrBackend {
    fn create_portal(
        &mut self,
        target: primitives::portal::PortalTarget,
        on_dismiss: Option<Rc<dyn Fn()>>,
        trap_focus: bool,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        Backend::create_portal(self, target, on_dismiss, trap_focus, a11y)
    }

    fn release_portal(&mut self, node: &Self::Node) {
        Backend::release_portal(self, node)
    }

    fn set_portal_hidden(&mut self, node: &Self::Node, hidden: bool) {
        Backend::set_portal_hidden(self, node, hidden)
    }

    fn make_portal_handle(&self, node: &Self::Node) -> primitives::portal::PortalHandle {
        Backend::make_portal_handle(self, node)
    }
}

impl caps::PresenceOps for SsrBackend {
    fn create_presence_placeholder(&mut self, a11y: &AccessibilityProps) -> Self::Node {
        Backend::create_presence_placeholder(self, a11y)
    }

    fn apply_presence(
        &mut self,
        node: &Self::Node,
        state: primitives::presence::PresenceState,
        transition: Option<(u32, Easing)>,
    ) {
        Backend::apply_presence(self, node, state, transition)
    }

    fn make_presence_handle(&self, node: &Self::Node) -> primitives::presence::PresenceHandle {
        Backend::make_presence_handle(self, node)
    }
}

impl caps::NavigatorOps for SsrBackend {
    fn create_navigator(
        &mut self,
        type_id: TypeId,
        type_name: &'static str,
        presentation: Rc<dyn Any>,
        host: primitives::navigator::NavigatorHost<Self::Node>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        Backend::create_navigator(self, type_id, type_name, presentation, host, a11y)
    }

    fn release_navigator(&mut self, node: &Self::Node) {
        Backend::release_navigator(self, node)
    }

    fn apply_navigator_slot_style(
        &mut self,
        node: &Self::Node,
        slot: &'static str,
        style: &Rc<StyleRules>,
    ) {
        Backend::apply_navigator_slot_style(self, node, slot, style)
    }

    fn make_navigator_handle(&self, node: &Self::Node) -> primitives::navigator::NavigatorHandle {
        Backend::make_navigator_handle(self, node)
    }

    fn navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: Box<dyn Any>,
    ) {
        Backend::navigator_attach_initial(self, navigator, screen, scope_id, options)
    }
}

// ===========================================================================
// External + document
// ===========================================================================

impl caps::ExternalOps for SsrBackend {
    fn create_external(
        &mut self,
        type_id: TypeId,
        type_name: &'static str,
        payload: &Rc<dyn Any>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        Backend::create_external(self, type_id, type_name, payload, a11y)
    }

    fn release_external(&mut self, node: &Self::Node) {
        Backend::release_external(self, node)
    }

    fn missing_primitive_placeholder(&mut self, label: &'static str) -> Self::Node {
        Backend::missing_primitive_placeholder(self, label)
    }
}

impl caps::DocumentOps for SsrBackend {
    fn create_element(&mut self, tag: &str) -> Self::Node {
        Backend::create_element(self, tag)
    }

    fn attach_html_id(&self, node: &Self::Node, id: &str) {
        Backend::attach_html_id(self, node, id)
    }

    fn attach_html_class(&self, node: &Self::Node, class: &str) {
        Backend::attach_html_class(self, node, class)
    }

    fn attach_html_style(&self, node: &Self::Node, prop: &str, value: &str) {
        Backend::attach_html_style(self, node, prop, value)
    }

    fn register_raw_css(&mut self, css: &str) {
        Backend::register_raw_css(self, css)
    }
}

// ===========================================================================
// Style + assets
// ===========================================================================

impl caps::StyleOps for SsrBackend {
    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        Backend::apply_style(self, node, style)
    }

    fn mint_style_class(&mut self, style: &Rc<StyleRules>) -> Option<String> {
        Backend::mint_style_class(self, style)
    }

    fn mint_class_for_app(&mut self, app: &StyleApplication) -> Option<String> {
        Backend::mint_class_for_app(self, app)
    }

    fn apply_styled_states(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        overlays: &[(StateBits, Rc<StyleRules>)],
    ) {
        Backend::apply_styled_states(self, node, base, overlays)
    }

    fn apply_styled_variants(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        state_overlays: &[(StateBits, Rc<StyleRules>)],
        breakpoint_overlays: &[(Breakpoint, Rc<StyleRules>)],
        container_overlays: &[(f32, Rc<StyleRules>)],
    ) {
        Backend::apply_styled_variants(
            self,
            node,
            base,
            state_overlays,
            breakpoint_overlays,
            container_overlays,
        )
    }

    fn mark_container(&mut self, node: &Self::Node) {
        Backend::mark_container(self, node)
    }

    fn handles_states_natively(&self) -> bool {
        Backend::handles_states_natively(self)
    }

    fn token_updates_propagate_via_cascade(&self) -> bool {
        Backend::token_updates_propagate_via_cascade(self)
    }

    fn register_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        Backend::register_stylesheet(self, rules)
    }

    fn unregister_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        Backend::unregister_stylesheet(self, rules)
    }

    fn install_tokens(&mut self, tokens: &[TokenEntry]) {
        Backend::install_tokens(self, tokens)
    }

    fn update_tokens(&mut self, tokens: &[TokenEntry]) {
        Backend::update_tokens(self, tokens)
    }

    fn on_node_unstyled(&mut self, node: &Self::Node) {
        Backend::on_node_unstyled(self, node)
    }

    fn attach_states(&mut self, node: &Self::Node, setter: Rc<dyn Fn(StateBits, bool)>) {
        Backend::attach_states(self, node, setter)
    }

    fn set_disabled(&mut self, node: &Self::Node, disabled: bool) {
        Backend::set_disabled(self, node, disabled)
    }

    fn supports_preminted_styles(&self) -> bool {
        Backend::supports_preminted_styles(self)
    }

    fn apply_default_text_font(&mut self, font: Option<&FontFamily>) {
        Backend::apply_default_text_font(self, font)
    }

    fn supports_js_class_bindings(&self) -> bool {
        Backend::supports_js_class_bindings(self)
    }

    fn register_reactive_class_binding(
        &mut self,
        node: &Self::Node,
        signal_id: u64,
        values: &[u32],
        classes: &[&str],
        value_reader: Rc<dyn Fn() -> u32>,
    ) -> u32 {
        Backend::register_reactive_class_binding(self, node, signal_id, values, classes, value_reader)
    }

    fn release_reactive_class_binding(&mut self, binding_id: u32) {
        Backend::release_reactive_class_binding(self, binding_id)
    }
}

impl caps::AssetOps for SsrBackend {
    fn register_asset(&mut self, id: AssetId, kind: AssetTag, source: &AssetSource) {
        Backend::register_asset(self, id, kind, source)
    }

    fn unregister_asset(&mut self, id: AssetId, kind: AssetTag) {
        Backend::unregister_asset(self, id, kind)
    }

    fn register_typeface(
        &mut self,
        id: TypefaceId,
        family_name: &str,
        faces: &[TypefaceFace],
        fallback: SystemFallback,
    ) {
        Backend::register_typeface(self, id, family_name, faces, fallback)
    }

    fn unregister_typeface(&mut self, id: TypefaceId) {
        Backend::unregister_typeface(self, id)
    }
}

// ===========================================================================
// A11y + animation + introspection
// ===========================================================================

impl caps::A11yOps for SsrBackend {
    fn update_accessibility(
        &mut self,
        node: &Self::Node,
        a11y: &AccessibilityProps,
        inferred_role: Option<Role>,
    ) {
        Backend::update_accessibility(self, node, a11y, inferred_role)
    }

    fn announce_for_accessibility(&mut self, msg: &str, priority: LiveRegionPriority) {
        Backend::announce_for_accessibility(self, msg, priority)
    }

    fn dump_accessibility_tree(&self) -> Option<AccessibilityTree> {
        Backend::dump_accessibility_tree(self)
    }
}

impl caps::AnimationOps for SsrBackend {
    fn set_animated_f32(&mut self, node: &Self::Node, prop: AnimProp, value: f32) {
        Backend::set_animated_f32(self, node, prop, value)
    }

    fn set_animated_color(&mut self, node: &Self::Node, prop: AnimProp, value: [f32; 4]) {
        Backend::set_animated_color(self, node, prop, value)
    }
}

impl caps::IntrospectionOps for SsrBackend {
    fn frame(&self, node: &Self::Node) -> Option<ViewportRect> {
        Backend::frame(self, node)
    }

    fn absolute_frame(&self, node: &Self::Node) -> Option<ViewportRect> {
        Backend::absolute_frame(self, node)
    }

    fn device_frame(&self, node: &Self::Node) -> Option<ViewportRect> {
        Backend::device_frame(self, node)
    }

    fn supports_native_introspection(&self) -> bool {
        Backend::supports_native_introspection(self)
    }

    fn introspect_native(&self, node: &Self::Node) -> Option<NativeNode> {
        Backend::introspect_native(self, node)
    }

    fn note_introspection_root(&self, node: &Self::Node) {
        Backend::note_introspection_root(self, node)
    }

    fn supports_screenshot(&self) -> bool {
        Backend::supports_screenshot(self)
    }

    fn capture_screenshot(&self, done: Box<dyn FnOnce(Result<Screenshot, String>)>) {
        Backend::capture_screenshot(self, done)
    }
}

// ===========================================================================
// Batch + wire bindings
// ===========================================================================

impl caps::BatchOps for SsrBackend {
    fn supports_batched_repeat(&self) -> bool {
        Backend::supports_batched_repeat(self)
    }

    fn execute_batch(&mut self, batch: BackendBatch) -> Vec<Self::Node> {
        Backend::execute_batch(self, batch)
    }

    fn execute_batch_with_attach(
        &mut self,
        batch: BackendBatch,
        parent: &mut Self::Node,
        attach_locals: &[u32],
    ) -> Vec<Self::Node> {
        Backend::execute_batch_with_attach(self, batch, parent, attach_locals)
    }
}

impl caps::WireBindingOps for SsrBackend {
    fn note_text_binding(&mut self, node: &Self::Node, signal_ids: &[u64], method: &'static str) {
        Backend::note_text_binding(self, node, signal_ids, method)
    }

    fn note_signal_initial(&mut self, signal_id: u64, value: &runtime_core::__serde_json::Value) {
        Backend::note_signal_initial(self, signal_id, value)
    }

    fn note_when_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        cond_method: &'static str,
        then_node: &Self::Node,
        otherwise_node: &Self::Node,
    ) {
        Backend::note_when_binding(self, anchor, signal_ids, cond_method, then_node, otherwise_node)
    }

    fn note_switch_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        cond_method: &'static str,
        arms: &[(runtime_core::__serde_json::Value, Self::Node)],
        default_node: &Self::Node,
    ) {
        Backend::note_switch_binding(self, anchor, signal_ids, cond_method, arms, default_node)
    }

    fn note_repeat_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        count_method: &'static str,
        row_template: &Self::Node,
        row_index_signal_id: Option<u64>,
    ) {
        Backend::note_repeat_binding(
            self,
            anchor,
            signal_ids,
            count_method,
            row_template,
            row_index_signal_id,
        )
    }

    fn note_virtualizer_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        count_method: &'static str,
        row_template: &Self::Node,
        row_index_signal_id: Option<u64>,
        horizontal: bool,
    ) {
        Backend::note_virtualizer_binding(
            self,
            anchor,
            signal_ids,
            count_method,
            row_template,
            row_index_signal_id,
            horizontal,
        )
    }

    fn supports_lazy_slot_capture(&self) -> bool {
        Backend::supports_lazy_slot_capture(self)
    }

    fn begin_slot_capture(&mut self) {
        Backend::begin_slot_capture(self)
    }

    fn end_slot_capture(&mut self, slot_root: &Self::Node) {
        Backend::end_slot_capture(self, slot_root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// REGRESSION: SSR's `Host::supports_splice` must be `false`
    /// independently of the `Backend` default — an anchored render is
    /// what the web hydration boot adopts (module docs). If either half
    /// flips, hydration adoption of SSR output breaks silently.
    #[test]
    fn newcore_host_is_anchored() {
        let b = SsrBackend::new();
        assert!(
            !Host::supports_splice(&b),
            "SSR Host must be anchored (hydration adopts the anchors)"
        );
        assert!(
            !Backend::supports_child_splice(&b),
            "old-core SSR is anchored too — if this ever changes, the \
             hydration adoption contract must be revisited for BOTH cores"
        );
    }
}
