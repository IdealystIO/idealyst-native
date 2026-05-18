//! Web backend: drives DOM nodes via web-sys/wasm-bindgen.
//!
//! # File layout
//!
//! - `style.rs` — CSS converters (`rules_to_css` + per-enum helpers),
//!   stylesheet rule-index bookkeeping (`insert_rule` / `delete_rule`
//!   on `WebBackend`), and the register/apply Backend methods that
//!   live next to the data they mutate.
//! - `defaults.rs` — global baselines: `.ui-default` class, spinner
//!   keyframes, virtualizer JS shim, dynamic-slot teardown.
//! - `primitives/` — one module per `Primitive` kind. Each owns its
//!   create/update functions, any `Ops` impl, and the `make_*_handle`
//!   builder where applicable. The `impl Backend for WebBackend`
//!   block at the bottom of this file is a thin delegation layer.
//!
//! # Style architecture
//!
//! Two distinct caches:
//!
//! - **Pre-generated cache.** Holds classes minted via
//!   `register_stylesheet` — variant combinations × theme. Content-keyed
//!   and shared across nodes. Lifecycle is anchored by the framework's
//!   `register_stylesheet` / `unregister_stylesheet` calls.
//!
//! - **Dynamic slots, one per styled node.** When a node's resolved
//!   style doesn't match any pre-generated class, the backend mints a
//!   per-node class for it. Each styled node owns at most one dynamic
//!   class. When the node's resolved style changes:
//!   1. Mint the new class (insert a CSS rule).
//!   2. Swap the node's `className`.
//!   3. Remove the old class's CSS rule.
//!
//! Dynamic classes are not shared across nodes — two nodes with the
//! same dynamic style get separate classes. The cost (slight CSS
//! duplication) is intentional: it eliminates content-keyed cache
//! contention for per-instance values and keeps dynamic-class lifecycle
//! simple (one class per node, replaced atomically).

#[cfg(feature = "async-driver")]
pub mod async_executor;
mod defaults;
#[cfg(feature = "aas-shell")]
pub mod dev_transport;
mod phase_timer;
mod primitives;
#[cfg(feature = "async-driver")]
pub mod render_loop;
pub mod scheduler;
mod style;
pub mod time_source;

#[cfg(feature = "async-driver")]
pub use async_executor::install_async_executor;
#[cfg(feature = "aas-shell")]
pub use dev_transport::{connect_web, WebClientHandle};
#[cfg(feature = "async-driver")]
pub use render_loop::install_render_loop;
pub use scheduler::install_scheduler;
pub use time_source::install_time_source;

use framework_core::{Backend, ButtonHandle, StyleRules};
use std::collections::HashMap;
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::{Document, Node};

