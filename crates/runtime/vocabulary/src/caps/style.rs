//! Styling (resolved rules, class minting, state/breakpoint/container
//! overlays, tokens, interaction states) and static assets.

use std::rc::Rc;

use runtime_shared::assets::{AssetId, AssetSource, AssetTag, SystemFallback, TypefaceFace, TypefaceId};
use runtime_shared::breakpoint::Breakpoint;
use runtime_shared::{FontFamily, StateBits, StyleApplication, StyleRules, TokenEntry};
use runtime_scene::Host;

/// The style engine's backend surface: applying resolved rules, class
/// minting for the batched/signal-class paths, declarative state +
/// breakpoint + container overlays, token variables, interaction-state
/// wiring, and per-node style teardown. Serves `walker/style.rs` (plus
/// the style bits of `walker/view.rs` and `walker/theme_cohort.rs`).
pub trait StyleOps: Host {
    /// Apply resolved, concrete `StyleRules` to a node.
    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>);

    /// Mint (or look up) a backend-side class for a resolved style
    /// without touching any node (batched-Repeat path). `None` = no
    /// named-class model; the walker falls back to per-call applies.
    #[allow(unused_variables)]
    fn mint_style_class(&mut self, style: &Rc<StyleRules>) -> Option<String> {
        None
    }

    /// Mint a class for a `StyleApplication` (SignalClass path); may
    /// mint fresh dynamic classes, unlike
    /// [`mint_style_class`](Self::mint_style_class).
    #[allow(unused_variables)]
    fn mint_class_for_app(&mut self, app: &StyleApplication) -> Option<String> {
        None
    }

    /// Apply a base style plus per-state overlays declaratively (only
    /// reached when [`handles_states_natively`](Self::handles_states_natively)
    /// is `true`). Default applies just the base.
    fn apply_styled_states(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        #[allow(unused_variables)] overlays: &[(StateBits, Rc<StyleRules>)],
    ) {
        // Default: just apply the base style. Mobile backends drive
        // state overlays via signal-flip → re-resolve → apply_style.
        self.apply_style(node, base);
    }

    /// Superset of [`apply_styled_states`](Self::apply_styled_states)
    /// adding breakpoint + container-query overlay axes. A backend
    /// reporting `handles_states_natively() == true` MUST handle the
    /// extra overlays here; the default drops them and delegates.
    fn apply_styled_variants(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        state_overlays: &[(StateBits, Rc<StyleRules>)],
        #[allow(unused_variables)] breakpoint_overlays: &[(Breakpoint, Rc<StyleRules>)],
        #[allow(unused_variables)] container_overlays: &[(f32, Rc<StyleRules>)],
    ) {
        self.apply_styled_states(node, base, state_overlays);
    }

    /// Mark `node` as a container-query containment context.
    #[allow(unused_variables)]
    fn mark_container(&mut self, node: &Self::Node) {}

    /// `true` = receive state overlays declaratively (CSS
    /// pseudo-classes); `false` = event-driven `attach_states` path.
    fn handles_states_natively(&self) -> bool {
        false
    }

    /// `true` when `update_tokens` propagates via a cascade (CSS
    /// `var()`) with no per-node re-apply.
    fn token_updates_propagate_via_cascade(&self) -> bool {
        false
    }

    /// Pre-generate backend state for a stylesheet's resolved rules.
    #[allow(unused_variables)]
    fn register_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        // default: no-op
    }

    /// Release a stylesheet's pre-generated state.
    #[allow(unused_variables)]
    fn unregister_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        // default: no-op
    }

    /// Install the initial token set as runtime variables.
    #[allow(unused_variables)]
    fn install_tokens(&mut self, tokens: &[TokenEntry]) {
        // default: no-op
    }

    /// Push updated token values.
    #[allow(unused_variables)]
    fn update_tokens(&mut self, tokens: &[TokenEntry]) {
        // default: no-op
    }

    /// A styled node is being torn down; free per-node style state.
    #[allow(unused_variables)]
    fn on_node_unstyled(&mut self, node: &Self::Node) {
        // default: no-op
    }

    /// Wire native interaction events (hover/press/focus) to the
    /// framework's per-node state signal via `setter(state, on)`.
    #[allow(unused_variables)]
    fn attach_states(&mut self, node: &Self::Node, setter: Rc<dyn Fn(StateBits, bool)>) {
        // default: no-op
    }

    /// Mark the native widget inert (distinct from the DISABLED style
    /// bit).
    #[allow(unused_variables)]
    fn set_disabled(&mut self, node: &Self::Node, disabled: bool) {
        // default: no-op
    }

    /// `true` when the backend can realize `StyleSource::Preminted`
    /// classes against a shipped `.css` asset (web, SSR).
    fn supports_preminted_styles(&self) -> bool {
        false
    }

    /// Publish the theme's default text font at the document level
    /// (the `--iy-default-font` variable preminted classes reference).
    #[allow(unused_variables)]
    fn apply_default_text_font(&mut self, font: Option<&FontFamily>) {}

    /// Capability flag for backend-side signal→class bindings.
    fn supports_js_class_bindings(&self) -> bool {
        false
    }

    /// Register a pre-resolved signal→class binding; returns a
    /// `binding_id` for release on scope teardown.
    fn register_reactive_class_binding(
        &mut self,
        _node: &Self::Node,
        _signal_id: u64,
        _values: &[u32],
        _classes: &[&str],
        _value_reader: std::rc::Rc<dyn Fn() -> u32>,
    ) -> u32 {
        unreachable!(
            "register_reactive_class_binding called without an override; \
             a backend that returns true from supports_js_class_bindings must \
             also override register_reactive_class_binding"
        )
    }

    /// Release a binding registered via
    /// [`register_reactive_class_binding`](Self::register_reactive_class_binding).
    fn release_reactive_class_binding(&mut self, _binding_id: u32) {}

    /// NEW-CORE-ONLY channel (not part of the frozen 159-method
    /// `Backend` mirror — no old-core counterpart exists): ship a
    /// `(signal_id, new_value)` change to the backend's JS-side
    /// binding dispatcher. On the old core the arena's `Signal::set`
    /// fires the registered JS notifier itself; world signals have no
    /// write hook, so the vocabulary's per-signal notifier effect
    /// (see `style_attach::ensure_signal_notifier`) delivers commits
    /// through this method instead. Default no-op — only backends
    /// returning `true` from
    /// [`supports_js_class_bindings`](Self::supports_js_class_bindings)
    /// need to override.
    fn notify_signal_value_js(&mut self, _signal_id: u64, _value: u32) {}
}

/// Static assets (fonts, images) and typeface families. Serves the
/// asset-registration effects in `walker/style.rs`, `walker/view.rs`,
/// and `walker/image.rs`.
pub trait AssetOps: Host {
    /// Make a static asset available to the renderer (deduped by id).
    #[allow(unused_variables)]
    fn register_asset(&mut self, id: AssetId, kind: AssetTag, source: &AssetSource) {
        // default: no-op
    }

    /// Release a previously-registered asset.
    #[allow(unused_variables)]
    fn unregister_asset(&mut self, id: AssetId, kind: AssetTag) {
        // default: no-op
    }

    /// Register a font family; every referenced face asset is already
    /// registered in the same flush.
    #[allow(unused_variables)]
    fn register_typeface(
        &mut self,
        id: TypefaceId,
        family_name: &str,
        faces: &[TypefaceFace],
        fallback: SystemFallback,
    ) {
        // default: no-op
    }

    /// Release a previously-registered typeface.
    #[allow(unused_variables)]
    fn unregister_typeface(&mut self, id: TypefaceId) {
        // default: no-op
    }
}
