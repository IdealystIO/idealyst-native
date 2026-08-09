//! New-core adoption for the wire-recording dev backend (idea-lite
//! migration — the dev-session wire chain).
//!
//! Implements [`runtime_scene::Host`] plus **all 30** capability traits
//! (`runtime_vocabulary::caps`) directly on [`WireRecordingBackend`] —
//! the same shape every shipping backend took (see
//! `backend-ssr/src/newcore.rs`, the template this file's delegation
//! bodies are generated from). Every trait method delegates via UFCS
//! (`WireRecordingBackend::method(self, …)`) to the
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
//! `runtime_shared::mount(recorder, app)` call the sidecar makes: fresh
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
//! wrapper enables `runtime-core/dev` + `runtime-core/dev` so the
//! emission gate, the `__mcp` anchors, and the bridge's `get_catalog`
//! verb are all live (build-runtime-server pins this).
//!
//! # What the new-core session does NOT yet do (each named, none silent)
//!
//! - **Identity-keyed node dedup across re-mounts.** The old walker set
//!   `runtime_shared::current_identity()` before every `create_*`, which
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

use runtime_shared::accessibility::{AccessibilityProps, LiveRegionPriority, Role};
use runtime_shared::assets::{
    AssetId, AssetSource, AssetTag, SystemFallback, TypefaceFace, TypefaceId,
};
use runtime_shared::primitives;
use runtime_shared::primitives::portal::ViewportRect;
use runtime_shared::{
    Action, Color, ColorScheme, Easing, PageMetadata, SafeAreaSides, StateBits, StyleRules, TokenEntry, Tokenized, VirtualizerCallbacks,
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
        WireRecordingBackend::finish(&mut *backend.borrow_mut(), root);

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
    type Node = wire::NodeId;

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        WireRecordingBackend::insert(self, parent, child)
    }

    fn insert_many(&mut self, parent: &mut Self::Node, children: Vec<Self::Node>) {
        WireRecordingBackend::insert_many(self, parent, children)
    }

    /// APPEND — the index is deliberately ignored, reproducing the body
    /// the recorder inherited from the old `Backend` default
    /// (`self.insert(parent, child)`). The wire protocol has no
    /// positional-insert op, and `supports_splice` is hard `false`, so
    /// no driver ever asks for a positional insert on this host: the
    /// only callers are structural paths that already appended. Kept as
    /// an explicit body rather than a trait default so the behavior is
    /// visible next to the `supports_splice` invariant.
    fn insert_at(&mut self, parent: &mut Self::Node, child: Self::Node, _index: usize) {
        WireRecordingBackend::insert(self, parent, child)
    }

    /// NO-OP — reproducing the old `Backend` default. Same reason as
    /// [`Self::insert_at`]: the wire protocol has no `RemoveChild`, and
    /// anchored regions detach by `ClearChildren` on their anchor
    /// instead. A silent no-op is correct here and wrong on a splicing
    /// host, which is exactly what `supports_splice() == false` asserts.
    fn remove_child(&mut self, _parent: &Self::Node, _child: &Self::Node) {}

    fn clear_children(&mut self, node: &Self::Node) {
        WireRecordingBackend::clear_children(self, node)
    }

    fn create_anchor(&mut self) -> Self::Node {
        WireRecordingBackend::create_reactive_anchor(self)
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
        WireRecordingBackend::color_scheme(self)
    }




    fn set_page_metadata(&mut self, meta: &PageMetadata) {
        WireRecordingBackend::set_page_metadata(self, meta)
    }

    fn set_app_background(&mut self, color: &Tokenized<Color>) {
        WireRecordingBackend::set_app_background(self, color)
    }

    fn set_scrollbar_theme(&mut self, thumb: &Tokenized<Color>, track: &Tokenized<Color>) {
        WireRecordingBackend::set_scrollbar_theme(self, thumb, track)
    }

}

impl caps::LifecycleOps for WireRecordingBackend {
    fn finish(&mut self, root: Self::Node) {
        WireRecordingBackend::finish(self, root)
    }




}

// ===========================================================================
// View + input + pressable
// ===========================================================================

impl caps::ViewOps for WireRecordingBackend {
    fn create_view(&mut self, a11y: &AccessibilityProps) -> Self::Node {
        WireRecordingBackend::create_view(self, a11y)
    }