pub struct WebBackend {
    pub(crate) doc: Document,
    pub(crate) mount: web_sys::Element,
    pub(crate) _click_closures: Vec<Closure<dyn FnMut()>>,
    /// Keyboard handlers for `Primitive::Pressable` (Enter/Space →
    /// click). Held so JS doesn't drop them while the element is in
    /// the layout tree. The click handler itself lives in
    /// `_click_closures` (shared shape: `FnMut()` no-arg).
    pub(crate) _pressable_key_closures: Vec<Closure<dyn FnMut(web_sys::KeyboardEvent)>>,
    /// Closures attached to `<a>` elements for `Primitive::Link`.
    /// Held so JS doesn't drop them while the anchor is still in
    /// the layout tree. Same posture as `_click_closures`.
    pub(crate) _link_click_closures: Vec<Closure<dyn FnMut(web_sys::MouseEvent)>>,
    /// Per-node interaction-event closures. Keyed by node-id so we
    /// can drop them when `on_node_unstyled` fires. Each entry holds
    /// the listeners for one node (pointerenter, pointerleave,
    /// pointerdown, pointerup, focusin, focusout) plus the
    /// pointer-event-type closures so the JS side keeps them alive.
    pub(crate) state_listeners: HashMap<u32, Vec<Closure<dyn FnMut(web_sys::Event)>>>,
    /// Has the `@keyframes ui-spin` rule been injected? First
    /// ActivityIndicator creation injects it; later creations skip
    /// the work.
    pub(crate) spinner_keyframes_injected: bool,
    /// Has the virtualizer JS shim been injected? First Virtualizer
    /// creation injects `runtime/js/virtualizer.js` into a
    /// `<script>` tag in the document head.
    pub(crate) virtualizer_shim_injected: bool,
    /// Per-virtualizer instance state — keyed by node id so we can
    /// route `virtualizer_data_changed` to the right instance AND
    /// drop its closures on `release_virtualizer`. The wrapped
    /// `VirtualizerInstance` owns the wasm-bindgen `Closure`s
    /// handed to the JS shim; dropping it destroys them via
    /// `__wbindgen_destroy_closure`, which is what prevents
    /// queued-but-not-yet-fired JS callbacks from reaching a
    /// freed-Signal arena slot after the surrounding scope has
    /// dropped.
    pub(crate) virtualizer_instances: HashMap<u32, primitives::virtualizer::VirtualizerInstance>,
    /// Monotonic id counter for virtualizer containers, written as
    /// `data-virtualizer-id` on the container `<div>`. Same trick as
    /// `data-graphics-id`: lets `release_virtualizer` look up the
    /// instance from a `&Node` without going through `node_ids`,
    /// which gets cleared by `on_node_unstyled` before our cleanup
    /// hook runs (style effects drop before the virtualizer cleanup
    /// effect within a single `Scope::drop` batch).
    pub(crate) next_virtualizer_id: u32,
    /// Per-Graphics-canvas runtime state — wgpu device, user closures,
    /// pending-paint flag, etc. Keyed by node id so `make_handle` can
    /// look up the matching instance after `create`. The `Rc` is the
    /// shared owner; the handle wraps the same Rc so `request_redraw`
    /// reaches the scheduler with no backend round-trip.
    pub(crate) graphics_instances:
        HashMap<u32, std::rc::Rc<std::cell::RefCell<primitives::graphics::GraphicsInstance>>>,
    /// Per-Navigator state. Keyed by the navigator id stamped on the
    /// container's `data-navigator-id` attribute so `make_handle` and
    /// `release_navigator` can find the right entry on lookup.
    pub(crate) navigator_instances: primitives::navigator::NavigatorInstances,
    /// Monotonic id counter for navigator containers. Same pattern as
    /// `next_graphics_id` — written as a data attribute on the
    /// container element.
    pub(crate) next_navigator_id: u32,
    /// Has the navigator CSS (`.ui-nav-root` + show/hide rules) been
    /// injected this session? Idempotent; first navigator create
    /// stamps it.
    pub(crate) navigator_css_injected: bool,
    /// Monotonic id counter for Graphics canvases. Written as the
    /// `data-graphics-id` attribute on each `<canvas>` so
    /// `make_handle` / `release` can look the instance up from a
    /// fresh `&Node` after the create call returned. Distinct from
    /// `next_node_id` — that one is keyed by Rust pointer identity,
    /// which doesn't survive return-by-value.
    pub(crate) next_graphics_id: u32,
    /// Shared `<style>` element holding every active CSS rule.
    pub(crate) style_element: Option<web_sys::HtmlStyleElement>,
    /// Pre-generated classes from `register_stylesheet`. Content-keyed,
    /// shared, refcounted (refcount tracks how many active
    /// registrations hold them — not how many nodes apply them).
    pub(crate) pregen: HashMap<String, PregenEntry>,
    /// Pointer-keyed mirror of `pregen` for the hot apply path. When
    /// the framework's resolution cache returns the same
    /// `Rc<StyleRules>` instance for many nodes (e.g. 10000 rows of
    /// the same variant), we look up the class name by `Rc::as_ptr`
    /// in O(1) — without paying for `content_key()` to format a
    /// 300-byte hex string per row.
    ///
    /// Populated by `register_stylesheet` alongside the content-keyed
    /// `pregen` map. Cleared on `unregister_stylesheet` /
    /// theme change.
    pub(crate) pregen_by_ptr: HashMap<*const framework_core::StyleRules, String>,
    /// Per-node dynamic class slot — `node_id -> (class_name, rule_index)`.
    /// At most one dynamic class per node. Replaced atomically when
    /// the node's resolved style changes.
    pub(crate) dynamic: HashMap<u32, DynamicSlot>,
    /// Stable per-Node id derived from the Node's pointer.
    pub(crate) next_node_id: u32,
    pub(crate) node_ids: HashMap<*const web_sys::Node, u32>,
    /// Indices in the shared `<style>` sheet that previously held a
    /// dynamic rule and are now available for re-use. See
    /// `insert_rule` / `delete_rule` in [`crate::style`] — instead
    /// of `deleteRule(idx)`-then-shifting-everything (O(N) per
    /// op), `delete_rule` records `idx` here and `insert_rule`
    /// recycles via an `insertRule(rule, idx)` after the matching
    /// `deleteRule(idx)`. The pair leaves all other indices
    /// unchanged, so insert+delete are both O(1) regardless of how
    /// many rules are live.
    pub(crate) free_rule_indices: Vec<u32>,
    /// CSS rule index of the `:root { --token: value; ... }` block
    /// that holds the active theme's token variables. `None` until the
    /// first `install_theme_variables` call. On theme swap we reach
    /// into the existing rule's `CSSStyleDeclaration` and `setProperty`
    /// each token in place — the rule itself is never deleted, so no
    /// other rule indices shift and no minted class re-emits.
    pub(crate) theme_root_rule_index: Option<u32>,
    /// Per-overlay state, keyed by the `data-overlay-id` attribute
    /// stamped on the portal root. Holds the wasm-bindgen `Closure`
    /// handles wired to dismiss events (Escape key, scrim click) so
    /// they stay alive while the overlay is mounted; dropping the
    /// instance entry in `release_overlay` is what frees the
    /// JS-side closures and prevents late-firing events from
    /// reaching a freed `Signal` slot.
    pub(crate) overlay_instances: primitives::overlay::OverlayInstances,
    /// Monotonic id counter for overlays. Same pattern as
    /// `next_navigator_id` — stamped as `data-overlay-id` on the
    /// portal root.
    pub(crate) next_overlay_id: u32,
    /// Has the (currently-empty) global overlay CSS been injected?
    /// Reserved for future focus-trap rules; the flag exists now so
    /// the injection step is idempotent.
    pub(crate) overlay_css_injected: bool,
}

