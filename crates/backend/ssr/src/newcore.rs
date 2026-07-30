//! SSR rendering: the `runtime_scene::Host` + capability-trait surface
//! and the per-request-world render entries.
//!
//! [`SsrBackend`] implements [`runtime_scene::Host`] plus **all 30**
//! capability traits (`runtime_vocabulary::caps`) — the production shape
//! of the migration (the same choice web/macOS/iOS made). Every method
//! body in this file IS the SSR mechanism: it was moved here verbatim
//! from the crate's old `impl runtime_core::Backend for SsrBackend` when
//! the 159-method mega-trait was deleted, so the HTML mechanism (node
//! building, class minting, `<head>` accumulation) is unchanged.
//! Capabilities SSR does not implement are simply absent — the
//! caps-trait DEFAULT bodies serve them, and those defaults were audited
//! byte-for-byte against the `Backend` defaults they replace
//! (`docs/runtime-v2-deletion-baseline.md` S2.1; 102 of SSR's 152 caps
//! methods resolve to a default).
//!
//! **30/30 traits implemented, 0 adapted, 0 stubbed.**
//!
//! # SSR is ANCHORED — the hydration contract
//!
//! [`Host::supports_splice`] is **hard-coded `false`**: every reactive
//! region (`Dyn`, `Keyed`) nests under a [`Host::create_anchor`]
//! `<div style="display: contents">`. The web backend's hydration boot
//! (`backend-web`'s `newcore_hydrate`) adopts SSR DOM cursor-style in
//! creation order and gates its own splice support on
//! `!is_hydrating()` — it can only adopt what an anchored render
//! produced. The value is also what the old `Backend` trait default
//! supplied, so the emitted bytes are unchanged; `Host` makes it
//! REQUIRED, so it is now stated here instead of inherited
//! (deletion-baseline S2.2), and `newcore_host_is_anchored` pins it.
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

use std::cell::RefCell;
use std::rc::Rc;

use runtime_scene::{realize, Element, Host, Registry};
use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::StyleRules;
use runtime_vocabulary::caps;
use runtime_world::World;

use crate::{
    add_class, add_inline_style, nref, remove_attr, scheduler, set_attr,
    set_styled_class, HtmlNode, NodeRef, RenderedPage, SsrBackend, SSR_VIEWPORT,
};

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
    runtime_shared::primitives::navigator::set_initial_path(Some(path.to_string()));
    runtime_shared::set_viewport_size(SSR_VIEWPORT);

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
    caps::LifecycleOps::finish(&mut *backend.borrow_mut(), root);

    // Commit anything staged during mount (write-backs, driver-effect
    // state) — the request's one and only flush.
    world.flush();

    // Clear in case the tree had no navigator to consume it (the root
    // navigator clears it itself — old walker contract, ported).
    runtime_shared::primitives::navigator::set_initial_path(None);
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
/// (`runtime_shared::primitives::navigator`, shared by both cores) is
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
    use runtime_shared::primitives::navigator::{enable_route_collector, take_route_collector};
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
    type Node = NodeRef;

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        parent.borrow_mut().children.push(child);
    }

    // `insert_many` is deliberately NOT implemented: `Host`'s default is
    // the same N-x-`insert` loop the old `Backend` default ran, so the
    // emitted child order is unchanged (deletion-baseline S2.2 —
    // "byte-identical on `Host`, safe").

    fn insert_at(&mut self, parent: &mut Self::Node, child: Self::Node, index: usize) {
        let mut p = parent.borrow_mut();
        let index = index.min(p.children.len());
        p.children.insert(index, child);
    }

    /// Explicit port of the old `Backend::remove_child` DEFAULT body (a
    /// no-op). `Host` makes it REQUIRED, so the default that used to
    /// supply this body is gone — reproduced verbatim rather than
    /// inherited (deletion-baseline S2.2). Never reached here anyway:
    /// [`supports_splice`](Self::supports_splice) is `false`, so every
    /// reactive region tears down through `clear_children` on its own
    /// anchor.
    fn remove_child(&mut self, _parent: &Self::Node, _child: &Self::Node) {
        // default: no-op
    }

    fn clear_children(&mut self, node: &Self::Node) {
        node.borrow_mut().children.clear();
    }

    fn create_anchor(&mut self) -> Self::Node {
        // `display: contents` (matching web) keeps the `when`/`switch`/
        // `each` placeholder layout-transparent: the branch's children
        // inherit the surrounding flex/sizing context and a
        // `position: sticky` child gets the real parent as its containing
        // block (without this, the opaque anchor is a short containing
        // block and sticky stops sticking — e.g. the docs "On this page"
        // rail).
        let mut node = HtmlNode::new("div");
        node.style = Some(css::REACTIVE_ANCHOR_STYLE.to_string());
        nref(node)
    }

    /// Hard `false`. This is BOTH the value the old
    /// `Backend::supports_child_splice` default supplied (so SSR output is
    /// unchanged) and a load-bearing invariant in its own right: the web
    /// hydration boot adopts the anchors this render emits, so SSR must
    /// stay ANCHORED regardless of what any default says. `Host` makes it
    /// required, so it is stated here rather than inherited
    /// (deletion-baseline S2.2). Pinned by `newcore_host_is_anchored`.
    fn supports_splice(&self) -> bool {
        false
    }
}

