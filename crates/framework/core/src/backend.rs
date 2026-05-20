//! The `Backend` trait — every renderer (web DOM, Android views, iOS
//! UIKit, etc.) implements this. Plus the `VirtualizerCallbacks`
//! bundle the framework hands to backends for virtualized lists,
//! and the no-op `Ops` implementations the trait's default methods
//! use for un-implemented primitives.
//!
//! The trait is intentionally large — one method per primitive +
//! lifecycle hook — but most methods have `unimplemented!()` or
//! no-op defaults so backends can ship incrementally. The walker
//! in [`crate`] is the only caller.

use std::any::Any;
use std::rc::Rc;

use crate::assets::{AssetId, AssetSource, AssetTag, SystemFallback, TypefaceFace, TypefaceId};
use crate::primitives;
use crate::style::{Color, StyleRules};
use crate::{
    ButtonHandle, ButtonOps, PressableHandle, PressableOps, StateBits, TextHandle, TextOps,
    ViewHandle, ViewOps,
};

// ---------------------------------------------------------------------------
// VirtualizerCallbacks
// ---------------------------------------------------------------------------

/// Callbacks handed to `Backend::create_virtualizer`. All Rc'd so
/// the backend can clone into per-event closures (scroll handler,
/// cell binder, etc.). Generic over the backend's `Node` type so
/// the mount callback returns the backend's actual native node
/// type, no type erasure.
pub struct VirtualizerCallbacks<N: Clone + 'static> {
    /// Current item count. Backend calls this on data-changed.
    pub item_count: Rc<dyn Fn() -> usize>,
    /// Stable identity for an index. Backend uses this to do
    /// keyed diffs across data updates.
    pub item_key: Rc<dyn Fn(usize) -> primitives::virtualizer::ItemKey>,
    /// Initial size for an index (Known: authoritative;
    /// Measured: estimate). For Measured mode, the backend should
    /// observe the rendered size after mount and update its
    /// internal layout when the value changes.
    pub item_size: Rc<dyn Fn(usize) -> f32>,
    /// True if `item_size` is an estimate that should be refined
    /// by measuring the mounted node. False if the size is
    /// authoritative.
    pub measure_sizes: bool,
    /// Mount an item: build its subtree inside a fresh per-item
    /// Scope. Returns the freshly-built native node plus the
    /// scope's id. The backend should hold the id alongside its
    /// pooled/mounted cell so it can call `release_item` later.
    pub mount_item: Rc<dyn Fn(usize) -> (N, u64)>,
    /// Release a previously-mounted item by scope id. Drops the
    /// scope, freeing every signal/effect/ref inside the item's
    /// subtree. Backend should NOT try to use the node after this;
    /// it should also detach the node from its parent.
    pub release_item: Rc<dyn Fn(u64)>,
    /// Backend may call this to inform the framework that an
    /// observed item's measured size has changed (Measured mode).
    /// The framework stores the new size and the backend uses it
    /// for future layout passes.
    pub set_measured_size: Rc<dyn Fn(u64, f32)>,
}

// ---------------------------------------------------------------------------
// ColorScheme
// ---------------------------------------------------------------------------

/// The platform's current appearance mode. Backends return this from
/// [`Backend::color_scheme`] so the app can pick an appropriate
/// default theme before the first render.
///
/// `Auto` means the platform has no explicit preference (e.g. iOS
/// `UIUserInterfaceStyleUnspecified`, or the browser has no
/// `prefers-color-scheme` media query match). Apps should fall back
/// to whichever theme they consider the default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorScheme {
    Light,
    Dark,
    /// The platform did not report an explicit preference.
    Auto,
}

// ---------------------------------------------------------------------------
// Backend trait
// ---------------------------------------------------------------------------

pub trait Backend {
    type Node: Clone;

    /// Returns the platform's current color scheme. Called before the
    /// first render so the app can select a matching default theme.
    /// Defaults to `ColorScheme::Auto` (no preference).
    fn color_scheme(&self) -> ColorScheme {
        ColorScheme::Auto
    }