/// Diagnostic snapshot returned by [`WebBackend::debug_counts`].
#[derive(Debug, Clone, Copy)]
pub struct WebBackendCounts {
    pub node_ids: usize,
    pub dynamic: usize,
    pub state_listeners: usize,
    pub pregen: usize,
    pub pregen_by_ptr: usize,
    pub free_rule_indices: usize,
    pub next_node_id: u32,
}

pub(crate) struct PregenEntry {
    #[allow(dead_code)]
    pub(crate) name: String,
    pub(crate) rule_index: u32,
    pub(crate) refcount: u32,
}

pub(crate) struct DynamicSlot {
    /// Kept for debugging — same hash that's set on the element's class.
    #[allow(dead_code)]
    pub(crate) name: String,
    /// CSS rule index for the base rule. Always set.
    pub(crate) rule_index: u32,
    /// Additional rule indices for per-state pseudo-class overlays
    /// (`.cls:hover`, `:active`, `:focus`, `:disabled`). Empty for
    /// nodes without `state` blocks.
    pub(crate) state_rule_indices: Vec<u32>,
}

impl WebBackend {
    /// Constructs a backend that will mount its root under `mount_selector`
    /// (e.g. `"#app"`). Panics if the element is not found.
    pub fn new(mount_selector: &str) -> Self {
        let window = web_sys::window().expect("no window");
        let doc = window.document().expect("no document");
        let mount = doc
            .query_selector(mount_selector)
            .expect("query failed")
            .expect("mount element not found");
        Self {
            doc,
            mount,
            _click_closures: Vec::new(),
            _pressable_key_closures: Vec::new(),
            _link_click_closures: Vec::new(),
            state_listeners: HashMap::new(),
            spinner_keyframes_injected: false,
            virtualizer_shim_injected: false,
            virtualizer_instances: HashMap::new(),
            next_virtualizer_id: 0,
            graphics_instances: HashMap::new(),
            next_graphics_id: 0,
            navigator_instances: HashMap::new(),
            next_navigator_id: 0,
            navigator_css_injected: false,
            style_element: None,
            pregen: HashMap::new(),
            pregen_by_ptr: HashMap::new(),
            dynamic: HashMap::new(),
            next_node_id: 0,
            node_ids: HashMap::new(),
            free_rule_indices: Vec::new(),
            theme_root_rule_index: None,
            overlay_instances: HashMap::new(),
            next_overlay_id: 0,
            overlay_css_injected: false,
        }
    }

