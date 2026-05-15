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

use crate::primitives;
use crate::style::{Color, StyleRules};
use crate::{ButtonHandle, ButtonOps, StateBits, TextHandle, TextOps, ViewHandle, ViewOps};

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
// Backend trait
// ---------------------------------------------------------------------------

pub trait Backend {
    type Node: Clone;

    fn create_view(&mut self) -> Self::Node;
    fn create_text(&mut self, content: &str) -> Self::Node;
    fn create_button(&mut self, label: &str, on_click: Rc<dyn Fn()>) -> Self::Node;
    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node);
    fn update_text(&mut self, node: &Self::Node, content: &str);

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

    /// Called when a styled node is being torn down (its surrounding
    /// `Effect` scope is dropping). Lets backends free per-node state —
    /// e.g. the web backend drops the node's dynamic CSS class slot
    /// and its node-id entry. Other backends typically don't need this.
    #[allow(unused_variables)]
    fn on_node_unstyled(&mut self, node: &Self::Node) {
        // default: no-op
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
    ) {
        // default: no-op; backends that don't implement Navigator
        // never get called here (the framework only invokes this
        // alongside `create_navigator`).
    }

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
    /// Default impl is `unimplemented!()` for backends that haven't
    /// yet implemented Link. Same posture as every other optional
    /// primitive.
    #[allow(unused_variables)]
    fn create_link(&mut self, config: primitives::link::LinkConfig) -> Self::Node {
        unimplemented!("create_link not implemented for this backend")
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

struct NoopButtonOps;
impl ButtonOps for NoopButtonOps {
    fn click(&self, _node: &dyn Any) {}
}

struct NoopViewOps;
impl ViewOps for NoopViewOps {}

struct NoopTextOps;
impl TextOps for NoopTextOps {}