// ===========================================================================
// App environment + lifecycle
// ===========================================================================

impl caps::AppEnvOps for SsrBackend {
    fn platform(&self) -> runtime_shared::Platform {
        runtime_shared::Platform::Web
    }

    fn set_page_metadata(&mut self, meta: &runtime_shared::PageMetadata) {
        self.metadata = meta.clone();
    }

    fn set_app_background(&mut self, color: &runtime_shared::Tokenized<runtime_shared::Color>) {
        self.app_bg = Some(color.clone());
    }

    fn set_scrollbar_theme(
        &mut self,
        thumb: &runtime_shared::Tokenized<runtime_shared::Color>,
        track: &runtime_shared::Tokenized<runtime_shared::Color>,
    ) {
        self.scrollbar = Some((thumb.clone(), track.clone()));
    }
}

impl caps::LifecycleOps for SsrBackend {
    fn finish(&mut self, root: Self::Node) {
        self.root = Some(root);
    }

    /// Headless render: keep `Element::Lazy` at its placeholder rather
    /// than resolving the chunk. The server can't paint lazy content (GPU
    /// canvas, etc.), and resolving it (the native loader resolves on
    /// first poll) would ship a body the client renders as a placeholder —
    /// a hydration mismatch. The live client loads the real chunk after
    /// adopting the matching placeholder.
    fn renders_lazy_chunks(&self) -> bool {
        false
    }
}

// ===========================================================================
// View + input + pressable
// ===========================================================================

impl caps::ViewOps for SsrBackend {
    fn create_view(&mut self, _a11y: &AccessibilityProps) -> Self::Node {
        nref(HtmlNode::new("div"))
    }
}

impl caps::InputOps for SsrBackend {}

impl caps::PressableOps for SsrBackend {
    fn create_pressable(
        &mut self,
        _on_click: Rc<dyn Fn()>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // A bare clickable `<div>`, matching the web pressable: button
        // a11y only. No inline cursor — `cursor` is now an
        // author/component style property (`StyleRules::cursor`), so the
        // static first paint matches the live web pressable (neither sets
        // an inline cursor) and hydration adoption stays consistent. The
        // click handler is the live bundle's job on hydration.
        let mut node = HtmlNode::new("div");
        node.attrs.push(("role", "button".to_string()));
        node.attrs.push(("tabindex", "0".to_string()));
        nref(node)
    }
}

// ===========================================================================
// Text + button
// ===========================================================================