    fn create_view(&mut self) -> Self::Node;
    fn create_text(&mut self, content: &str) -> Self::Node;
    fn create_button(
        &mut self,
        label: &str,
        on_click: &crate::derive::Action,
        leading_icon: Option<&primitives::icon::IconData>,
        trailing_icon: Option<&primitives::icon::IconData>,
    ) -> Self::Node;
    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node);

    /// Tappable container node with a click handler attached. Used
    /// by [`Primitive::Pressable`]. Children are inserted into this
    /// node via the regular `insert` path.
    ///
    /// Default impl falls back to `create_view` — appropriate for
    /// backends that don't yet support pressables (clicks won't
    /// fire, but the subtree still renders). Web overrides with a
    /// `<div>` that has `cursor: pointer` and an `onclick` handler.
    #[allow(unused_variables)]
    fn create_pressable(&mut self, on_click: Rc<dyn Fn()>) -> Self::Node {
        self.create_view()
    }

    /// Install a raw touch handler on `node`. The framework calls this
    /// once per `Primitive::View { on_touch: Some(_), .. }` (and any
    /// other primitive that grows a touch slot in the future) after
    /// the node is created.
    ///
    /// The backend's job is to wire `handler` to whatever native touch
    /// delivery mechanism it uses (UIView subclass + `touchesBegan:`,
    /// Android `OnTouchListener`, winit pointer events, web Pointer
    /// Events) and invoke it for every event hitting this node, with
    /// the event already translated into framework coordinates.
    ///
    /// Default impl is a no-op — appropriate for backends that don't
    /// yet support raw touch. Subscribed views still render; they just
    /// receive no events. See `docs/native-touch-plan.md` for the
    /// design and the per-platform implementation notes.
    #[allow(unused_variables)]
    fn install_touch_handler(
        &mut self,
        node: &Self::Node,
        handler: crate::TouchHandler,
    ) {
        // default: no-op
    }

    /// Called when a handler returns
    /// [`TouchResponse { claim: true, .. }`](crate::TouchResponse).
    /// The backend decides locally how to suppress competing native
    /// consumers of this touch — parent scroll containers, system
    /// gestures, pointer-capture, etc. The framework does not enumerate
    /// or know about those mechanisms; they are implementation-private
    /// to each backend.
    ///
    /// Default impl is a no-op. Backends that don't implement the
    /// claim protocol will see scroll containers win contested touches.
    #[allow(unused_variables)]
    fn claim_touch(&mut self, node: &Self::Node, touch_id: crate::TouchId) {
        // default: no-op
    }

    /// Placeholder node for reactive `when` / `switch` branches.
    /// The walker creates one of these as a stable parent that
    /// stays put across branch swaps, with the live branch's
    /// children re-inserted on each rebuild.
    ///
    /// On web the anchor needs to be layout-transparent
    /// (`display: contents`) so the branch's children inherit the
    /// surrounding flex / sizing context — otherwise an extra
    /// `<div>` collapses widths and breaks `flex: 1` / `width:
    /// 100%` on full-width children. Native backends have no such
    /// problem; the default `create_view` is fine.
    fn create_reactive_anchor(&mut self) -> Self::Node {
        self.create_view()
    }

    /// Batched insertion of many siblings into `parent`. Default
    /// implementation falls back to N `insert` calls — backends
    /// override this to collapse N FFI crossings into one (e.g.
    /// web uses a `DocumentFragment` to push 10 000 children in
    /// a single `appendChild` call). Called by the build walker
    /// when it expands a `Primitive::Repeat` produced by `ui!`'s
    /// `for` lowering.
    fn insert_many(&mut self, parent: &mut Self::Node, children: Vec<Self::Node>) {
        for child in children {
            self.insert(parent, child);
        }
    }

    /// Backend capability flag for the local-render batched-Repeat
    /// path. When `true`, the walker collapses `Primitive::Repeat`
    /// expansions whose rows match the batchable shape (static
    /// View+Text+style — see [`crate::BackendBatch`]) into one
    /// [`execute_batch`](Self::execute_batch) call. When `false`,
    /// the walker uses the existing per-call path: one
    /// `create_view`/`create_text`/`apply_style`/`insert` chain per
    /// row.
    ///
    /// Web backend opts in for the rebuild benchmark's pattern.
    /// Native backends keep the per-call path — their FFI cost per
    /// call is already small and the batching benefit doesn't pay
    /// for the encoding/decoding overhead. Default `false`.
    fn supports_batched_repeat(&self) -> bool {
        false
    }

    /// Execute a queued [`BackendBatch`] in a single round-trip and
    /// return the materialized nodes, indexed by `local_id`.
    ///
    /// The walker submits this when expanding a `Primitive::Repeat`
    /// whose rows are all batchable (static View+Text trees with
    /// static styles). On the web backend this turns ~4N FFI calls
    /// (createElement, createTextNode, setAttribute, appendChild ×
    /// N) into a single wasm→JS call carrying the whole op stream.
    ///
    /// The returned `Vec`'s length must equal
    /// `batch.node_count as usize`. Element at index `i` is the node
    /// that corresponds to `local_id == i`.
    ///
    /// Backends that don't implement batching keep the default
    /// `unimplemented!()` — the walker only calls this when
    /// [`supports_batched_repeat`](Self::supports_batched_repeat)
    /// returned `true`.
    #[allow(unused_variables)]
    fn execute_batch(&mut self, batch: crate::BackendBatch) -> Vec<Self::Node> {
        unimplemented!(
            "execute_batch is only called when supports_batched_repeat() returns true; \
             this backend opted in without implementing it"
        )
    }

    fn update_text(&mut self, node: &Self::Node, content: &str);

    /// Optional hook the walker calls when a `Primitive::Text`'s
    /// source is `TextSource::Bound`. Backends with declarative wire
    /// formats override this to record `signal_ids` + the
    /// transformer `method` name so they can ship the binding to a
    /// remote renderer instead of running the closure locally on
    /// every change.
    ///
    /// Effect-driven backends leave the default no-op in place — the
    /// walker still sets up an `Effect` around the binding's closure
    /// on every backend, which is what those backends rely on. The
    /// metadata is only consumed by backends that need it.
    #[allow(unused_variables)]
    fn note_text_binding(
        &mut self,
        node: &Self::Node,
        signal_ids: &[u64],
        method: &'static str,
    ) {
        // default: no-op
    }

    /// Optional hook the walker calls (once per signal per binding)
    /// when it encounters a `TextSource::Bound`. Backends that need
    /// to ship signal state across a wire boundary use this to
    /// declare each signal's existence + initial value to the remote
    /// renderer. Backends that read live signal values directly from
    /// the framework's arena leave the default no-op in place.
    ///
    /// Backends are expected to dedupe internally — the walker will
    /// call this for the *same* signal_id across multiple bindings
    /// if more than one binding reads that signal. Only the first
    /// observation needs to ship a value declaration to the wire.
    #[allow(unused_variables)]
    fn note_signal_initial(
        &mut self,
        signal_id: u64,
        value: &crate::__serde_json::Value,
    ) {
        // default: no-op
    }

    /// Optional hook the walker calls after building both branches
    /// of a `Primitive::When` declaratively. Backends record the
    /// signal IDs the condition reads, the name of the boolean
    /// transformer (`#[method]`) that decides which branch is
    /// active, and the node ids of the then/otherwise subtrees so
    /// the remote runtime can toggle their visibility on signal
    /// change. Only called when `handles_when_natively()` returns
    /// true and the `When` carries a `bind_when!`-produced binding.
    #[allow(unused_variables)]
    fn note_when_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        cond_method: &'static str,
        then_node: &Self::Node,
        otherwise_node: &Self::Node,
    ) {
        // default: no-op
    }

    /// Optional hook the walker calls after building every arm +
    /// default of a `Primitive::Switch` on the lazy-slot-capture
    /// path. `arms` carries each arm's `(pattern_value, node)` pair
    /// so the remote runtime can compare the discriminant's value
    /// against the pattern and play / tear down the matching arm.
    #[allow(unused_variables)]
    fn note_switch_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        cond_method: &'static str,
        arms: &[(crate::__serde_json::Value, Self::Node)],
        default_node: &Self::Node,
    ) {
        // default: no-op
    }

    /// Optional hook the walker calls after building the row
    /// template of a `Primitive::Virtualizer` on the structured /
    /// generator-backend path. Backends record the count method +
    /// the template node so the remote runtime can clone the
    /// template per row (with id remapping) on every
    /// count change.
    #[allow(unused_variables)]
    fn note_repeat_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        count_method: &'static str,
        row_template: &Self::Node,
        row_index_signal_id: Option<u64>,
    ) {
        // default: no-op
    }

    /// Hook for `Primitive::Virtualizer` on the structured /
    /// generator-backend path. Backends opt in to native windowed
    /// list rendering here (Roku → MarkupList). Default delegates
    /// to `note_repeat_binding` so backends that don't yet
    /// implement native virtualization still get correct (if
    /// unwindowed) row rendering.
    #[allow(unused_variables)]
    fn note_virtualizer_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        count_method: &'static str,
        row_template: &Self::Node,
        row_index_signal_id: Option<u64>,
        horizontal: bool,
    ) {
        self.note_repeat_binding(
            anchor,
            signal_ids,
            count_method,
            row_template,
            row_index_signal_id,
        );
    }

    /// Backend capability flag for lazy slot materialization. When
    /// `true`, the walker wraps each `bind_when!`/`bind_switch!`/
    /// `bind_repeat!` slot's subtree build in `begin_slot_capture`/
    /// `end_slot_capture` calls and skips attaching the slot's root
    /// to the anchor at build time — the backend captures the slot's
    /// commands so the remote runtime can play / tear them down on
    /// demand. Default `false` (eager mode): every slot is built
    /// and attached up-front, the way the framework has always
    /// worked. Backends like Roku with no host-side runtime opt in
    /// so inactive subtrees never materialize on the device.
    fn supports_lazy_slot_capture(&self) -> bool {
        false
    }

    /// Begin a slot-capture region. Subsequent backend mutations
    /// (create_*, insert, apply_style, etc.) should be redirected
    /// from the main command stream into a capture buffer kept by
    /// the backend. Called only when `supports_lazy_slot_capture()`
    /// is true.
    fn begin_slot_capture(&mut self) {
        // default: no-op
    }

    /// End the most-recent slot-capture region. The backend should
    /// associate the captured commands with `slot_root` so a later
    /// `note_when_binding` / `note_switch_binding` /
    /// `note_repeat_binding` call can package them into the
    /// appropriate binding's wire form.
    #[allow(unused_variables)]
    fn end_slot_capture(&mut self, slot_root: &Self::Node) {
        // default: no-op
    }

    /// Create an image node with the initial URL. The framework
    /// wraps the user's `src` source in an effect that calls
    /// `update_image_src` whenever the source changes.
    #[allow(unused_variables)]
    fn create_image(&mut self, src: &str, alt: Option<&str>) -> Self::Node {
        unimplemented!("create_image not implemented for this backend")
    }
    #[allow(unused_variables)]
    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        // default: no-op; backends that don't implement images just
        // leave the URL static.
    }

    /// Create an icon node from static vector path data. The initial
    /// color (if any) is provided; reactive color updates flow through
    /// `update_icon_color`.
    ///
    /// Backends render the paths natively:
    /// - **Web**: inline `<svg>` with `<path>` children.
    /// - **iOS**: `CAShapeLayer` with `UIBezierPath`.
    /// - **Android**: `VectorDrawable` or `Canvas.drawPath`.
    #[allow(unused_variables)]
    fn create_icon(
        &mut self,
        data: &primitives::icon::IconData,
        color: Option<&Color>,
    ) -> Self::Node {
        unimplemented!("create_icon not implemented for this backend")
    }

    /// Update an icon's fill color reactively. Called by the walker's
    /// Effect when the color closure re-fires.
    #[allow(unused_variables)]
    fn update_icon_color(&mut self, node: &Self::Node, color: &Color) {
        // default: no-op
    }

    /// Set the icon's stroke progress immediately (no animation).
    /// `progress` is 0.0 (nothing drawn) to 1.0 (fully drawn).
    /// Called by the walker's reactive Effect when the `stroke`
    /// closure re-fires.
    #[allow(unused_variables)]
    fn update_icon_stroke(&mut self, node: &Self::Node, progress: f32) {
        // default: no-op — icon stays fully drawn
    }

    /// Animate the icon's stroke from `from` to `to` over `duration_ms`
    /// with the given easing. Called once on mount for `draw_in`, or
    /// imperatively via `IconHandle::animate_stroke`.
    ///
    /// When `infinite` is true, the animation loops (from→to→from→…).
    ///
    /// Platforms implement this with their native animation system:
    /// - Web: CSS `@keyframes` animation on `stroke-dashoffset`
    /// - iOS: `CABasicAnimation` on `strokeEnd` with `repeatCount = .infinity`
    /// - Android: `ObjectAnimator` with `setRepeatCount(INFINITE)`
    #[allow(unused_variables)]
    fn animate_icon_stroke(
        &mut self,
        node: &Self::Node,
        from: f32,
        to: f32,
        duration_ms: u32,
        easing: crate::style::Easing,
        infinite: bool,
        autoreverses: bool,
    ) {
        // default: no-op — icon renders fully drawn
    }

    /// Update a button's visible label. Called by the walker's
    /// reactive-label Effect when the user passed a closure (or any
    /// expression containing `.get()`) for the `label` prop. Default
    /// impl falls back to `update_text` — most backends use the same
    /// underlying widget API for both ("setText" on Android,
    /// `textContent` on the web button element). Backends with a
    /// distinct button-text API can override.
    #[allow(unused_variables)]
    fn update_button_label(&mut self, node: &Self::Node, label: &str) {
        self.update_text(node, label);
    }

    /// Create a text input with the initial value, placeholder, and
    /// an `on_change` callback fired on every native input event.
    /// The framework wraps the controlled `value` signal in an
    /// effect that calls `update_text_input_value` on signal change.
    #[allow(unused_variables)]
    fn create_text_input(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
    ) -> Self::Node {
        unimplemented!("create_text_input not implemented for this backend")
    }
    #[allow(unused_variables)]
    fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {}

    /// Create a toggle (switch / checkbox) with the initial value and
    /// an `on_change` callback. Same controlled-update pattern as
    /// text input.
    #[allow(unused_variables)]
    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
    ) -> Self::Node {
        unimplemented!("create_toggle not implemented for this backend")
    }
    #[allow(unused_variables)]
    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {}

    /// Create a scrolling container. `horizontal` selects the
    /// scrolling axis (false = vertical, the default; true = horizontal).
    #[allow(unused_variables)]
    fn create_scroll_view(&mut self, horizontal: bool) -> Self::Node {
        unimplemented!("create_scroll_view not implemented for this backend")
    }

    /// Create a slider widget. `min`/`max`/`step` are static after
    /// creation; controlled value updates flow through
    /// `update_slider_value`. `on_change` fires on every drag tick.
    #[allow(unused_variables)]
    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
    ) -> Self::Node {
        unimplemented!("create_slider not implemented for this backend")
    }
    #[allow(unused_variables)]
    fn update_slider_value(&mut self, node: &Self::Node, value: f32) {}

    /// Create a WebView with the initial URL. `update_web_view_url`
    /// drives subsequent navigations from the reactive source.
    #[allow(unused_variables)]
    fn create_web_view(&mut self, url: &str) -> Self::Node {
        unimplemented!("create_web_view not implemented for this backend")
    }
    #[allow(unused_variables)]
    fn update_web_view_url(&mut self, node: &Self::Node, url: &str) {}

    /// Register a callback fired for each `postMessage` from the
    /// embedded content. The walker calls this after
    /// `create_web_view` when the primitive carries an
    /// `on_message` slot. Default: drop the callback (backend
    /// doesn't service the message channel).
    #[allow(unused_variables)]
    fn web_view_set_on_message(
        &mut self,
        node: &Self::Node,
        callback: Box<dyn Fn(String)>,
    ) {
    }

    /// Register a callback fired when the embedded content
    /// finishes loading.
    #[allow(unused_variables)]
    fn web_view_set_on_load(
        &mut self,
        node: &Self::Node,
        callback: Box<dyn Fn()>,
    ) {
    }

    /// Register a callback fired when the embedded content fails
    /// to load.
    #[allow(unused_variables)]
    fn web_view_set_on_error(
        &mut self,
        node: &Self::Node,
        callback: Box<dyn Fn()>,
    ) {
    }

    /// Create a Video element. Static autoplay/controls/loop are
    /// passed at construction time; reactive `src` updates flow
    /// through `update_video_src`.
    #[allow(unused_variables)]
    fn create_video(
        &mut self,
        src: &str,
        autoplay: bool,
        controls: bool,
        loop_playback: bool,
    ) -> Self::Node {
        unimplemented!("create_video not implemented for this backend")
    }
    #[allow(unused_variables)]
    fn update_video_src(&mut self, node: &Self::Node, src: &str) {}

    /// Create a loading spinner. Size/color are static at construction.
    #[allow(unused_variables)]
    fn create_activity_indicator(
        &mut self,
        size: primitives::activity_indicator::ActivityIndicatorSize,
        color: Option<&Color>,
    ) -> Self::Node {
        unimplemented!("create_activity_indicator not implemented for this backend")
    }

    /// Create a virtualized list. The backend gets a bundle of
    /// callbacks (via `VirtualizerCallbacks`) it uses to query the
    /// current data set, request mounted subtrees, and release
    /// them when items leave the viewport / get recycled.
    ///
    /// The backend owns the scroll handler and the visible-window
    /// math. It calls `mount_item(idx)` when an index needs to
    /// become visible, getting back `(node, scope_id)`. When the
    /// index leaves the visible window (web: scrolled out; native:
    /// cell recycled), the backend calls `release_item(scope_id)`
    /// to free the framework's per-item Scope — which drops every
    /// signal, effect, and ref nested inside that item.
    #[allow(unused_variables)]
    fn create_virtualizer(
        &mut self,
        callbacks: VirtualizerCallbacks<Self::Node>,
        overscan: f32,
        horizontal: bool,
    ) -> Self::Node {
        unimplemented!("create_virtualizer not implemented for this backend")
    }

    /// Signal that the underlying data set has changed. The backend
    /// re-queries item_count + item_key + item_size to figure out
    /// what changed, runs its diff, and updates the mounted set
    /// accordingly. Called from an Effect that reads the data signal,
    /// so it fires on every data update automatically.
    #[allow(unused_variables)]
    fn virtualizer_data_changed(&mut self, node: &Self::Node) {}

    /// Tear down a Virtualizer's backend-side state. The framework
    /// calls this when the primitive's enclosing scope drops — a
    /// `when` branch flip, a `switch` arm rebuild, list recycling,
    /// `Owner` teardown.
    ///
    /// Backends should: detach DOM/native scroll listeners and
    /// observers, drop the wasm-bindgen (or JNI) closure handles
    /// they handed the JS/JVM side, and remove the per-node
    /// instance entry from any internal map.
    ///
    /// **Why this exists**: the user's data closures (passed into
    /// `VirtualizerCallbacks`) typically capture `Signal<T>`s
    /// scoped to the same teardown event. Without this hook, a
    /// browser-queued scroll/resize event firing after the scope
    /// dropped would invoke a Rust callback against a freed
    /// `Signal` slot, panicking with "signal used after its scope
    /// was dropped". Default impl is a no-op for backends that
    /// don't yet implement Virtualizer.
    #[allow(unused_variables)]
    fn release_virtualizer(&mut self, node: &Self::Node) {
        // default no-op
    }

    /// Create a Graphics surface. The backend stands up its native
    /// drawable widget (`<canvas>` on web, `SurfaceView` on Android,
    /// `UIView`+`CAMetalLayer` on iOS), wires up its surface
    /// lifecycle to fire `on_ready` / `on_resize` / `on_lost`, and
    /// returns the host node for the layout tree. The framework
    /// doesn't know what GPU library the author will use; backends
    /// just need to expose their drawable as a
    /// `raw_window_handle::HasWindowHandle + HasDisplayHandle`.
    #[allow(unused_variables)]
    fn create_graphics(
        &mut self,
        on_ready: primitives::graphics::OnReady,
        on_resize: primitives::graphics::OnResize,
        on_lost: primitives::graphics::OnLost,
    ) -> Self::Node {
        unimplemented!("create_graphics not implemented for this backend")
    }

    /// Tear down a Graphics surface. The framework calls this when
    /// the primitive's enclosing scope drops — typically a `When`
    /// branch flipping or `Owner` teardown. Backends should drop
    /// their wgpu device, queue, surface, the user's render state,
    /// any rAF / ResizeObserver closures, and remove the per-node
    /// instance entry. Default impl is a no-op for backends that
    /// don't implement Graphics.
    #[allow(unused_variables)]
    fn release_graphics(&mut self, node: &Self::Node) {
        // default no-op
    }

    /// Remove every child from `node`. Used by reactive conditionals when
    /// the active branch flips and the old subtree needs to be unmounted.
    fn clear_children(&mut self, node: &Self::Node);

    /// Apply a resolved style to a node. The framework has already run
    /// the stylesheet's closure against the active theme; the backend
    /// receives concrete `StyleRules` with literal values.
    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>);

    /// Mint (or look up) a backend-side class identifier for a
    /// resolved style **without** touching any DOM node. Used by the
    /// batched-Repeat path so the walker can compute class names
    /// pre-batch and feed them into a single
    /// [`execute_batch`](Self::execute_batch) call.
    ///
    /// Returns `None` for backends that don't have a named-class
    /// model (most native backends — they apply styles imperatively
    /// to each node and have nothing to mint up front). In that case
    /// the walker treats the Repeat as non-batchable and falls back
    /// to the per-call path. Web overrides this to either return a
    /// cached pre-generated class name or mint a fresh dynamic class
    /// (inserting the CSS rule into the shared sheet, with no
    /// per-node tracking — the per-node bookkeeping happens later
    /// when the batch's `ApplyStyleStatic` op fires).
    #[allow(unused_variables)]
    fn mint_style_class(&mut self, style: &Rc<StyleRules>) -> Option<String> {
        None
    }

    /// Apply a base style plus per-state overlays. Called when the
    /// stylesheet declares interaction-state blocks (`state hovered`,
    /// `state pressed`, etc.) AND the backend reports native state
    /// handling via [`Backend::handles_states_natively`].
    ///
    /// Web overrides this to emit the overlays as CSS pseudo-class
    /// rules scoped to the base class — the browser then handles
    /// state tracking natively. No Rust↔JS round trip per event.
    ///
    /// Backends that rely on event-driven state activation
    /// (`attach_states` + signal-driven re-resolve) leave both the
    /// default impl AND `handles_states_natively() = false`. State
    /// overlays reach those backends through the regular
    /// `apply_style` path when the state signal flips.
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

    /// Backend capability flag. `true` means the backend wants to
    /// receive state overlays declaratively via `apply_styled_states`
    /// and handle state tracking natively (e.g. CSS pseudo-classes
    /// on web). `false` means the backend uses the event-driven path:
    /// `attach_states` registers native event listeners that flip the
    /// framework's per-node state signal, and each state change
    /// re-fires the style effect with the appropriate overlay merged
    /// into a fresh `StyleApplication`.
    ///
    /// The framework reads this once per `attach_style` to choose
    /// between the two paths. Default is `false` — backends opt in.
    fn handles_states_natively(&self) -> bool {
        false
    }

    /// True if `update_tokens` on this backend propagates new token
    /// values to every node referencing those tokens, WITHOUT
    /// requiring the framework to re-apply each styled node's
    /// resolved rules.
    ///
    /// Web backends emit `var(--token, fallback)` references in
    /// CSS for `Tokenized<T>` values; on `update_tokens` they set
    /// the corresponding `--token` on `:root` and the browser's
    /// cascade does the rest. No per-node `setAttribute` or CSS
    /// rule re-emit is needed for theme value changes.
    ///
    /// Native backends typically resolve tokens to literal values
    /// at apply time, so a value change requires per-node
    /// re-application. They return `false` (the default).
    ///
    /// The framework reads this at the cohort-driver level: when
    /// true, the driver skips iterating the cohort on token-only
    /// updates. When false (or when the stylesheet contains
    /// `Derived<T>` that resolves to a concrete value rather than
    /// a token reference), the driver fans out to all members.
    ///
    /// Caveat: if author code uses `Derived<T>` whose closure
    /// produces a *concrete* value computed from token values
    /// (e.g. a custom `Color::lighten` against `t.primary`), the
    /// resulting CSS rule body contains the literal RGB and won't
    /// re-emit on theme change. Such stylesheets need either
    /// per-node re-apply (set this backend's capability to false)
    /// or to be rewritten using `Tokenized<T>` references that
    /// emit as `var()` in the CSS output.
    fn token_updates_propagate_via_cascade(&self) -> bool {
        false
    }

    /// Pre-generate any backend-side state for a stylesheet against the
    /// current theme. Web backends typically use this to mint CSS
    /// classes for every variant + compound combination up front, so
    /// `apply_style` is a cache hit. Other backends can leave the
    /// default no-op implementation.
    ///
    /// Called by the framework:
    /// - The first time a stylesheet is `resolve`d.
    /// - After every `set_theme(...)`, for every still-live stylesheet,
    ///   so the backend's pre-generated state is refreshed.
    ///
    /// The framework passes pre-resolved `StyleRules` (one per relevant
    /// variant combination) so the backend doesn't have to think about
    /// theme tokens — it gets concrete property bags.
    #[allow(unused_variables)]
    fn register_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        // default: no-op
    }

    /// Release a previously-registered stylesheet's pre-generated state.
    /// Called when the stylesheet is no longer reachable (its last
    /// `Rc<StyleSheet>` has been dropped) and after every theme change
    /// (before re-registering, so old state is cleaned up).
    #[allow(unused_variables)]
    fn unregister_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        // default: no-op
    }

    /// Make a static asset available for use by the renderer.
    ///
    /// Called the first time an `AssetId` is observed for this backend
    /// (the framework dedupes by id). The backend decides what
    /// registration means:
    /// - **Web**: fonts inject a `@font-face` rule into the document
    ///   stylesheet; images stash the URL in a node↔URL map.
    /// - **iOS**: fonts call `CTFontManagerRegisterFontsForURL`;
    ///   images become a `UIImage(named:)` cache entry.
    /// - **Android**: fonts go through `Typeface.createFromAsset`;
    ///   images preload into a `Bitmap` cache.
    /// - **wgpu**: bytes are uploaded into the text engine / texture
    ///   atlas.
    ///
    /// `kind` exists so a single entry point can fan out without each
    /// backend writing a giant `match` on the source's extension. The
    /// type-safe [`Asset<K>`](crate::assets::Asset) handle on the
    /// author side already enforces this at compile time; `kind`
    /// repeats it for runtime dispatch.
    ///
    /// Default no-op so backends without a renderer-side asset
    /// concept (early stubs, or fully wire-driven backends that
    /// forward registration upstream) compile without scaffolding.
    #[allow(unused_variables)]
    fn register_asset(&mut self, id: AssetId, kind: AssetTag, source: &AssetSource) {
        // default: no-op
    }

    /// Release a previously-registered asset. Currently called only
    /// from explicit unload paths (assets are otherwise `'static` and
    /// live for the duration of the program); backends with bounded
    /// caches override to evict, others can leave the default no-op.
    #[allow(unused_variables)]
    fn unregister_asset(&mut self, id: AssetId, kind: AssetTag) {
        // default: no-op
    }

    /// Register a font family (a [`Typeface`](crate::assets::Typeface))
    /// so subsequent style applications can resolve its faces.
    ///
    /// The framework guarantees that every `face.asset` referenced
    /// here has already been registered via [`register_asset`] in the
    /// same render flush. Backends that key fonts by family name + a
    /// weight/style table (web's `font-family` / `font-weight` /
    /// `font-style`, iOS post-registration `UIFont(name:)`) use this
    /// call to record the mapping; backends that just take raw bytes
    /// per face (wgpu / cosmic-text) can leave the default no-op and
    /// drive registration entirely off `register_asset`.
    ///
    /// [`register_asset`]: Self::register_asset
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

    /// Release a previously-registered typeface. Mirrors
    /// [`unregister_asset`](Self::unregister_asset).
    #[allow(unused_variables)]
    fn unregister_typeface(&mut self, id: TypefaceId) {
        // default: no-op
    }

    /// Install the initial token set as runtime variables. Called by
    /// the framework once at app boot, before any stylesheet is
    /// registered.
    ///
    /// Backends with a runtime variable layer (web's CSS custom
    /// properties) implement this to write `--{name}: {value}` on the
    /// document root. Backends without a variable system (iOS,
    /// Android) leave the default no-op; they read
    /// `Tokenized::value()` at apply time and behave as if the literal
    /// were set.
    #[allow(unused_variables)]
    fn install_tokens(&mut self, tokens: &[crate::TokenEntry]) {
        // default: no-op
    }

    /// Push updated token values. Called by the framework on every
    /// `update_tokens(...)`. Backends with a runtime variable layer
    /// update the existing declarations in place — one DOM op per
    /// changed token, no rule churn. Backends without a variable
    /// system leave the default no-op; the framework re-fires every
    /// styled effect via the tokens-version signal so the new
    /// fallback values flow through `apply_style`.
    #[allow(unused_variables)]
    fn update_tokens(&mut self, tokens: &[crate::TokenEntry]) {
        // default: no-op
    }

    /// Called when a styled node is being torn down (its surrounding
    /// `Effect` scope is dropping). Lets backends free per-node state —
    /// e.g. the web backend drops the node's dynamic CSS class slot
    /// and its node-id entry. Other backends typically don't need this.
    #[allow(unused_variables)]
    fn on_node_unstyled(&mut self, node: &Self::Node) {
        // default: no-op
    }

    /// Node's rect in its **parent's** coordinate system.
    /// Returns `None` if the node isn't mounted in a layout yet (e.g.
    /// queried before the first frame) or if the backend can't report
    /// positions. Default returns `None`.
    ///
    /// Use this for "where is X relative to its parent" — e.g. measuring
    /// a sidebar item's offset within its container. For viewport
    /// positions, use [`absolute_frame`](Backend::absolute_frame).
    #[allow(unused_variables)]
    fn frame(&self, node: &Self::Node) -> Option<primitives::portal::ViewportRect> {
        None
    }

    /// Node's rect in the **window/viewport's** coordinate system.
    /// Returns `None` if the node isn't mounted in a window yet.
    /// Default returns `None`.
    ///
    /// Backends that already implement `*Ops::rect` for overlay
    /// anchoring should forward to the same conversion path here
    /// (e.g. UIKit `convertRect:toView:window`, DOM
    /// `getBoundingClientRect()`).
    #[allow(unused_variables)]
    fn absolute_frame(&self, node: &Self::Node) -> Option<primitives::portal::ViewportRect> {
        None
    }

    /// Wires the backend's native interaction events (hover, press,
    /// focus) to the framework's per-node state machinery. The
    /// framework allocates a `Signal<StateBits>` per styled node and
    /// passes a setter closure here; backends call the setter when
    /// the corresponding native event fires.
    ///
    /// The setter takes `(state, on)` where `state` is a
    /// `StateBits` flag (`StateBits::HOVERED`, etc.) and `on` is
    /// true for entering / false for leaving the state. The framework
    /// re-resolves and re-applies the node's style when state bits
    /// change — backends don't need to do any style work themselves.
    ///
    /// Default impl is a no-op for backends that don't yet support
    /// interaction states (states declared in the stylesheet simply
    /// never activate on those platforms — a documented no-op).
    #[allow(unused_variables)]
    fn attach_states(&mut self, node: &Self::Node, setter: Rc<dyn Fn(StateBits, bool)>) {
        // default: no-op
    }

    /// Mark the native widget as disabled or enabled. Distinct from
    /// the `DISABLED` style-state bit (which controls overlay
    /// styling) — this one is about the widget being inert: web's
    /// `disabled` attribute, `setEnabled(false)` on native. Backends
    /// that don't distinguish leave the default no-op.
    #[allow(unused_variables)]
    fn set_disabled(&mut self, node: &Self::Node, disabled: bool) {
        // default: no-op
    }

    // ---- handle builders ------------------------------------------------
    //
    // Each one defaults to "no-op handle backed by `Rc::new(())`" —
    // backends that don't yet support `.bind()` refs for a given
    // primitive get something type-correct without having to think
    // about ops downcasting.

    #[allow(unused_variables)]
    fn make_button_handle(&self, node: &Self::Node) -> ButtonHandle {
        ButtonHandle::new(Rc::new(()), &NoopButtonOps)
    }

    #[allow(unused_variables)]
    fn make_pressable_handle(&self, node: &Self::Node) -> PressableHandle {
        PressableHandle::new(Rc::new(()), &NoopPressableOps)
    }

    #[allow(unused_variables)]
    fn make_view_handle(&self, node: &Self::Node) -> ViewHandle {
        ViewHandle::new(Rc::new(()), &NoopViewOps)
    }

    #[allow(unused_variables)]
    fn make_text_handle(&self, node: &Self::Node) -> TextHandle {
        TextHandle::new(Rc::new(()), &NoopTextOps)
    }

    #[allow(unused_variables)]
    fn make_image_handle(&self, node: &Self::Node) -> primitives::image::ImageHandle {
        primitives::image::ImageHandle::new(Rc::new(()), &NoopImageOps)
    }

    #[allow(unused_variables)]
    fn make_icon_handle(&self, node: &Self::Node) -> primitives::icon::IconHandle {
        primitives::icon::IconHandle::new(Rc::new(()), &NoopIconOps)
    }

    #[allow(unused_variables)]
    fn make_text_input_handle(
        &self,
        node: &Self::Node,
    ) -> primitives::text_input::TextInputHandle {
        primitives::text_input::TextInputHandle::new(Rc::new(()), &NoopTextInputOps)
    }

    #[allow(unused_variables)]
    fn make_toggle_handle(&self, node: &Self::Node) -> primitives::toggle::ToggleHandle {
        primitives::toggle::ToggleHandle::new(Rc::new(()), &NoopToggleOps)
    }

    #[allow(unused_variables)]
    fn make_scroll_view_handle(
        &self,
        node: &Self::Node,
    ) -> primitives::scroll_view::ScrollViewHandle {
        primitives::scroll_view::ScrollViewHandle::new(Rc::new(()), &NoopScrollViewOps)
    }

    #[allow(unused_variables)]
    fn make_slider_handle(&self, node: &Self::Node) -> primitives::slider::SliderHandle {
        primitives::slider::SliderHandle::new(Rc::new(()), &NoopSliderOps)
    }

    #[allow(unused_variables)]
    fn make_web_view_handle(
        &self,
        node: &Self::Node,
    ) -> primitives::web_view::WebViewHandle {
        primitives::web_view::WebViewHandle::new(Rc::new(()), &NoopWebViewOps)
    }

    #[allow(unused_variables)]
    fn make_video_handle(&self, node: &Self::Node) -> primitives::video::VideoHandle {
        primitives::video::VideoHandle::new(Rc::new(()), &NoopVideoOps)
    }

    #[allow(unused_variables)]
    fn make_activity_indicator_handle(
        &self,
        node: &Self::Node,
    ) -> primitives::activity_indicator::ActivityIndicatorHandle {
        primitives::activity_indicator::ActivityIndicatorHandle::new(
            Rc::new(()),
            &NoopActivityIndicatorOps,
        )
    }

    #[allow(unused_variables)]
    fn make_virtualizer_handle(
        &self,
        node: &Self::Node,
    ) -> primitives::virtualizer::VirtualizerHandle {
        primitives::virtualizer::VirtualizerHandle::new(
            Rc::new(()),
            &NoopVirtualizerOps,
        )
    }

    #[allow(unused_variables)]
    fn make_graphics_handle(
        &self,
        node: &Self::Node,
    ) -> primitives::graphics::GraphicsHandle {
        primitives::graphics::GraphicsHandle::new(Rc::new(()), &NoopGraphicsOps)
    }

    /// Stand up a navigator. The backend builds its native container
    /// (UINavigationController / FragmentManager root / a `<div>` on
    /// web) and installs the dispatcher closure on the supplied
    /// `NavigatorControl` so handle calls reach the backend.
    ///
    /// The backend is responsible for:
    /// 1. Returning the navigator's container node.
    /// 2. Calling `control.install(Box::new(...))` with its dispatcher.
    /// 3. Calling `callbacks.depth_changed(new_depth)` after every
    ///    push/pop/replace/reset commits.
    /// 4. Calling `callbacks.release_screen(scope_id)` for every
    ///    screen it removes (popped or replaced), so its `Scope`
    ///    drops and the screen's signals/effects/refs are freed.
    ///
    /// **The backend MUST NOT call `callbacks.mount_screen` synchronously
    /// inside this method.** `create_navigator` is invoked while the
    /// framework holds a `borrow_mut` on the backend `RefCell`;
    /// `mount_screen` re-enters the build walker which would attempt
    /// another `borrow_mut` — double-borrow panic. The framework
    /// mounts the initial screen itself *after* this method returns
    /// and hands the result to [`Backend::navigator_attach_initial`].
    /// Dispatcher closures saved on `control` run later (outside the
    /// borrow window), so they're free to call `mount_screen`.
    ///
    /// Default impl is `unimplemented!()` — most backends will want a
    /// real implementation.
    #[allow(unused_variables)]
    fn create_navigator(
        &mut self,
        callbacks: primitives::navigator::NavigatorCallbacks<Self::Node>,
        control: Rc<primitives::navigator::NavigatorControl>,
    ) -> Self::Node {
        unimplemented!("create_navigator not implemented for this backend")
    }

    /// Mount the initial screen into a freshly-created navigator.
    /// Called by the framework immediately after `create_navigator`
    /// returns, with the result of mounting the initial route via
    /// the framework's per-screen scope machinery.
    ///
    /// Splitting this from `create_navigator` avoids a re-entrant
    /// `borrow_mut` — see [`Backend::create_navigator`] for the full
    /// explanation. Backends that don't implement navigators can
    /// leave the default no-op.
    #[allow(unused_variables)]
    fn navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: primitives::navigator::ScreenOptions,
    ) {
        // default: no-op; backends that don't implement Navigator
        // never get called here (the framework only invokes this
        // alongside `create_navigator`).
    }

    /// Apply style to the navigator's header bar (background, shadow).
    #[allow(unused_variables)]
    fn apply_navigator_header_style(&mut self, navigator: &Self::Node, style: &Rc<StyleRules>) {}

    /// Apply style to the navigator's title text (color, font).
    #[allow(unused_variables)]
    fn apply_navigator_title_style(&mut self, navigator: &Self::Node, style: &Rc<StyleRules>) {}

    /// Apply style to the navigator's bar button items (tint color).
    #[allow(unused_variables)]
    fn apply_navigator_button_style(&mut self, navigator: &Self::Node, style: &Rc<StyleRules>) {}

    /// Apply style to the navigator's body container — the view that
    /// hosts the active screen. Used by `.background_color(...)` on
    /// the navigator (and by `HeaderStyle.body_background` via the
    /// `.header(...)` helper) to set a fill that shows through any
    /// transparent regions of the mounted screen.
    #[allow(unused_variables)]
    fn apply_navigator_body_style(&mut self, navigator: &Self::Node, style: &Rc<StyleRules>) {}

    /// Apply style to a drawer navigator's sidebar panel.
    #[allow(unused_variables)]
    fn apply_drawer_sidebar_style(&mut self, navigator: &Self::Node, style: &Rc<StyleRules>) {}

    /// Apply style to a drawer navigator's scrim overlay.
    #[allow(unused_variables)]
    fn apply_drawer_scrim_style(&mut self, navigator: &Self::Node, style: &Rc<StyleRules>) {}

    /// Apply style to a tab navigator's tab bar.
    #[allow(unused_variables)]
    fn apply_tab_bar_style(&mut self, navigator: &Self::Node, style: &Rc<StyleRules>) {}

    /// Apply style to a tab navigator's tab icons.
    #[allow(unused_variables)]
    fn apply_tab_icon_style(&mut self, navigator: &Self::Node, style: &Rc<StyleRules>) {}

    /// Apply style to a tab navigator's tab labels.
    #[allow(unused_variables)]
    fn apply_tab_label_style(&mut self, navigator: &Self::Node, style: &Rc<StyleRules>) {}

    /// Tear down a navigator. The framework calls this when the
    /// navigator's enclosing scope drops — owner teardown, a `when`
    /// flipping past the navigator, etc. Backends should drop their
    /// native stack, release every still-mounted screen scope, and
    /// drop any closures they handed the JS/JVM side. Default is a
    /// no-op for backends that don't implement Navigator.
    #[allow(unused_variables)]
    fn release_navigator(&mut self, node: &Self::Node) {}

    /// Default no-op handle. Backends that actually implement
    /// navigators override this to return a real handle wired to the
    /// control plane (see `NavigatorHandle::with_control`).
    #[allow(unused_variables)]
    fn make_navigator_handle(
        &self,
        node: &Self::Node,
    ) -> primitives::navigator::NavigatorHandle {
        primitives::navigator::NavigatorHandle::new(Rc::new(()), &NoopNavigatorOps)
    }

    /// Create a tab navigator. Same shape contract as
    /// [`Backend::create_navigator`]: backend stores the callbacks,
    /// installs a dispatcher on `control`, but does NOT call
    /// `mount_screen` synchronously (re-entrant borrow). Per-mount
    /// timing depends on `callbacks.mount_policy`:
    ///
    /// - `EagerPersistent`: mount every tab on creation via
    ///   microtask (web) / main-queue dispatch (iOS, Android).
    /// - `LazyPersistent`: mount on first activation; keep mounted.
    /// - `LazyDisposing`: mount on activation; release the previous
    ///   tab's scope on switch.
    ///
    /// Default: panic. Phase-3 lands the framework-side plumbing;
    /// each backend implements it in a follow-up.
    #[allow(unused_variables)]
    fn create_tab_navigator(
        &mut self,
        callbacks: primitives::navigator::TabNavigatorCallbacks<Self::Node>,
        control: Rc<primitives::navigator::NavigatorControl>,
    ) -> Self::Node {
        unimplemented!("create_tab_navigator not implemented for this backend")
    }

    /// Mount the initial screen into a freshly-created tab
    /// navigator. Same shape as [`Backend::navigator_attach_initial`]
    /// — splitting from `create_tab_navigator` avoids the
    /// re-entrant borrow_mut. Backends that mount the initial
    /// screen via a microtask (web) can leave this as the default
    /// no-op; backends that mount synchronously (Android) implement
    /// it.
    #[allow(unused_variables)]
    fn tab_navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: primitives::navigator::ScreenOptions,
    ) {
    }

    /// Tear down a tab navigator. Same contract as
    /// [`Backend::release_navigator`]. Default no-op so backends
    /// that don't implement tabs aren't required to define this.
    #[allow(unused_variables)]
    fn release_tab_navigator(&mut self, node: &Self::Node) {}

    /// Default no-op handle for tab navigators. Backends override to
    /// return a real handle wired to the control plane.
    #[allow(unused_variables)]
    fn make_tab_navigator_handle(
        &self,
        node: &Self::Node,
    ) -> primitives::navigator::TabsHandle {
        primitives::navigator::TabsHandle::from_inner(
            primitives::navigator::NavigatorHandle::new(Rc::new(()), &NoopNavigatorOps),
        )
    }

    /// Create a drawer navigator. Same shape contract as
    /// [`Backend::create_navigator`] and
    /// [`Backend::create_tab_navigator`]: backend stores the
    /// callbacks, installs a dispatcher on `control`, does NOT call
    /// `mount_screen` synchronously.
    ///
    /// In addition to the screen-mounting machinery shared with
    /// other navigator kinds, the backend's dispatcher handles
    /// `OpenDrawer` / `CloseDrawer` / `ToggleDrawer` commands and
    /// drives the platform's drawer widget (DrawerLayout on
    /// Android, hand-rolled overlay on iOS, off-canvas aside on
    /// web). The `callbacks.is_open` signal mirrors the open state
    /// for reactive layouts.
    ///
    /// Default: panic. Phase-4 lands the framework-side plumbing;
    /// each backend implements it in a follow-up.
    #[allow(unused_variables)]
    fn create_drawer_navigator(
        &mut self,
        callbacks: primitives::navigator::DrawerNavigatorCallbacks<Self::Node>,
        control: Rc<primitives::navigator::NavigatorControl>,
    ) -> Self::Node {
        unimplemented!("create_drawer_navigator not implemented for this backend")
    }

    /// Mount the initial screen into a freshly-created drawer
    /// navigator. Same contract as
    /// [`Backend::tab_navigator_attach_initial`].
    #[allow(unused_variables)]
    fn drawer_navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: primitives::navigator::ScreenOptions,
    ) {
    }

    /// Attach a pre-built sidebar Node to a drawer navigator.
    /// Called by the walker after `create_drawer_navigator` returns
    /// and the framework has built the user's `.sidebar(...)`
    /// closure output into a backend Node.
    ///
    /// Native backends (Android) override to position the sidebar
    /// inside their native drawer-shell. Web ignores this — its
    /// `.layout(...)` handles sidebar placement via
    /// `LayoutProps::sidebar`, so a separate attach call isn't
    /// needed there. Default no-op.
    #[allow(unused_variables)]
    fn drawer_navigator_attach_sidebar(
        &mut self,
        navigator: &Self::Node,
        sidebar: Self::Node,
    ) {
    }

    /// Attach a pre-built layout subtree to a navigator.
    ///
    /// In the local-render path, `create_*_navigator` invokes the
    /// author's `.layout(...)` closure itself (via the framework
    /// callbacks). For the AAS / wire-replay path, the recording
    /// backend invokes it on the dev-side and ships the layout as
    /// a normal subtree of `CreateView`/`Insert`/`ApplyStyle`
    /// commands; this call is what tells the receiving backend
    /// "here is the layout's root node (insert it into the
    /// navigator's container) and here is the outlet (mount
    /// subsequent screens into it instead of the bare container)."
    ///
    /// Default no-op — only web cares (the iOS/Android backends
    /// render navigator chrome natively and don't use the
    /// `.layout()` slot).
    #[allow(unused_variables)]
    fn attach_navigator_layout(
        &mut self,
        navigator: &Self::Node,
        root: Self::Node,
        outlet: Self::Node,
    ) {
    }

    /// Tear down a drawer navigator. Same contract as
    /// [`Backend::release_navigator`]. Default no-op so backends
    /// that don't implement drawers aren't required to define this.
    #[allow(unused_variables)]
    fn release_drawer_navigator(&mut self, node: &Self::Node) {}

    /// Default no-op handle for drawer navigators. Backends override
    /// to return a real handle wired to the control plane.
    #[allow(unused_variables)]
    fn make_drawer_navigator_handle(
        &self,
        node: &Self::Node,
    ) -> primitives::navigator::DrawerHandle {
        primitives::navigator::DrawerHandle::from_inner(
            primitives::navigator::NavigatorHandle::new(Rc::new(()), &NoopNavigatorOps),
            Rc::new(std::cell::Cell::new(false)),
        )
    }

    /// Create a third-party `Primitive::External` node. Backends that
    /// expose an [`ExternalRegistry`](crate::external::ExternalRegistry)
    /// consult it for a registered handler; on miss they should fall
    /// through to a platform-native "not supported" placeholder.
    /// Backends with no external support leave the default panic.
    ///
    /// `type_id` drives dispatch; `type_name` is for debug/error
    /// messages only.
    #[allow(unused_variables)]
    fn create_external(
        &mut self,
        type_id: std::any::TypeId,
        type_name: &'static str,
        payload: &Rc<dyn Any>,
    ) -> Self::Node {
        unimplemented!(
            "create_external not implemented for this backend (external primitive: {})",
            type_name
        )
    }

    /// Tear down an external primitive's backend-side state. Default
    /// no-op; backends that hold per-node listeners / observers /
    /// closure handles override.
    #[allow(unused_variables)]
    fn release_external(&mut self, node: &Self::Node) {
        // default no-op
    }

    /// Create a portal — render `children` (mounted via subsequent
    /// `insert(node, child)` calls on the returned node) at `target`,
    /// escaping the parent's layout and clipping context.
    ///
    /// Backends stand up their platform-native render-elsewhere
    /// mechanism:
    /// - **Web**: a `<div>` appended to `document.body` (escapes
    ///   `overflow:hidden` and stacking contexts).
    /// - **iOS**: a `UIView` added to the key window
    ///   (`UIWindow.addSubview:`).
    /// - **Android**: a `WindowManager.addView` window-level view, or
    ///   a `Dialog`-hosted container.
    /// - **Roku**: a `Group` parented to the root scene.
    ///
    /// For [`PortalTarget::Anchor`], backends should subscribe to
    /// scroll / layout / orientation events from the anchor's host
    /// hierarchy and re-query `target.rect()` to reposition the
    /// portal as the anchor moves.
    ///
    /// `on_dismiss` fires when the platform requests dismissal
    /// (Escape on web, back gesture on Android, swipe-down on iOS).
    /// The framework doesn't auto-tear-down — the host's open-state
    /// signal is the source of truth; flipping it drops the
    /// surrounding scope and triggers [`Backend::release_portal`].
    ///
    /// Default: panic. Backends that don't yet implement portals
    /// shouldn't have authors mounting them.
    #[allow(unused_variables)]
    fn create_portal(
        &mut self,
        target: primitives::portal::PortalTarget,
        on_dismiss: Option<Rc<dyn Fn()>>,
        trap_focus: bool,
    ) -> Self::Node {
        unimplemented!("create_portal not implemented for this backend")
    }

    /// Tear down a portal's backend-side state. Same contract as
    /// [`Backend::release_overlay`] — detach the platform mount,
    /// drop event-listener handles, free observer subscriptions.
    #[allow(unused_variables)]
    fn release_portal(&mut self, node: &Self::Node) {
        // default no-op
    }

    /// Default no-op handle for portals. Backends with imperative
    /// portal APIs (future: reposition, update target, …) override.
    #[allow(unused_variables)]
    fn make_portal_handle(
        &self,
        node: &Self::Node,
    ) -> primitives::portal::PortalHandle {
        primitives::portal::PortalHandle::new(Rc::new(()), &NoopPortalOps)
    }

    /// Apply a presence-style transform (opacity + 2D translate +
    /// uniform scale) to a node. Called by the walker's presence
    /// arm at three points:
    ///
    /// - **Pre-mount enter** — `state = enter.from`, `transition =
    ///   None`. The node is snapped to the entering state before
    ///   its first paint.
    /// - **Animate to resting** — `state = PresenceState::rest()`,
    ///   `transition = Some((duration, easing))`. The next animation
    ///   frame after mount; the backend interpolates from the
    ///   pre-mount state to identity.
    /// - **Exit** — `state = exit.to`, `transition = Some((duration,
    ///   easing))`. The walker schedules a scope-drop after the
    ///   transition completes.
    /// - **Reversal** — same as "animate to resting" when an exit
    ///   is interrupted by `present()` flipping back true.
    ///
    /// `PresenceState::rest()` means "no presence override is
    /// active." Backends that don't implement presence leave the
    /// default no-op; presence-controlled subtrees still mount and
    /// unmount, just without animation.
    #[allow(unused_variables)]
    fn apply_presence(
        &mut self,
        node: &Self::Node,
        state: primitives::presence::PresenceState,
        transition: Option<(u32, crate::style::Easing)>,
    ) {
        // default: no-op
    }

    /// Default no-op handle for presence. Backends with an imperative
    /// presence API can override.
    #[allow(unused_variables)]
    fn make_presence_handle(
        &self,
        node: &Self::Node,
    ) -> primitives::presence::PresenceHandle {
        primitives::presence::PresenceHandle::new(Rc::new(()), &NoopPresenceOps)
    }

    /// Create a navigable container — the `Link` primitive.
    ///
    /// Backends are responsible for:
    /// - Producing the platform-native interactive widget that
    ///   wraps the eventual children. On web this should be a
    ///   real `<a href=config.url>` so the browser's native link
    ///   contract works (right-click "copy link," middle-click
    ///   "open in new tab," screen-reader "link" role, etc.).
    ///   On native platforms, an accessibility-Link-roled tappable
    ///   container is the right shape.
    /// - Wiring activation: when the user taps / clicks / activates
    ///   the widget, call `config.on_activate()`. The framework
    ///   has already baked the push/replace/reset dispatch into
    ///   that closure — the backend just fires it.
    /// - For web specifically: intercept the click and
    ///   `preventDefault` to keep the SPA single-page, but only
    ///   for plain clicks. Modified clicks (cmd/ctrl/middle,
    ///   shift) should fall through to the browser's default
    ///   handler so "open in new tab/window" still works.
    ///
    /// Default falls through to `create_view`, dropping
    /// `on_activate`. Backends that don't implement Link still mount
    /// the children correctly — the link just isn't tappable. This
    /// keeps a Link in a primitive tree from panicking the screen
    /// build on an unimplemented backend, which matches the posture
    /// of every other optional handle method (return a no-op rather
    /// than refuse). Backends that want real activation override.
    #[allow(unused_variables)]
    fn create_link(&mut self, config: primitives::link::LinkConfig) -> Self::Node {
        self.create_view()
    }

    /// Apply safe-area-aware padding to `node`. Called by the walker
    /// for every container that opted in via `.safe_area(...)`, and
    /// again reactively whenever
    /// [`crate::safe_area_insets()`] fires (orientation flip,
    /// dynamic-island change, sheet adaptation).
    ///
    /// Backends should:
    /// 1. Read the platform's current safe-area insets (from
    ///    `UIView.safeAreaInsets`, `WindowInsets.systemBars()`,
    ///    `env(safe-area-inset-*)`, etc.).
    /// 2. For each side flag in `sides`, add the corresponding inset
    ///    to that side's *padding* on the node — combining with any
    ///    author-set padding (don't clobber it).
    /// 3. Schedule a layout pass if the padding changed.
    ///
    /// The default impl is a no-op so backends without safe-area
    /// awareness (or that don't yet implement it) silently ignore
    /// the opt-in instead of panicking.
    #[allow(unused_variables)]
    fn apply_safe_area_padding(
        &mut self,
        node: &Self::Node,
        sides: crate::SafeAreaSides,
    ) {
        // default: no-op
    }

    /// Apply safe-area treatment to a *ScrollView*. Same shape as
    /// `apply_safe_area_padding` but with native-correct semantics
    /// for a scroll container: the scroll surface bleeds edge-to-edge
    /// while the *content origin* is inset by the safe-area amount.
    /// On iOS this is `UIScrollView.contentInset` (with
    /// `contentInsetAdjustmentBehavior = .never`). On Android,
    /// `setPadding(...)` + `setClipToPadding(false)`. On web, padding
    /// on the inner content wrapper while the scroll view itself
    /// keeps `padding: 0`.
    ///
    /// Distinguished from `apply_safe_area_padding` by the dispatch
    /// site in the walker: `Primitive::View` with `.safe_area(...)`
    /// uses padding; `Primitive::ScrollView` with `.safe_area(...)`
    /// uses content insets. The user-facing builder is the same
    /// (`.safe_area(...)`); the framework picks the right path
    /// based on which primitive it's on.
    ///
    /// Default impl falls back to `apply_safe_area_padding` so
    /// backends without a separate inset path keep working (just
    /// with the old "padding on the scroll view itself" visual).
    #[allow(unused_variables)]
    fn apply_scroll_view_safe_area_inset(
        &mut self,
        node: &Self::Node,
        sides: crate::SafeAreaSides,
    ) {
        self.apply_safe_area_padding(node, sides);
    }

    /// Default no-op handle for `Ref<LinkHandle>`. Backends that
    /// can synthesize activation events override this.
    #[allow(unused_variables)]
    fn make_link_handle(&self, node: &Self::Node) -> primitives::link::LinkHandle {
        primitives::link::LinkHandle::new(Rc::new(()), &NoopLinkOps)
    }

    fn finish(&mut self, root: Self::Node);
}