    /// Diagnostic: snapshot of all the per-node HashMaps the backend
    /// owns. Used by the arena bench to detect when a rebuild loop
    /// leaves stale entries behind. Each field is a `usize` count of
    /// live entries; `free_rule_indices` shows how many CSS-rule
    /// slots are recycled (waiting to be reused) — large values
    /// indicate a previously-grown sheet that hasn't been compacted.
    pub fn debug_counts(&self) -> WebBackendCounts {
        WebBackendCounts {
            node_ids: self.node_ids.len(),
            dynamic: self.dynamic.len(),
            state_listeners: self.state_listeners.len(),
            pregen: self.pregen.len(),
            pregen_by_ptr: self.pregen_by_ptr.len(),
            free_rule_indices: self.free_rule_indices.len(),
            next_node_id: self.next_node_id,
        }
    }

    /// Assigns a stable per-Node id we use as a key in `dynamic`.
    pub(crate) fn node_id(&mut self, node: &Node) -> u32 {
        let p: *const web_sys::Node = node;
        if let Some(&id) = self.node_ids.get(&p) {
            return id;
        }
        let id = self.next_node_id;
        self.next_node_id += 1;
        self.node_ids.insert(p, id);
        id
    }
}

// ---------------------------------------------------------------------------
// Backend trait impl. Each method delegates to the matching primitive
// module (or to one of the style/defaults helpers on `WebBackend`).
// Keep this thin — anything substantial belongs in the primitive's file.
// ---------------------------------------------------------------------------

impl Backend for WebBackend {
    type Node = Node;

    fn color_scheme(&self) -> framework_core::ColorScheme {
        let window = match self.doc.default_view() {
            Some(w) => w,
            None => return framework_core::ColorScheme::Auto,
        };
        let prefers_dark = window
            .match_media("(prefers-color-scheme: dark)")
            .ok()
            .flatten()
            .map(|mql| mql.matches())
            .unwrap_or(false);
        let prefers_light = window
            .match_media("(prefers-color-scheme: light)")
            .ok()
            .flatten()
            .map(|mql| mql.matches())
            .unwrap_or(false);
        if prefers_dark {
            framework_core::ColorScheme::Dark
        } else if prefers_light {
            framework_core::ColorScheme::Light
        } else {
            framework_core::ColorScheme::Auto
        }
    }

    fn create_view(&mut self) -> Self::Node {
        primitives::view::create(self)
    }

    fn create_reactive_anchor(&mut self) -> Self::Node {
        primitives::view::create_reactive_anchor(self)
    }

    fn create_text(&mut self, content: &str) -> Self::Node {
        primitives::text::create(self, content)
    }

    fn create_button(
        &mut self,
        label: &str,
        on_click: Rc<dyn Fn()>,
        leading_icon: Option<&framework_core::IconData>,
        trailing_icon: Option<&framework_core::IconData>,
    ) -> Self::Node {
        primitives::button::create(self, label, on_click, leading_icon, trailing_icon)
    }