    fn make_view_handle(&self, node: &Self::Node) -> runtime_shared::ViewHandle {
        WireRecordingBackend::make_view_handle(self, node)
    }
}

impl caps::InputOps for WireRecordingBackend {





}

impl caps::PressableOps for WireRecordingBackend {
    fn create_pressable(&mut self, on_click: Rc<dyn Fn()>, a11y: &AccessibilityProps) -> Self::Node {
        WireRecordingBackend::create_pressable(self, on_click, a11y)
    }

}

// ===========================================================================
// Text + button
// ===========================================================================

impl caps::TextOps for WireRecordingBackend {
    fn create_text(&mut self, content: &str, a11y: &AccessibilityProps) -> Self::Node {
        WireRecordingBackend::create_text(self, content, a11y)
    }



    fn update_text(&mut self, node: &Self::Node, content: &str) {
        WireRecordingBackend::update_text(self, node, content)
    }







    fn make_text_handle(&self, node: &Self::Node) -> runtime_shared::TextHandle {
        WireRecordingBackend::make_text_handle(self, node)
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
        WireRecordingBackend::create_button(self, label, on_click, leading_icon, trailing_icon, a11y)
    }

    fn update_button_label(&mut self, node: &Self::Node, label: &str) {
        WireRecordingBackend::update_button_label(self, node, label)
    }

}

// ===========================================================================
// Image + icon + link
// ===========================================================================

impl caps::ImageOps for WireRecordingBackend {
    fn create_image(&mut self, src: &str, alt: Option<&str>, a11y: &AccessibilityProps) -> Self::Node {
        WireRecordingBackend::create_image(self, src, alt, a11y)
    }

    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        WireRecordingBackend::update_image_src(self, node, src)
    }

    fn update_image_alt(&mut self, node: &Self::Node, alt: Option<&str>) {
        WireRecordingBackend::update_image_alt(self, node, alt)
    }



}

impl caps::IconOps for WireRecordingBackend {
    fn create_icon(
        &mut self,
        data: &primitives::icon::IconData,
        color: Option<&Color>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        WireRecordingBackend::create_icon(self, data, color, a11y)
    }

    fn update_icon_color(&mut self, node: &Self::Node, color: &Color) {
        WireRecordingBackend::update_icon_color(self, node, color)
    }

    fn update_icon_data(&mut self, node: &Self::Node, data: &primitives::icon::IconData) {
        WireRecordingBackend::update_icon_data(self, node, data)
    }

    fn update_icon_stroke(&mut self, node: &Self::Node, progress: f32) {
        WireRecordingBackend::update_icon_stroke(self, node, progress)
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
        WireRecordingBackend::animate_icon_stroke(self, node, from, to, duration_ms, easing, infinite, autoreverses)
    }

}

impl caps::LinkOps for WireRecordingBackend {
    fn create_link(
        &mut self,
        config: primitives::link::LinkConfig,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        WireRecordingBackend::create_link(self, config, a11y)
    }

    fn update_link_url(&mut self, node: &Self::Node, url: &str) {
        WireRecordingBackend::update_link_url(self, node, url)
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
        WireRecordingBackend::create_text_input(
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
        WireRecordingBackend::update_text_input_value(self, node, value)
    }

    fn update_text_input_secure(&mut self, node: &Self::Node, secure: bool) {
        WireRecordingBackend::update_text_input_secure(self, node, secure)
    }


    fn update_text_input_placeholder(&mut self, node: &Self::Node, placeholder: Option<&str>) {
        WireRecordingBackend::update_text_input_placeholder(self, node, placeholder)
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
        WireRecordingBackend::create_text_area(
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
        WireRecordingBackend::update_text_area_value(self, node, value)
    }


}

impl caps::ToggleOps for WireRecordingBackend {
    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        WireRecordingBackend::create_toggle(self, initial_value, on_change, a11y)
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        WireRecordingBackend::update_toggle_value(self, node, value)
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
        WireRecordingBackend::create_slider(self, initial_value, min, max, step, on_change, a11y)
    }

    fn update_slider_value(&mut self, node: &Self::Node, value: f32) {
        WireRecordingBackend::update_slider_value(self, node, value)
    }

}

impl caps::ActivityIndicatorOps for WireRecordingBackend {
    fn create_activity_indicator(
        &mut self,
        size: primitives::activity_indicator::ActivityIndicatorSize,
        color: Option<&Color>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        WireRecordingBackend::create_activity_indicator(self, size, color, a11y)
    }

