//! New-core adoption for the wire-recording dev backend (idea-lite
//! migration — the dev-session wire chain).
//!
//! Implements [`runtime_scene::Host`] plus **all 30** capability traits
//! (`runtime_vocabulary::caps`) directly on [`WireRecordingBackend`] —
//! the same shape every shipping backend took (see
//! `backend-ssr/src/newcore.rs`, the template this file's delegation
//! bodies are generated from). Every trait method delegates via UFCS
//! (`<WireRecordingBackend as Backend>::method(self, …)`) to the
//! existing `Backend` impl, so the wire emission mechanism — node/style
//! interning, handler-table registration, `SceneModel` mirroring, the
//! `Command` log — is REUSED verbatim.
//!
//! **The wire protocol is the compatibility contract.** Because the
//! caps signatures are frozen to the old `Backend` signatures and every
//! cap here resolves to the recorder's existing `Backend` impl, a
//! new-core realize that performs the same logical backend operations
//! as an old-core walk emits the **identical wire `Command`s** — by
//! construction, not by re-implementation. The new-core wire-behavior
//! tests in `mock-backend` (`tests/wire_behavior_newcore.rs`) pin this
//! end-to-end: same scene → same reconstructed client tree, over the
//! real codec.
//!
//! # The recorder is ANCHORED — the wire structural contract
//!
//! [`Host::supports_splice`] is **hard-coded `false`** (not delegated):
//! the wire protocol has `Insert` / `InsertMany` / `ClearChildren` ops
//! but **no** `RemoveChild` / `InsertAt` — a splice simply cannot be
//! expressed on the wire. Anchored mode keeps every reactive region
//! under a `CreateReactiveAnchor` + `ClearChildren` + re-insert cycle,
//! all of which are wire ops the replay client already understands
//! (including old clients — no protocol bump). If the recorder ever
//! wants splice, the protocol must grow those two ops first; flipping
//! this bool without them silently drops structural updates on every
//! client. Regression-pinned by `newcore_recorder_is_anchored`.
//!
//! # Session mounting — [`SceneSession`]
//!
//! [`SceneSession::mount`] is the new-core counterpart of the old
//! `runtime_core::mount(recorder, app)` call the sidecar makes: fresh
//! per-session [`World`], `register_builtins` + a registration seam,
//! realize inside `World::enter`, `finish` the single root, one
//! `flush`. Dropping the session runs every cleanup (realized tree
//! first, then the world — same teardown order as
//! `backend_ssr::newcore::render_path`). Event dispatch commits through
//! [`SceneSession::flush`] — world signals have no ambient auto-flush
//! driver on the dev server; the sidecar flushes after every dispatched
//! event / animation tick, then drains the recorder's command log.
//!
//! # Robot + MCP catalog (wave 2b — the pre-2b "no catalog in
//! `--new-core` sessions" gap is CLOSED)
//!
//! [`install_robot_env`] wires the vocabulary robot registry into the
//! shared TCP bridge for a mounted session (driver env + verb router;
//! see its docs), and the sidecar session thread starts the bridge
//! transport exactly like the old-core `mount` did. The catalog side
//! needs no code here: the recipes are static data in
//! `runtime_shared::recipes`, and the generated `--new-core` sidecar
//! wrapper enables `runtime-core/dev` + `runtime-facade/dev` so the
//! emission gate, the `__mcp` anchors, and the bridge's `get_catalog`
//! verb are all live (build-runtime-server pins this).
//!
//! # What the new-core session does NOT yet do (each named, none silent)
//!
//! - **Identity-keyed node dedup across re-mounts.** The old walker set
//!   `runtime_core::current_identity()` before every `create_*`, which
//!   the recorder's `mint_node` uses to reuse wire `NodeId`s across
//!   hot-reload re-renders (incremental patching). The new-core realize
//!   path does not set ambient identities, so every re-mount mints
//!   fresh ids and clients rebuild the scene from the epoch-bumped
//!   snapshot — correct, but a full rebuild rather than an incremental
//!   patch. Fixing this belongs in the vocabulary drivers (set ambient
//!   identity per mount site), not here.
//! - **Hot-patch function rebinding.** The `#[component]` macro's
//!   new-core emission has no `dev_hot` split form yet, so a subsecond
//!   jump table cannot rebind patched component bodies. The CLI's
//!   `--new-core` dev flow therefore disables the hot-patch adapter and
//!   rides the host's rebuild-and-respawn path (state does not survive
//!   saves; the session itself and all clients do).

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
use runtime_scene::{realize, Host, Realized, Registry};
use runtime_vocabulary::caps;
use runtime_world::World;

use crate::WireRecordingBackend;

/// Re-exports for generated sidecar wrappers and app-side registration
/// seams (`register_scene_extensions_recorder(&mut SceneRegistry)`), so
/// user crates don't need their own `runtime-scene` dep.
pub use runtime_scene::Element as SceneElement;
/// See [`SceneElement`].
pub type SceneRegistry = Registry<WireRecordingBackend>;

// ===========================================================================
// Session mounting
// ===========================================================================