impl caps::TextOps for SsrBackend {
    fn create_text(&mut self, content: &str, _a11y: &AccessibilityProps) -> Self::Node {
        let mut node = HtmlNode::new("span");
        node.text = Some(content.to_string());
        node.is_text = true;
        nref(node)
    }

    fn create_styled_text(
        &mut self,
        runs: &[runtime_shared::TextRun],
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Same structure the web backend builds live (outer <span>,
        // one child <span> per run, inline style from the shared css
        // emitter) so hydration adopts it as-is. Run colors emit as
        // `var(--token, fallback)` and resolve against the `:root`
        // token block this backend emits — the SSR first paint is
        // theme-correct without any JS.
        let mut outer = HtmlNode::new("span");
        outer.is_text = true;
        for run in runs {
            let mut child = HtmlNode::new("span");
            child.text = Some(run.text.clone());
            child.is_text = true;
            if let Some(style) = &run.style {
                if !style.is_empty() {
                    let decl = css::text_run_style_css(style);
                    if !decl.is_empty() {
                        child.style = Some(decl);
                    }
                }
            }
            outer.children.push(nref(child));
        }
        nref(outer)
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        node.borrow_mut().text = Some(content.to_string());
    }
}

impl caps::ButtonOps for SsrBackend {
    fn create_button(
        &mut self,
        label: &str,
        _on_click: &runtime_shared::Action,
        _leading_icon: Option<&runtime_shared::primitives::icon::IconData>,
        _trailing_icon: Option<&runtime_shared::primitives::icon::IconData>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let mut node = HtmlNode::new("button");
        node.text = Some(label.to_string());
        nref(node)
    }

    fn update_button_label(&mut self, node: &Self::Node, label: &str) {
        node.borrow_mut().text = Some(label.to_string());
    }
}

// ===========================================================================
// Image + icon + link
// ===========================================================================

impl caps::ImageOps for SsrBackend {
    fn create_image(
        &mut self,
        src: &str,
        alt: Option<&str>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let mut node = HtmlNode::new("img");
        node.attrs.push(("src", src.to_string()));
        node.attrs.push(("alt", alt.unwrap_or("").to_string()));
        nref(node)
    }

    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        set_attr(node,"src", src.to_string());
    }
}

impl caps::IconOps for SsrBackend {
    fn create_icon(
        &mut self,
        data: &runtime_shared::primitives::icon::IconData,
        color: Option<&runtime_shared::Color>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Emit the same `<svg>` structure the web backend produces so
        // `WebBackend::hydrate` can adopt the SSR node by tag-matching
        // (`svg` == `svg`). The earlier placeholder `<span>` triggered
        // a tag mismatch on every icon and `primitives::icon::create`
        // on web doesn't honor the hydration cursor — the fresh `<svg>`
        // appended next to the stale `<span>`, leaving both in the DOM.
        let (vw, vh) = data.view_box;
        let mut svg = HtmlNode::new("svg");
        svg.attrs
            .push(("viewBox", format!("0 0 {} {}", vw, vh)));
        svg.attrs.push(("xmlns", "http://www.w3.org/2000/svg".to_string()));
        svg.attrs.push(("width", "1em".to_string()));
        svg.attrs.push(("height", "1em".to_string()));
        // Mirror the web backend: filled icons paint the interior with the
        // icon color and disable the stroke; outlined icons stroke the
        // outline and leave the interior empty. Must match
        // `backend_web::primitives::icon::create` for hydration parity.
        let icon_color = color.map(|c| c.0.clone()).unwrap_or_else(|| "currentColor".to_string());
        if data.filled {
            svg.attrs.push(("fill", icon_color));
            svg.attrs.push(("stroke", "none".to_string()));
        } else {
            svg.attrs.push(("fill", "none".to_string()));
            svg.attrs.push(("stroke", icon_color));
            svg.attrs.push(("stroke-width", "2".to_string()));
            svg.attrs.push(("stroke-linecap", "round".to_string()));
            svg.attrs.push(("stroke-linejoin", "round".to_string()));
        }
        svg.style = Some(css::ICON_INLINE_STYLE.to_string());
        let fill_rule = match data.fill_rule {
            runtime_shared::primitives::icon::FillRule::NonZero => "nonzero",
            runtime_shared::primitives::icon::FillRule::EvenOdd => "evenodd",
        };
        for path_d in data.paths {
            let mut path = HtmlNode::new("path");
            path.attrs.push(("d", (*path_d).to_string()));
            path.attrs.push(("fill-rule", fill_rule.to_string()));
            path.attrs.push(("pathLength", "1".to_string()));
            path.attrs.push(("stroke-dasharray", "1".to_string()));
            path.attrs.push(("stroke-dashoffset", "0".to_string()));
            svg.children.push(nref(path));
        }
        nref(svg)
    }
}