    fn update_activity_indicator_size(
        &mut self,
        node: &Self::Node,
        size: primitives::activity_indicator::ActivityIndicatorSize,
    ) {
        WireRecordingBackend::update_activity_indicator_size(self, node, size)
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
        WireRecordingBackend::create_scroll_view(self, horizontal, on_scroll, a11y)
    }



}

impl caps::SafeAreaOps for WireRecordingBackend {
    fn apply_safe_area_padding(&mut self, node: &Self::Node, sides: SafeAreaSides) {
        WireRecordingBackend::apply_safe_area_padding(self, node, sides)
    }

    fn apply_scroll_view_safe_area_inset(&mut self, node: &Self::Node, sides: SafeAreaSides) {
        WireRecordingBackend::apply_scroll_view_safe_area_inset(self, node, sides)
    }
}

// No two-axis grid engine on this backend yet; every `GridOps`
// method defaults, so `virtual_grid` reports itself as an
// unsupported primitive instead of silently rendering nothing.
impl caps::GridOps for WireRecordingBackend {}

impl caps::VirtualizerOps for WireRecordingBackend {
    fn create_virtualizer(
        &mut self,
        callbacks: VirtualizerCallbacks<Self::Node>,
        overscan: f32,
        layout: primitives::virtualizer::VirtualLayout,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        WireRecordingBackend::create_virtualizer(self, callbacks, overscan, layout, a11y)
    }

    fn virtualizer_data_changed(&mut self, node: &Self::Node) {
        WireRecordingBackend::virtualizer_data_changed(self, node)
    }

    fn release_virtualizer(&mut self, node: &Self::Node) {
        WireRecordingBackend::release_virtualizer(self, node)
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
        WireRecordingBackend::create_graphics(self, on_ready, on_resize, on_lost, a11y)
    }

    fn release_graphics(&mut self, node: &Self::Node) {
        WireRecordingBackend::release_graphics(self, node)
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
        WireRecordingBackend::create_portal(self, target, on_dismiss, trap_focus, a11y)
    }

    fn release_portal(&mut self, node: &Self::Node) {
        WireRecordingBackend::release_portal(self, node)
    }


}

impl caps::PresenceOps for WireRecordingBackend {

    fn apply_presence(
        &mut self,
        node: &Self::Node,
        state: primitives::presence::PresenceState,
        transition: Option<(u32, Easing)>,
    ) {
        WireRecordingBackend::apply_presence(self, node, state, transition)
    }

}

impl caps::NavigatorOps for WireRecordingBackend {




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
        WireRecordingBackend::create_external(self, type_id, type_name, payload, a11y)
    }

    fn release_external(&mut self, node: &Self::Node) {
        WireRecordingBackend::release_external(self, node)
    }

}

impl caps::DocumentOps for WireRecordingBackend {




    fn register_raw_css(&mut self, css: &str) {
        WireRecordingBackend::register_raw_css(self, css)
    }
}

// ===========================================================================
// Style + assets
// ===========================================================================

impl caps::StyleOps for WireRecordingBackend {
    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        WireRecordingBackend::apply_style(self, node, style)
    }



    fn apply_styled_states(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        overlays: &[(StateBits, Rc<StyleRules>)],
    ) {
        WireRecordingBackend::apply_styled_states(self, node, base, overlays)
    }





    fn register_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        WireRecordingBackend::register_stylesheet(self, rules)
    }

    fn unregister_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        WireRecordingBackend::unregister_stylesheet(self, rules)
    }

    fn install_tokens(&mut self, tokens: &[TokenEntry]) {
        WireRecordingBackend::install_tokens(self, tokens)
    }

    fn update_tokens(&mut self, tokens: &[TokenEntry]) {
        WireRecordingBackend::update_tokens(self, tokens)
    }

    fn on_node_unstyled(&mut self, node: &Self::Node) {
        WireRecordingBackend::on_node_unstyled(self, node)
    }

    fn attach_states(&mut self, node: &Self::Node, setter: Rc<dyn Fn(StateBits, bool)>) {
        WireRecordingBackend::attach_states(self, node, setter)
    }

    fn set_disabled(&mut self, node: &Self::Node, disabled: bool) {
        WireRecordingBackend::set_disabled(self, node, disabled)
    }





}