/// A mounted new-core dev session: the per-session [`World`] plus the
/// realized scene, in teardown order (realized drops first — cleanups
/// fire while their world is still alive).
pub struct SceneSession {
    // Field order IS the drop order: `realized` unmounts (scope
    // cleanups, release_* recorder emissions) before `world` — its
    // slots' owner — dies and unregisters from the thread's world
    // table. Same contract as `backend_ssr::newcore::render_path`.
    realized: Realized<wire::NodeId>,
    world: World,
}

impl SceneSession {
    /// Mount `app`'s scene on a fresh world against `recorder`,
    /// recording the full initial scene as wire commands (ending in
    /// `Command::Finish`). `register` runs after
    /// [`runtime_vocabulary::register_builtins`] so apps/SDKs can add
    /// their own scene handlers to the same registry.
    pub fn mount(
        recorder: &WireRecordingBackend,
        register: impl FnOnce(&mut SceneRegistry),
        app: impl FnOnce() -> SceneElement,
    ) -> Self {
        let backend = Rc::new(RefCell::new(recorder.clone()));
        let mut registry: SceneRegistry = Registry::new();
        runtime_vocabulary::register_builtins(&mut registry);
        register(&mut registry);
        let registry = Rc::new(registry);

        let world = World::new();
        let realized = world.enter(|| {
            let element = app();
            realize(&backend, &registry, element)
        });

        // Single-root contract, matching the old-core `mount` and the
        // other new-core boots: `finish` marks the mount complete on
        // the wire (`Command::Finish { root }`), which replay clients
        // use to close the initial batch.
        let mut roots = realized.collect_nodes();
        let root = match roots.len() {
            1 => roots.pop().expect("len checked"),
            n => panic!(
                "dev_server::newcore::SceneSession::mount: the app root must contribute \
                 exactly one top-level node (got {n}) — wrap fragment/multi-root trees in \
                 a view"
            ),
        };
        Backend::finish(&mut *backend.borrow_mut(), root);

        // Commit anything staged during mount (write-backs,
        // driver-effect state) — the mount's first flush.
        world.flush();

        Self { realized, world }
    }

    /// Commit pending reactive work (signal writes from dispatched
    /// events, animation ticks) so the effects they trigger re-fire and
    /// emit their wire deltas into the recorder's log. Call after every
    /// `WireRecordingBackend::dispatch_event` / `dispatch_state` /
    /// `tick_animations`, before draining commands. The dev server has
    /// no ambient flush driver (unlike backend-web's rAF-hooked one) —
    /// the session's message loop IS the driver.
    pub fn flush(&self) {
        self.world.flush();
    }

    /// The session's world — for tests that need `enter` (e.g. to
    /// create signals in the app closure's world context up front).
    pub fn world(&self) -> &World {
        &self.world
    }

    /// Number of realized root nodes (diagnostics).
    pub fn root_count(&self) -> usize {
        self.realized.collect_nodes().len()
    }
}

impl Host for WireRecordingBackend {
    type Node = <WireRecordingBackend as Backend>::Node;

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

    /// Hard `false`, NOT delegated to `supports_child_splice`: the wire
    /// protocol has no `RemoveChild`/`InsertAt` ops, so a splice cannot
    /// be expressed to replay clients — anchored regions
    /// (`CreateReactiveAnchor` + `ClearChildren` + re-insert) are the
    /// wire's structural contract (module docs, "the recorder is
    /// ANCHORED"). Regression-pinned by `newcore_recorder_is_anchored`.
    fn supports_splice(&self) -> bool {
        false
    }
}

// ===========================================================================
// App environment + lifecycle
// ===========================================================================

impl caps::AppEnvOps for WireRecordingBackend {
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

impl caps::LifecycleOps for WireRecordingBackend {
    fn finish(&mut self, root: Self::Node) {
        Backend::finish(self, root)
    }

    fn run_layout(&mut self) {
        Backend::run_layout(self)
    }