impl caps::LinkOps for SsrBackend {
    fn create_link(
        &mut self,
        config: runtime_shared::primitives::link::LinkConfig,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let mut node = HtmlNode::new("a");
        // Same de-defaulting reset as the web link primitive (strip the
        // browser's blue/underlined anchor styling).
        node.style = Some(css::LINK_RESET_STYLE.to_string());
        node.attrs.push(("href", config.url.clone()));
        if config.external {
            node.attrs.push(("target", "_blank".to_string()));
            node.attrs.push(("rel", "noopener noreferrer".to_string()));
        }
        nref(node)
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
        _on_change: Rc<dyn Fn(String)>,
        _on_key_down: Option<runtime_shared::primitives::key::KeyDownHandler>,
        _on_blur: Option<runtime_shared::primitives::text_input::BlurHandler>,
        secure: bool,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let mut node = HtmlNode::new("input");
        node.attrs
            .push(("type", if secure { "password" } else { "text" }.to_string()));
        node.attrs.push(("value", initial_value.to_string()));
        if let Some(p) = placeholder {
            node.attrs.push(("placeholder", p.to_string()));
        }
        nref(node)
    }

    fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {
        set_attr(node,"value", value.to_string());
    }

    fn update_text_input_secure(&mut self, node: &Self::Node, secure: bool) {
        // Mirror the create-time `type` so a reactive `secure` resolved during
        // the server render emits the right input type for hydration.
        set_attr(node, "type", if secure { "password" } else { "text" }.to_string());
    }

    fn create_text_area(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        // Soft-wrap (default) vs. the code-editor no-wrap shape. SSR
        // emits the `wrap="off"` attribute for the latter so the
        // server-rendered first paint matches what the web backend
        // adopts on hydration. (Content-height growth needs no SSR
        // attribute — it's intrinsic sizing the client reproduces.)
        wrap: bool,
        // Row bounds need no SSR attribute: the client backend's autosize
        // reproduces the floor/cap from the same primitive props on hydration
        // (rows→px is a client-side metric). Accepted to match the trait.
        _min_rows: Option<u32>,
        _max_rows: Option<u32>,
        _on_change: Rc<dyn Fn(String)>,
        _on_key_down: Option<runtime_shared::primitives::key::KeyDownHandler>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let mut node = HtmlNode::new("textarea");
        node.text = Some(initial_value.to_string());
        if let Some(p) = placeholder {
            node.attrs.push(("placeholder", p.to_string()));
        }
        if !wrap {
            node.attrs.push(("wrap", "off".to_string()));
        }
        nref(node)
    }

    fn update_text_area_value(&mut self, node: &Self::Node, value: &str) {
        node.borrow_mut().text = Some(value.to_string());
    }
}

impl caps::ToggleOps for SsrBackend {
    fn create_toggle(
        &mut self,
        initial_value: bool,
        _on_change: Rc<dyn Fn(bool)>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let mut node = HtmlNode::new("input");
        node.attrs.push(("type", "checkbox".to_string()));
        if initial_value {
            node.attrs.push(("checked", String::new()));
        }
        nref(node)
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        if value {
            set_attr(node,"checked", String::new());
        } else {
            remove_attr(node, "checked");
        }
    }
}

