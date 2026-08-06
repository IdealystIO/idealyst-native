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

    /// Apply per-instance rules as an INLINE style, layered ON TOP of any
    /// classes the node already carries — the backend half of
    /// [`StyleApplication::with_inline`].
    ///
    /// Distinct from [`Self::apply_style`], which on a class-model backend
    /// REPLACES the node's style class. This one must not disturb the
    /// stamped preminted classes; it only sets the properties it is given,
    /// and they must win over those classes (on web that is what an inline
    /// `style` attribute already does).
    ///
    /// Only ever called on backends that report
    /// [`Self::supports_preminted_styles`], because that is the only path
    /// where classes and per-instance values are applied separately —
    /// everywhere else the live engine folds the inline layer into the
    /// resolved rules before `apply_style`. The default therefore panics
    /// rather than silently dropping the values: a backend that opts into
    /// preminted styles owns this too.
    #[allow(unused_variables)]
    fn apply_inline_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        unreachable!(
            "apply_inline_style reached a backend that does not override it; \
             a backend reporting supports_preminted_styles() = true must \
             implement it, and one reporting false should never receive an \
             inline layer (the engine folds it in during resolve)"
        )
    }

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
    ///
    /// # This default is a TRAP on any backend that reports
    /// [`handles_states_natively`](Self::handles_states_natively) `== false`
    ///
    /// `false` — the default, and the correct answer for every backend
    /// without a CSS pseudo-class layer — selects the **event-driven**
    /// path: the framework builds a per-node state signal, hands you the
    /// setter, and waits for you to call it. Not overriding this method
    /// compiles, renders every node's BASE style correctly, and lights no
    /// `state hover` / `state pressed` / `state focused` variant *ever*.
    /// Nothing warns. The symptom is a UI where hover highlights, press
    /// feedback and focus rings simply do not exist — which reads as a
    /// styling bug, not a missing capability. GTK shipped exactly this.
    ///
    /// Coverage as of this writing — `-` means the bit is never flipped:
    ///
    /// | backend  | HOVERED | PRESSED | FOCUSED | notes |
    /// |----------|---------|---------|---------|-------|
    /// | web, ssr | n/a     | n/a     | n/a     | `handles_states_natively() == true`; CSS owns it |
    /// | macos    | yes     | yes     | yes     | tracking area + mouseDown/Up |
    /// | linux    | yes     | yes     | yes     | `EventControllerMotion` / `GestureClick` / `EventControllerFocus` |
    /// | android  | n/a     | yes     | yes     | no hover on a touch device — deliberate |
    /// | ios      | n/a     | **-**   | yes     | only the text-field focus setter is wired |
    /// | windows  | **-**   | **-**   | **-**   | not overridden at all |
    /// | terminal, cpu, roku | **-** | **-** | **-** | not overridden at all |
    ///
    /// `DISABLED` is not in the table: it is not input-driven. The
    /// framework sets it from the author's `disabled` prop and routes the
    /// native side through [`set_disabled`](Self::set_disabled).
    ///
    /// # Lifetime: the setter is already scope-guarded
    ///
    /// `setter` writes a signal owned by the node's reactive scope, and
    /// native input machinery routinely outlives that scope — a toolkit
    /// emits focus-leave *while* the framework unparents a focused widget.
    /// You do **not** need to guard against that: the framework hands over
    /// a setter that is inert once its scope drops
    /// (`runtime_vocabulary::callback_guard`), so a late call is a no-op
    /// rather than a stale-signal panic inside a non-unwinding C
    /// trampoline (which aborts the process). The same holds for every
    /// author callback a backend is given.
    ///
    /// Detaching your native observers on
    /// [`on_node_unstyled`](Self::on_node_unstyled) is still worthwhile —
    /// better not to deliver at all than to deliver into a guard — but it
    /// is an optimization, not a correctness requirement.
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