    fn create_pressable(&mut self, on_click: Rc<dyn Fn()>) -> Self::Node {
        primitives::pressable::create(self, on_click)
    }

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        primitives::view::insert(parent, child)
    }

    fn insert_many(&mut self, parent: &mut Self::Node, children: Vec<Self::Node>) {
        primitives::view::insert_many(self, parent, children)
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        primitives::text::update_text(node, content)
    }

    fn create_image(&mut self, src: &str, alt: Option<&str>) -> Self::Node {
        primitives::image::create(self, src, alt)
    }

    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        primitives::image::update_src(node, src)
    }

    fn create_icon(
        &mut self,
        data: &framework_core::primitives::icon::IconData,
        color: Option<&framework_core::Color>,
    ) -> Self::Node {
        primitives::icon::create(self, data, color)
    }

    fn update_icon_color(&mut self, node: &Self::Node, color: &framework_core::Color) {
        primitives::icon::update_color(node, color)
    }

    fn update_icon_stroke(&mut self, node: &Self::Node, progress: f32) {
        primitives::icon::update_stroke(node, progress)
    }

    fn animate_icon_stroke(
        &mut self,
        node: &Self::Node,
        from: f32,
        to: f32,
        duration_ms: u32,
        easing: framework_core::Easing,
        infinite: bool,
        _autoreverses: bool,
    ) {
        primitives::icon::animate_stroke(node, from, to, duration_ms, easing, infinite)
    }

    fn create_text_input(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
    ) -> Self::Node {
        primitives::text_input::create(self, initial_value, placeholder, on_change)
    }

    fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {
        primitives::text_input::update_value(node, value)
    }

    fn create_toggle(&mut self, initial_value: bool, on_change: Rc<dyn Fn(bool)>) -> Self::Node {
        primitives::toggle::create(self, initial_value, on_change)
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        primitives::toggle::update_value(node, value)
    }

    fn create_scroll_view(&mut self, horizontal: bool) -> Self::Node {
        primitives::scroll_view::create(self, horizontal)
    }

    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
    ) -> Self::Node {
        primitives::slider::create(self, initial_value, min, max, step, on_change)
    }

    fn update_slider_value(&mut self, node: &Self::Node, value: f32) {
        primitives::slider::update_value(node, value)
    }

    fn create_web_view(&mut self, url: &str) -> Self::Node {
        primitives::web_view::create(self, url)
    }

    fn update_web_view_url(&mut self, node: &Self::Node, url: &str) {
        primitives::web_view::update_url(node, url)
    }

    fn create_video(
        &mut self,
        src: &str,
        autoplay: bool,
        controls: bool,
        loop_playback: bool,
    ) -> Self::Node {
        primitives::video::create(self, src, autoplay, controls, loop_playback)
    }

    fn update_video_src(&mut self, node: &Self::Node, src: &str) {
        primitives::video::update_src(node, src)
    }

    fn create_activity_indicator(
        &mut self,
        size: framework_core::primitives::activity_indicator::ActivityIndicatorSize,
        color: Option<&framework_core::Color>,
    ) -> Self::Node {
        primitives::activity_indicator::create(self, size, color)
    }

    fn create_virtualizer(
        &mut self,
        callbacks: framework_core::VirtualizerCallbacks<Self::Node>,
        overscan: f32,
        horizontal: bool,
    ) -> Self::Node {
        primitives::virtualizer::create(self, callbacks, overscan, horizontal)
    }

    fn virtualizer_data_changed(&mut self, node: &Self::Node) {
        primitives::virtualizer::data_changed(self, node)
    }

    fn release_virtualizer(&mut self, node: &Self::Node) {
        primitives::virtualizer::release(self, node)
    }

    fn create_graphics(
        &mut self,
        on_ready: framework_core::primitives::graphics::OnReady,
        on_resize: framework_core::primitives::graphics::OnResize,
        on_lost: framework_core::primitives::graphics::OnLost,
    ) -> Self::Node {
        primitives::graphics::create(self, on_ready, on_resize, on_lost)
    }

    fn release_graphics(&mut self, node: &Self::Node) {
        primitives::graphics::release(self, node)
    }

    fn make_graphics_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::primitives::graphics::GraphicsHandle {
        primitives::graphics::make_handle(self, node)
    }

    fn create_navigator(
        &mut self,
        callbacks: framework_core::NavigatorCallbacks<Self::Node>,
        control: Rc<framework_core::NavigatorControl>,
    ) -> Self::Node {
        primitives::navigator::create(self, callbacks, control)
    }

    fn navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        _options: framework_core::primitives::navigator::ScreenOptions,
    ) {
        // The framework's local-mode path runs the initial mount
        // via the microtask in `create_navigator` and never calls
        // this method directly (the trait default is a no-op).
        // AAS mode is the opposite: the create-time microtask
        // bails early on `defer_initial_mount = true`, and this
        // method is the one that actually mounts the screen,
        // using the wire-supplied DOM subtree + scope id.
        primitives::navigator::attach_initial(self, navigator, screen, scope_id)
    }

    fn release_navigator(&mut self, node: &Self::Node) {
        primitives::navigator::release(self, node)
    }

    fn make_navigator_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::NavigatorHandle {
        primitives::navigator::make_handle(self, node)
    }

    // On web every navigator kind reduces to the same underlying
    // screen-swap-plus-layout machinery — the layout slot is where
    // tab bars and drawer panels actually render. So tab + drawer
    // creation just dispatches into the existing instance code with
    // a kind-appropriate command dispatcher; teardown reuses
    // `release` because the entry shape is identical.
    fn create_tab_navigator(
        &mut self,
        callbacks: framework_core::TabNavigatorCallbacks<Self::Node>,
        control: Rc<framework_core::NavigatorControl>,
    ) -> Self::Node {
        primitives::navigator::create_tab(self, callbacks, control)
    }

    fn release_tab_navigator(&mut self, node: &Self::Node) {
        primitives::navigator::release(self, node)
    }

    fn make_tab_navigator_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::TabsHandle {
        primitives::navigator::make_tab_handle(self, node)
    }

    fn create_drawer_navigator(
        &mut self,
        callbacks: framework_core::DrawerNavigatorCallbacks<Self::Node>,
        control: Rc<framework_core::NavigatorControl>,
    ) -> Self::Node {
        primitives::navigator::create_drawer(self, callbacks, control)
    }

    fn release_drawer_navigator(&mut self, node: &Self::Node) {
        primitives::navigator::release(self, node)
    }

    fn make_drawer_navigator_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::DrawerHandle {
        primitives::navigator::make_drawer_handle(self, node)
    }

    fn create_link(
        &mut self,
        config: framework_core::primitives::link::LinkConfig,
    ) -> Self::Node {
        primitives::link::create(self, config)
    }

    fn make_link_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::primitives::link::LinkHandle {
        primitives::link::make_handle(node)
    }

    fn create_overlay(
        &mut self,
        placement: framework_core::primitives::overlay::ViewportPlacement,
        backdrop: framework_core::primitives::overlay::BackdropMode,
        on_dismiss: Option<Rc<dyn Fn()>>,
        trap_focus: bool,
    ) -> Self::Node {
        primitives::overlay::create_viewport(self, placement, backdrop, on_dismiss, trap_focus)
    }

    fn apply_overlay_backdrop_style(
        &mut self,
        node: &Self::Node,
        style: &Rc<StyleRules>,
    ) {
        primitives::overlay::apply_backdrop_style(self, node, style)
    }

    fn release_overlay(&mut self, node: &Self::Node) {
        primitives::overlay::release(self, node)
    }

    fn make_overlay_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::primitives::overlay::OverlayHandle {
        primitives::overlay::make_handle(node)
    }

    fn create_anchored_overlay(
        &mut self,
        target: framework_core::primitives::overlay::AnchorTarget,
        side: framework_core::primitives::overlay::ElementSide,
        align: framework_core::primitives::overlay::ElementAlign,
        offset: f32,
        backdrop: framework_core::primitives::overlay::BackdropMode,
        on_dismiss: Option<Rc<dyn Fn()>>,
        trap_focus: bool,
    ) -> Self::Node {
        primitives::overlay::create_anchored(
            self, target, side, align, offset, backdrop, on_dismiss, trap_focus,
        )
    }

    fn apply_anchored_overlay_backdrop_style(
        &mut self,
        node: &Self::Node,
        style: &Rc<StyleRules>,
    ) {
        // Same plumbing as viewport overlays on the web — backdrop is
        // a separate child element of the portal root either way.
        primitives::overlay::apply_backdrop_style(self, node, style)
    }

    fn release_anchored_overlay(&mut self, node: &Self::Node) {
        primitives::overlay::release(self, node)
    }

    fn make_anchored_overlay_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::primitives::overlay::AnchoredOverlayHandle {
        primitives::overlay::make_anchored_handle(node)
    }

    fn apply_presence(
        &mut self,
        node: &Self::Node,
        state: framework_core::PresenceState,
        transition: Option<(u32, framework_core::Easing)>,
    ) {
        primitives::presence::apply(self, node, state, transition)
    }

    fn clear_children(&mut self, node: &Self::Node) {
        primitives::view::clear_children(node)
    }

    fn register_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        self.impl_register_stylesheet(rules)
    }

    fn unregister_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        self.impl_unregister_stylesheet(rules)
    }

    fn install_theme_variables(&mut self, tokens: &[framework_core::TokenEntry]) {
        self.impl_install_theme_variables(tokens)
    }

    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        self.impl_apply_style(node, style)
    }

    /// Web handles interaction states via CSS pseudo-classes
    /// (`:hover`, `:active`, `:focus`, `:disabled`) — the browser
    /// tracks transitions natively and no Rust-side state signal is
    /// needed. The framework calls `apply_styled_states` instead of
    /// `apply_style` when this returns true.
    fn handles_states_natively(&self) -> bool {
        true
    }

    fn apply_styled_states(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        overlays: &[(framework_core::StateBits, Rc<StyleRules>)],
    ) {
        self.impl_apply_styled_states(node, base, overlays)
    }

    fn on_node_unstyled(&mut self, node: &Self::Node) {
        self.impl_on_node_unstyled(node)
    }

    fn set_disabled(&mut self, node: &Self::Node, disabled: bool) {
        // Most disable-able elements (button, input, select) accept
        // the `disabled` attribute. We set/remove it as appropriate.
        // For non-form elements, this is a no-op visually but doesn't
        // hurt.
        let Ok(element) = node.clone().dyn_into::<web_sys::Element>() else {
            return;
        };
        if disabled {
            let _ = element.set_attribute("disabled", "");
        } else {
            let _ = element.remove_attribute("disabled");
        }
    }

    /// Web state styling uses native CSS pseudo-classes (`:hover`,
    /// `:active`, `:focus`, `:disabled`) rather than reactive JS
    /// listeners. That happens at CSS-emit time in `apply_style` (see
    /// `rules_to_css` / pseudo-class rule generation), not here. We
    /// override `attach_states` to a no-op so the framework's
    /// signal-driven state machinery doesn't fire on web.
    ///
    /// Why not listeners + signal-driven re-style? It causes wasm-
    /// bindgen `WasmRefCell` re-entry crashes when DOM events fire
    /// while a style is being applied, and the CSS path is both
    /// simpler and faster (browser tracks the state natively, no
    /// per-event Rust↔JS round trip).
    fn attach_states(
        &mut self,
        _node: &Self::Node,
        _setter: Rc<dyn Fn(framework_core::StateBits, bool)>,
    ) {
        // intentional no-op on web; CSS pseudo-classes drive states.
    }

    fn make_button_handle(&self, node: &Self::Node) -> ButtonHandle {
        primitives::button::make_handle(node)
    }

    fn make_pressable_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::PressableHandle {
        primitives::pressable::make_handle(node)
    }

    fn make_view_handle(&self, node: &Self::Node) -> framework_core::ViewHandle {
        // Wrap the actual `web_sys::Node` (not the trait-default
        // `Rc<()>`), so framework helpers like `LayoutPlan` can
        // downcast back to the concrete node and operate on it.
        framework_core::ViewHandle::new(Rc::new(node.clone()), &WebViewOps)
    }

    fn make_text_input_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::primitives::text_input::TextInputHandle {
        primitives::text_input::make_handle(node)
    }

    fn make_scroll_view_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::primitives::scroll_view::ScrollViewHandle {
        primitives::scroll_view::make_handle(node)
    }

    fn make_video_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::primitives::video::VideoHandle {
        primitives::video::make_handle(node)
    }

    fn finish(&mut self, root: Self::Node) {
        self.mount
            .append_child(&root)
            .expect("mount append failed");
    }
}

/// Marker ops for `ViewHandle`. Views don't have methods yet (no
/// scroll, no measure) — the trait is reserved for future
/// additions. We still need an instance to satisfy
/// `ViewHandle::new`'s `&'static dyn ViewOps` parameter.
struct WebViewOps;
impl framework_core::ViewOps for WebViewOps {
    fn rect(&self, node: &dyn std::any::Any) -> framework_core::ViewportRect {
        let el: &web_sys::Node = match node.downcast_ref::<web_sys::Node>() {
            Some(n) => n,
            None => return framework_core::ViewportRect::default(),
        };
        let element: web_sys::Element = match el.clone().dyn_into() {
            Ok(e) => e,
            Err(_) => return framework_core::ViewportRect::default(),
        };
        let r = element.get_bounding_client_rect();
        framework_core::ViewportRect {
            x: r.x() as f32,
            y: r.y() as f32,
            width: r.width() as f32,
            height: r.height() as f32,
        }
    }
}