impl caps::SliderOps for SsrBackend {
    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        _on_change: Rc<dyn Fn(f32)>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let mut node = HtmlNode::new("input");
        node.attrs.push(("type", "range".to_string()));
        node.attrs.push(("min", min.to_string()));
        node.attrs.push(("max", max.to_string()));
        if let Some(s) = step {
            node.attrs.push(("step", s.to_string()));
        }
        node.attrs.push(("value", initial_value.to_string()));
        nref(node)
    }

    fn update_slider_value(&mut self, node: &Self::Node, value: f32) {
        set_attr(node,"value", value.to_string());
    }
}

impl caps::ActivityIndicatorOps for SsrBackend {
    fn create_activity_indicator(
        &mut self,
        _size: runtime_shared::primitives::activity_indicator::ActivityIndicatorSize,
        _color: Option<&runtime_shared::Color>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Spinner animation is the live bundle's job; reserve a slot.
        nref(HtmlNode::new("div"))
    }
}

// ===========================================================================
// Scroll + safe area + virtualizer
// ===========================================================================

impl caps::ScrollOps for SsrBackend {
    fn create_scroll_view(
        &mut self,
        _horizontal: bool,
        _on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let mut node = HtmlNode::new("div");
        node.scroll = true;
        nref(node)
    }
}

impl caps::SafeAreaOps for SsrBackend {}

impl caps::VirtualizerOps for SsrBackend {
    fn create_virtualizer(
        &mut self,
        _callbacks: runtime_shared::VirtualizerCallbacks<Self::Node>,
        _overscan: f32,
        _layout: runtime_shared::VirtualLayout,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // First paint emits the scroll container only; the live bundle
        // mounts visible rows on boot. (Row pre-rendering for SEO of
        // virtualized content is a later enhancement.)
        nref(HtmlNode::new("div"))
    }
}

// ===========================================================================
// Graphics + portal + presence + navigator
// ===========================================================================

impl caps::GraphicsOps for SsrBackend {
    fn create_graphics(
        &mut self,
        _on_ready: runtime_shared::primitives::graphics::OnReady,
        _on_resize: runtime_shared::primitives::graphics::OnResize,
        _on_lost: runtime_shared::primitives::graphics::OnLost,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        nref(HtmlNode::new("canvas"))
    }
}

impl caps::PortalOps for SsrBackend {
    fn create_portal(
        &mut self,
        _target: runtime_shared::primitives::portal::PortalTarget,
        _on_dismiss: Option<Rc<dyn Fn()>>,
        _trap_focus: bool,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        nref(HtmlNode::new("div"))
    }
}

impl caps::PresenceOps for SsrBackend {}

// Both surviving `NavigatorOps` methods resolve to their caps DEFAULTS
// (no-op release; `navigator_attach_initial` drops the screen). The old
// bodies routed through a backend-side per-instance `NavigatorHandler`
// map populated exclusively by `create_navigator` — the one caps method
// that CEASES TO EXIST with the old core (deletion-baseline S2.3) — so
// the map could never be populated again. Navigators mount through
// `runtime_vocabulary::handlers::navigator` over the Lifecycle/View caps
// instead and never call this trait.
impl caps::NavigatorOps for SsrBackend {}

// ===========================================================================
// External + document
// ===========================================================================

impl caps::ExternalOps for SsrBackend {
    fn create_external(
        &mut self,
        _type_id: std::any::TypeId,
        _type_name: &'static str,
        _payload: &Rc<dyn std::any::Any>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Third-party primitives register a scene-`Registry` handler
        // (`codeblock::register`, `table::register`, …) and never
        // reach this method — the registry realizes their real DOM so
        // hydration adopts it. `create_external` therefore only serves
        // `missing_primitive_placeholder`: an empty host `<div>` the
        // client bundle fills in. (The old core routed here through a
        // backend-side `ExternalRegistry`, which died with
        // `Element::External`; the fallback shape is unchanged.)
        nref(HtmlNode::new("div"))
    }
}