// ---------------------------------------------------------------------------
// Noop ops — default ZST impls used by the trait's `make_*_handle`
// defaults. Backends that don't support a particular primitive's refs
// can leave the defaults in place and authors get a type-correct
// no-op handle.
// ---------------------------------------------------------------------------

struct NoopIconOps;
impl primitives::icon::IconOps for NoopIconOps {
    // Default impls in the trait handle no-op behavior.
}

struct NoopImageOps;
impl primitives::image::ImageOps for NoopImageOps {}

struct NoopTextInputOps;
impl primitives::text_input::TextInputOps for NoopTextInputOps {
    fn focus(&self, _: &dyn Any) {}
    fn blur(&self, _: &dyn Any) {}
    fn select_all(&self, _: &dyn Any) {}
}

struct NoopToggleOps;
impl primitives::toggle::ToggleOps for NoopToggleOps {}

struct NoopScrollViewOps;
impl primitives::scroll_view::ScrollViewOps for NoopScrollViewOps {
    fn scroll_to(&self, _: &dyn Any, _: f32, _: f32) {}
}

struct NoopSliderOps;
impl primitives::slider::SliderOps for NoopSliderOps {}

struct NoopWebViewOps;
impl primitives::web_view::WebViewOps for NoopWebViewOps {}

struct NoopVideoOps;
impl primitives::video::VideoOps for NoopVideoOps {
    fn play(&self, _: &dyn Any) {}
    fn pause(&self, _: &dyn Any) {}
    fn seek(&self, _: &dyn Any, _: f32) {}
}