    fn schedule_layout_pass() {
        <WireRecordingBackend as Backend>::schedule_layout_pass()
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

impl caps::ViewOps for WireRecordingBackend {
    fn create_view(&mut self, a11y: &AccessibilityProps) -> Self::Node {
        Backend::create_view(self, a11y)
    }

    fn make_view_handle(&self, node: &Self::Node) -> runtime_core::ViewHandle {
        Backend::make_view_handle(self, node)
    }
}

impl caps::InputOps for WireRecordingBackend {
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

impl caps::PressableOps for WireRecordingBackend {
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

impl caps::TextOps for WireRecordingBackend {
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

impl caps::ButtonOps for WireRecordingBackend {
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

impl caps::ImageOps for WireRecordingBackend {
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

impl caps::IconOps for WireRecordingBackend {
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

impl caps::LinkOps for WireRecordingBackend {
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

impl caps::TextInputOps for WireRecordingBackend {
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

impl caps::ToggleOps for WireRecordingBackend {
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

impl caps::SliderOps for WireRecordingBackend {
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

impl caps::ActivityIndicatorOps for WireRecordingBackend {
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

impl caps::ScrollOps for WireRecordingBackend {
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

impl caps::SafeAreaOps for WireRecordingBackend {
    fn apply_safe_area_padding(&mut self, node: &Self::Node, sides: SafeAreaSides) {
        Backend::apply_safe_area_padding(self, node, sides)
    }

    fn apply_scroll_view_safe_area_inset(&mut self, node: &Self::Node, sides: SafeAreaSides) {
        Backend::apply_scroll_view_safe_area_inset(self, node, sides)
    }
}

impl caps::VirtualizerOps for WireRecordingBackend {
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

impl caps::GraphicsOps for WireRecordingBackend {
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

impl caps::PortalOps for WireRecordingBackend {
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

impl caps::PresenceOps for WireRecordingBackend {
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

impl caps::NavigatorOps for WireRecordingBackend {
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

impl caps::ExternalOps for WireRecordingBackend {
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

impl caps::DocumentOps for WireRecordingBackend {
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

impl caps::StyleOps for WireRecordingBackend {
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

impl caps::AssetOps for WireRecordingBackend {
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

impl caps::A11yOps for WireRecordingBackend {
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

impl caps::AnimationOps for WireRecordingBackend {
    fn set_animated_f32(&mut self, node: &Self::Node, prop: AnimProp, value: f32) {
        Backend::set_animated_f32(self, node, prop, value)
    }

    fn set_animated_color(&mut self, node: &Self::Node, prop: AnimProp, value: [f32; 4]) {
        Backend::set_animated_color(self, node, prop, value)
    }
}

impl caps::IntrospectionOps for WireRecordingBackend {
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

impl caps::BatchOps for WireRecordingBackend {
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

impl caps::WireBindingOps for WireRecordingBackend {
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

// ---------------------------------------------------------------------------
// Robot env for a new-core session (wave 2b: robot + MCP catalog in
// `--new-core` dev sessions)
// ---------------------------------------------------------------------------

/// Install the vocabulary Robot driver env + the shared bridge's verb
/// router for a mounted [`SceneSession`]. One function used by BOTH the
/// sidecar session thread (`sidecar::run_session_thread_newcore`) and
/// the integration tests, so the tested wiring IS the shipped wiring.
///
/// - **Driver env**: queries run with the session's world entered
///   (label closures read world signals), actions settle through
///   `SceneSession::flush` so a verb's staged writes commit before its
///   reply — the same contract `backend_web::robot_transport` installs
///   for web boots.
/// - **Verb router**: the shared TCP bridge dispatches against the OLD
///   registry, which a new-core session leaves empty (`find_element`
///   would answer `null`, silently blinding drivers). The router
///   forwards verbs the vocabulary bridge owns to
///   `runtime_vocabulary::robot::bridge::invoke_command`
///   (wire-identical responses) and falls back — keyed on the exact
///   `unknown command:` marker so real verb errors are never masked —
///   for the registry-INDEPENDENT verbs old dispatch still owns:
///   `get_catalog` (the MCP catalog), `get_logs`, and custom commands
///   (the sidecar's `screenshot`).
///
/// The holder is `Rc<RefCell<Option<SceneSession>>>` so the closures
/// track the CURRENT session across Rerender remounts; a `None` window
/// (mid-remount) degrades to plain (un-entered) query execution.
pub fn install_robot_env(session: &Rc<RefCell<Option<SceneSession>>>) {
    let env_session = session.clone();
    let settle_session = session.clone();
    runtime_vocabulary::robot::install_driver_env(
        move |f| match env_session.borrow().as_ref() {
            Some(s) => {
                s.world.enter(|| f());
            }
            None => f(),
        },
        move || {
            if let Some(s) = settle_session.borrow().as_ref() {
                s.flush();
            }
        },
    );
    runtime_core::robot::bridge::install_verb_router(|cmd, args| {
        match runtime_vocabulary::robot::bridge::invoke_command(cmd, args) {
            Err(e) if e.starts_with("unknown command:") => None,
            other => Some(other),
        }
    });
}

/// Tear down what [`install_robot_env`] installed (session shutdown /
/// tests that boot repeatedly on one thread).
pub fn clear_robot_env() {
    runtime_core::robot::bridge::clear_verb_router();
    runtime_vocabulary::robot::clear_driver_env();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// REGRESSION: the recorder's `Host::supports_splice` must be
    /// `false` independently of the `Backend` default — the wire
    /// protocol has no `RemoveChild`/`InsertAt` ops, so a splice can't
    /// be expressed to replay clients (module docs, "the recorder is
    /// ANCHORED"). If the old-core default ever flips, this pin keeps
    /// the wire leg anchored until the protocol grows splice ops.
    #[test]
    fn newcore_recorder_is_anchored() {
        let b = WireRecordingBackend::new();
        assert!(
            !Host::supports_splice(&b),
            "the wire recorder must stay anchored: the protocol cannot express splices"
        );
        assert!(
            !Backend::supports_child_splice(&b),
            "old-core recorder is anchored too — if this changes, the wire protocol \
             needs RemoveChild/InsertAt ops before either leg may splice"
        );
    }
}