impl caps::DocumentOps for SsrBackend {
    fn create_element(&mut self, tag: &str) -> Self::Node {
        // `HtmlNode.tag` is `&'static str`; intern the structural tags an
        // External handler might emit to a static (no allocation/leak).
        // Unknown tags fall back to `div`.
        let tag: &'static str = match tag {
            "pre" => "pre",
            "code" => "code",
            "p" => "p",
            "ul" => "ul",
            "ol" => "ol",
            "li" => "li",
            "blockquote" => "blockquote",
            "table" => "table",
            "thead" => "thead",
            "tbody" => "tbody",
            "tr" => "tr",
            "td" => "td",
            "th" => "th",
            "section" => "section",
            "article" => "article",
            "header" => "header",
            "footer" => "footer",
            "h1" => "h1",
            "h2" => "h2",
            "h3" => "h3",
            "h4" => "h4",
            "h5" => "h5",
            "h6" => "h6",
            // A GPU external (canvas) SSR-renders only its HOST element — the
            // content paints client-side after hydration. Interning `canvas`
            // lets an SSR external handler emit the real `<canvas>` the web
            // graphics primitive adopts, instead of the `div` fallback.
            "canvas" => "canvas",
            _ => "div",
        };
        nref(HtmlNode::new(tag))
    }

    fn attach_html_class(&self, node: &Self::Node, class: &str) {
        add_class(node, class);
    }

    fn attach_html_style(&self, node: &Self::Node, prop: &str, value: &str) {
        add_inline_style(node, prop, value);
    }

    fn register_raw_css(&mut self, css: &str) {
        // Dedupe: navigator chrome registers the same layout sheet on
        // every navigator instance.
        if !self.raw_css.iter().any(|c| c == css) {
            self.raw_css.push(css.to_string());
        }
    }
}

// ===========================================================================
// Style + assets
// ===========================================================================