struct NoopActivityIndicatorOps;
impl primitives::activity_indicator::ActivityIndicatorOps for NoopActivityIndicatorOps {}

struct NoopVirtualizerOps;
impl primitives::virtualizer::VirtualizerOps for NoopVirtualizerOps {
    fn scroll_to_index(&self, _: &dyn Any, _: usize) {}
}

struct NoopGraphicsOps;
impl primitives::graphics::GraphicsOps for NoopGraphicsOps {}

struct NoopNavigatorOps;
impl primitives::navigator::NavigatorOps for NoopNavigatorOps {}

struct NoopLinkOps;
impl primitives::link::LinkOps for NoopLinkOps {
    fn activate(&self, _node: &dyn Any) {}
}

struct NoopPresenceOps;
impl primitives::presence::PresenceOps for NoopPresenceOps {}

struct NoopPortalOps;
impl primitives::portal::PortalOps for NoopPortalOps {}

struct NoopButtonOps;
impl ButtonOps for NoopButtonOps {
    fn click(&self, _node: &dyn Any) {}
}

struct NoopPressableOps;
impl PressableOps for NoopPressableOps {
    fn click(&self, _node: &dyn Any) {}
}

struct NoopViewOps;
impl ViewOps for NoopViewOps {}

struct NoopTextOps;
impl TextOps for NoopTextOps {}