impl caps::AssetOps for WireRecordingBackend {
    fn register_asset(&mut self, id: AssetId, kind: AssetTag, source: &AssetSource) {
        WireRecordingBackend::register_asset(self, id, kind, source)
    }

    fn unregister_asset(&mut self, id: AssetId, kind: AssetTag) {
        WireRecordingBackend::unregister_asset(self, id, kind)
    }

    fn register_typeface(
        &mut self,
        id: TypefaceId,
        family_name: &str,
        faces: &[TypefaceFace],
        fallback: SystemFallback,
    ) {
        WireRecordingBackend::register_typeface(self, id, family_name, faces, fallback)
    }

    fn unregister_typeface(&mut self, id: TypefaceId) {
        WireRecordingBackend::unregister_typeface(self, id)
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
        WireRecordingBackend::update_accessibility(self, node, a11y, inferred_role)
    }

    fn announce_for_accessibility(&mut self, msg: &str, priority: LiveRegionPriority) {
        WireRecordingBackend::announce_for_accessibility(self, msg, priority)
    }

}

impl caps::AnimationOps for WireRecordingBackend {

}

impl caps::IntrospectionOps for WireRecordingBackend {


    fn device_frame(&self, node: &Self::Node) -> Option<ViewportRect> {
        WireRecordingBackend::device_frame(self, node)
    }





}

// ===========================================================================
// Batch + wire bindings
// ===========================================================================

impl caps::BatchOps for WireRecordingBackend {


}

impl caps::WireBindingOps for WireRecordingBackend {








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
    runtime_shared::robot::bridge::install_verb_router(|cmd, args| {
        match runtime_vocabulary::robot::bridge::invoke_command(cmd, args) {
            Err(e) if e.starts_with("unknown command:") => None,
            other => Some(other),
        }
    });
}

/// Tear down what [`install_robot_env`] installed (session shutdown /
/// tests that boot repeatedly on one thread).
pub fn clear_robot_env() {
    runtime_shared::robot::bridge::clear_verb_router();
    runtime_vocabulary::robot::clear_driver_env();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// REGRESSION: the recorder's `Host::supports_splice` must be
    /// `false` — the wire protocol has no `RemoveChild`/`InsertAt` ops,
    /// so a splice can't be expressed to replay clients (module docs,
    /// "the recorder is ANCHORED"). The assertion is a literal, not a
    /// delegation: nothing upstream can flip it by accident.
    #[test]
    fn newcore_recorder_is_anchored() {
        let b = WireRecordingBackend::new();
        assert!(
            !Host::supports_splice(&b),
            "the wire recorder must stay anchored: the protocol cannot express splices"
        );
    }

    /// REGRESSION: `Host::insert_at` APPENDS (index ignored) and
    /// `Host::remove_child` is a no-op — the two Host-required methods
    /// the recorder used to inherit as `Backend` trait defaults. Both
    /// are now explicit bodies; this pins that they still behave the way
    /// the wire protocol requires (no positional-insert op, no
    /// remove-child op) rather than silently acquiring some other
    /// default.
    #[test]
    fn newcore_recorder_insert_at_appends_and_remove_child_is_inert() {
        let mut b = WireRecordingBackend::new();
        let mut parent = WireRecordingBackend::create_view(&mut b, &AccessibilityProps::default());
        let a = WireRecordingBackend::create_text(&mut b, "a", &AccessibilityProps::default());
        let c = WireRecordingBackend::create_text(&mut b, "b", &AccessibilityProps::default());
        b.drain_commands();

        Host::insert_at(&mut b, &mut parent, a, 0);
        Host::insert_at(&mut b, &mut parent, c, 0);
        Host::remove_child(&mut b, &parent, &a);

        let cmds = b.drain_commands();
        let inserts: Vec<_> = cmds
            .iter()
            .filter_map(|c| match c {
                wire::Command::Insert { parent, child } => Some((*parent, *child)),
                _ => None,
            })
            .collect();
        assert_eq!(
            inserts,
            vec![(parent, a), (parent, c)],
            "insert_at must append in call order, ignoring the index"
        );
        assert!(
            !cmds.iter().any(|c| !matches!(c, wire::Command::Insert { .. })),
            "remove_child must emit nothing: the wire protocol has no RemoveChild op \
             (got {cmds:?})"
        );
    }
}