impl caps::StyleOps for SsrBackend {
    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        // Match the web backend's structure: each resolved style becomes a
        // content-keyed class (`ui-<hash>`) plus one shared rule in the
        // document stylesheet — NOT an inline `style="…"`. Same
        // `hash_class_name` + `rules_to_css` as web, so a given style gets
        // the same class name and declarations on both. Dedupe by class so
        // N nodes sharing a style emit one rule (as web's `pregen` does).
        // A text node carrying a `shadow` lowers it to `text-shadow` and
        // mints a distinct class (keyed via `text_shadow_class_key`) so it
        // never reuses a box element's `box-shadow` class. Web mirrors this
        // exactly, so the class name matches on hydration.
        let is_text_shadow = node.borrow().is_text && css::text_needs_shadow_variant(style);
        let (class, body): (String, fn(&StyleRules) -> String) = if is_text_shadow {
            (
                css::hash_class_name(&css::text_shadow_class_key(&style.content_key())),
                css::rules_to_css_text,
            )
        } else {
            (css::hash_class_name(&style.content_key()), css::rules_to_css)
        };
        if !self.style_rules.contains_key(&class) {
            self.style_rules.insert(class.clone(), body(style));
        }
        let change = set_styled_class(node, &class);
        self.book_styled_class(&class, change);
    }

    fn apply_styled_states(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        overlays: &[(runtime_shared::StateBits, Rc<StyleRules>)],
    ) {
        // States-only entry; delegate to the superset with no breakpoint
        // or container overlays so the combined-key + emission logic lives
        // in one place.
        self.apply_styled_variants(node, base, overlays, &[], &[]);
    }

    fn apply_styled_variants(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        overlays: &[(runtime_shared::StateBits, Rc<StyleRules>)],
        breakpoint_overlays: &[(runtime_shared::Breakpoint, Rc<StyleRules>)],
        container_overlays: &[(f32, Rc<StyleRules>)],
    ) {
        // Key the class by base + every state overlay + every breakpoint
        // overlay + every container overlay through `css::variant_class_key`
        // — the SINGLE SOURCE shared with the web backend. Building the key
        // here independently (as this used to) drifted from web's scheme
        // (`|<bits>:` vs `;<tag>:`), so the SAME stateful style minted
        // DIFFERENT classes on server vs client and hydration couldn't
        // reuse the server's styling. Sharing the builder guarantees
        // byte-identical classes.
        let mut combined = css::variant_class_key(
            &base.content_key(),
            overlays,
            breakpoint_overlays,
            container_overlays,
        );
        // If this is a text node and any layer (base or overlay) carries a
        // shadow, the whole class renders shadows as `text-shadow` and mints
        // a distinct key (matching the web backend). `emit` picks the lowering.
        let text_shadow = node.borrow().is_text
            && (css::text_needs_shadow_variant(base)
                || overlays.iter().any(|(_, o)| css::text_needs_shadow_variant(o))
                || breakpoint_overlays.iter().any(|(_, o)| css::text_needs_shadow_variant(o))
                || container_overlays.iter().any(|(_, o)| css::text_needs_shadow_variant(o)));
        if text_shadow {
            combined = css::text_shadow_class_key(&combined);
        }
        let emit: fn(&StyleRules) -> String =
            if text_shadow { css::rules_to_css_text } else { css::rules_to_css };
        let class = css::hash_class_name(&combined);
        self.style_rules
            .entry(class.clone())
            .or_insert_with(|| emit(base));
        for (state, overlay) in overlays {
            if let Some(pseudo) = css::state_pseudo(*state) {
                // Key carries the pseudo so head_css emits
                // `.ui-<hash>:hover{ … }` (the node still wears `ui-<hash>`).
                self.style_rules
                    .entry(format!("{class}{pseudo}"))
                    .or_insert_with(|| {
                        let body = emit(overlay);
                        // Component-owned focus overlay suppresses the UA
                        // ring, matching the web backend's minted rule —
                        // without this the SSR first paint double-draws the
                        // native outline under the themed ring.
                        if *state == runtime_shared::StateBits::FOCUSED {
                            format!("outline:none;{body}")
                        } else {
                            body
                        }
                    });
            }
        }
        // Breakpoint overlays → `@media (min-width: …) { .ui-<hash> { … } }`.
        // Keyed by `{class}@{rank}` so `head_css`'s BTreeMap iteration emits
        // them ascending by rank (mobile-first cascade). `None` only for Xs,
        // which the walker never sends as an overlay.
        for (bp, overlay) in breakpoint_overlays {
            let body = emit(overlay);
            if let Some(rule) = css::breakpoint_media_rule(&class, *bp, &body) {
                self.media_rules
                    .entry(format!("{class}@{}", bp.rank()))
                    .or_insert(rule);
            }
        }
        // Container overlays → `@container (min-width: …) { .ui-<hash> { … } }`.
        // Keyed by `{class}@cq<threshold-bits>` so each distinct threshold
        // gets its own rule; the browser resolves it against the nearest
        // `container-type` ancestor (set by `mark_container`). Stacking by
        // source order reproduces the mobile-first cascade.
        for (threshold, overlay) in container_overlays {
            let body = emit(overlay);
            let rule = css::container_query_rule(&class, *threshold, &body);
            self.media_rules
                .entry(format!("{class}@cq{:08x}", threshold.to_bits()))
                .or_insert(rule);
        }
        let change = set_styled_class(node, &class);
        self.book_styled_class(&class, change);
    }

    fn mark_container(&mut self, node: &Self::Node) {
        // SSR mirrors web: tag the node as a containment context so
        // descendant `@container` rules resolve against it. Emitted as a
        // shared one-line class kept in `style_rules` (deduped by key).
        self.style_rules
            .entry(css::CONTAINER_TYPE_CLASS.to_string())
            .or_insert_with(|| css::CONTAINER_TYPE_BODY.to_string());
        add_class(node, css::CONTAINER_TYPE_CLASS);
    }

    // SSR opts into the web's declarative state model: interaction-state
    // overlays (`state hovered`, etc.) become CSS pseudo-class rules, so
    // hover/press/focus styling works on the static first paint with no
    // JS — same as the live web build (which the bundle takes over on
    // hydration). The event-driven `attach_states` path needs a runtime.
    fn handles_states_natively(&self) -> bool {
        true
    }

    fn install_tokens(&mut self, tokens: &[runtime_shared::TokenEntry]) {
        self.tokens = tokens.to_vec();
    }

    fn update_tokens(&mut self, tokens: &[runtime_shared::TokenEntry]) {
        // Merge: `update_tokens` may carry only the changed tokens.
        for incoming in tokens {
            if let Some(slot) = self.tokens.iter_mut().find(|t| t.name == incoming.name) {
                slot.value = incoming.value.clone();
            } else {
                self.tokens.push(incoming.clone());
            }
        }
    }

    // Preminted classes resolve against the same build-time `.css` asset
    // the served page links, so SSR markup carrying them is correct on
    // first paint and adopts cleanly on hydration.
    fn supports_preminted_styles(&self) -> bool {
        true
    }

    fn apply_default_text_font(&mut self, font: Option<&runtime_shared::FontFamily>) {
        self.default_text_font = font.cloned();
    }
}

impl caps::AssetOps for SsrBackend {
    fn register_asset(
        &mut self,
        id: runtime_shared::assets::AssetId,
        kind: runtime_shared::assets::AssetTag,
        source: &runtime_shared::assets::AssetSource,
    ) {
        if self.asset_urls.contains_key(&id) {
            return;
        }
        // `Embedded` sources have no served URL on a headless server
        // (they'd need a runtime blob, which is web-only) — skip them.
        if let Some(url) = css::asset_url(kind, source) {
            self.asset_urls.insert(id, url);
        }
    }

    fn register_typeface(
        &mut self,
        _id: runtime_shared::assets::TypefaceId,
        family_name: &str,
        faces: &[runtime_shared::assets::TypefaceFace],
        _fallback: runtime_shared::assets::SystemFallback,
    ) {
        for face in faces {
            if let Some(url) = self.asset_urls.get(&face.asset) {
                let rule = css::font_face_css(family_name, face, url);
                if !self.font_faces.contains(&rule) {
                    self.font_faces.push(rule);
                }
            }
        }
    }
}

// ===========================================================================
// A11y + animation + introspection
// ===========================================================================

impl caps::A11yOps for SsrBackend {}

impl caps::AnimationOps for SsrBackend {}

impl caps::IntrospectionOps for SsrBackend {}

// ===========================================================================
// Batch + wire bindings
// ===========================================================================

impl caps::BatchOps for SsrBackend {}

impl caps::WireBindingOps for SsrBackend {}

#[cfg(test)]
mod tests {
    use super::*;

    /// REGRESSION: SSR's `Host::supports_splice` must be `false`. An
    /// anchored render is what the web hydration boot adopts (module
    /// docs), and every frozen artifact in `tests/goldens/` was recorded
    /// in anchored mode. The value used to arrive from a `Backend` trait
    /// default; it is now an explicit body, so this literal assertion is
    /// the only thing standing between a typo and silently-broken
    /// hydration.
    #[test]
    fn newcore_host_is_anchored() {
        let b = SsrBackend::new();
        assert!(
            !Host::supports_splice(&b),
            "SSR Host must be anchored (hydration adopts the anchors)"
        );
    }
}
