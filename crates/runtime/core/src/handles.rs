//! Primitive handles + backend ops + state bitflags + `RefFill`.
//!
//! Each primitive kind has a corresponding handle type that the parent
//! reaches via a `Ref<Handle>`. A handle is a thin record:
//!   - `node`: an `Rc<dyn Any>` holding the backend's concrete node value
//!     (`web_sys::HtmlButtonElement` on web, `View` on Android, …).
//!   - `ops`: a `&'static dyn …Ops` trait object providing the kind's
//!     methods. Backends ship a single ZST `Ops` impl per kind.
//!
//! This shape keeps `Ref<Handle>` backend-agnostic in user code while
//! letting the backend implement methods against its native node type
//! via a single downcast inside each op.
//!
//! Also home to `StateBits` (the interaction-state bitmask) and
//! `RefFill` (the type-erased enum of mount-time ref-fill closures
//! `Primitive` variants carry).

use crate::primitives;
use std::any::Any;
use std::rc::Rc;

// =============================================================================
// StateBits
// =============================================================================

/// Bitflags for interaction states the framework recognizes. Backends
/// flip these bits when corresponding native events fire (hover,
/// press, focus, disabled state). Each bit corresponds to one of the
/// `__state_*` axes a `stylesheet!` may declare as `state hovered`
/// etc. — when the bit is on, the framework adds the axis to the
/// node's `StyleApplication` so the overlay applies.
///
/// Only the listed states are supported, matching the cross-platform
/// contract enforced by the `stylesheet!` macro. Adding more would
/// need backend + macro updates in lockstep.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct StateBits(pub u8);

impl StateBits {
    pub const HOVERED: StateBits = StateBits(1 << 0);
    pub const PRESSED: StateBits = StateBits(1 << 1);
    pub const FOCUSED: StateBits = StateBits(1 << 2);
    pub const DISABLED: StateBits = StateBits(1 << 3);

    pub const NONE: StateBits = StateBits(0);

    pub fn contains(self, other: StateBits) -> bool {
        (self.0 & other.0) == other.0
    }

    pub fn with(self, other: StateBits) -> StateBits {
        StateBits(self.0 | other.0)
    }

    pub fn without(self, other: StateBits) -> StateBits {
        StateBits(self.0 & !other.0)
    }

    /// The CSS-axis name for this bit, used in `StyleApplication`
    /// variant lookups. Returns `None` for empty (zero) bits.
    pub fn axis_name(self) -> Option<&'static str> {
        match self {
            Self::HOVERED => Some("__state_hovered"),
            Self::PRESSED => Some("__state_pressed"),
            Self::FOCUSED => Some("__state_focused"),
            Self::DISABLED => Some("__state_disabled"),
            _ => None,
        }
    }

    /// Iterate the set bits in this bitmask, yielding their
    /// `__state_*` axis names. Used by the framework to build a
    /// `VariantSet` for resolution from the current active states.
    pub fn active_axes(self) -> impl Iterator<Item = &'static str> {
        [Self::HOVERED, Self::PRESSED, Self::FOCUSED, Self::DISABLED]
            .into_iter()
            .filter(move |&bit| self.contains(bit))
            .filter_map(|bit| bit.axis_name())
    }
}

// =============================================================================
// ButtonHandle / ViewHandle / TextHandle + their Ops traits
// =============================================================================

/// A handle to a mounted `Button` primitive.
///
/// `Clone` is cheap: an `Rc` bump plus copying a `'static` pointer.
/// Cloning is what lets `Ref::get()` hand back an owned handle rather
/// than forcing callers through a `.with(|h| ...)` closure.
#[derive(Clone)]
pub struct ButtonHandle {
    node: Rc<dyn Any>,
    ops: &'static dyn ButtonOps,
}

impl ButtonHandle {
    /// Backend constructor. Called by `Backend::make_button_handle`
    /// impls. The `node` is type-erased here so user code can hold
    /// `Ref<ButtonHandle>` without naming the backend's node type.
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn ButtonOps) -> Self {
        Self { node, ops }
    }

    /// Programmatically triggers the button's click handler (and on
    /// platforms with native click semantics, dispatches the native
    /// event).
    pub fn click(&self) {
        self.ops.click(&*self.node);
    }
}

pub trait ButtonOps {
    fn click(&self, node: &dyn Any);
    /// Viewport-relative rect, used when a `Button` is the anchor
    /// target of an `Overlay`. Default returns the zero rect, which
    /// causes overlays to fall back to viewport-centered placement.
    /// Backends that can measure (web `getBoundingClientRect`,
    /// iOS `UIView.frame`, Android `View.getLocationOnScreen`)
    /// override to return real values.
    #[allow(unused_variables)]
    fn rect(&self, node: &dyn Any) -> primitives::portal::ViewportRect {
        primitives::portal::ViewportRect { x: 0.0, y: 0.0, width: 0.0, height: 0.0 }
    }
}

impl primitives::portal::AnchorableHandle for ButtonHandle {
    fn rect(&self) -> primitives::portal::ViewportRect {
        self.ops.rect(&*self.node)
    }
}

/// A handle to a mounted `Pressable` primitive.
///
/// Same shape as [`ButtonHandle`] — the two diverge only in the
/// underlying primitive's children-vs-label contract. Authors that
/// want a clickable container with custom children (icon + label,
/// row of children, etc.) use `Pressable`; authors that want a bare
/// label button use `Button`.
///
/// Like `ButtonHandle`, it carries an `AnchorableHandle` impl so a
/// `Pressable` can be the anchor target of an `Overlay`.
#[derive(Clone)]
pub struct PressableHandle {
    node: Rc<dyn Any>,
    ops: &'static dyn PressableOps,
}

impl PressableHandle {
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn PressableOps) -> Self {
        Self { node, ops }
    }

    /// Fire the press callback. On platforms with native press
    /// semantics this also dispatches a native event so any UA
    /// hooks (analytics, focus shifts) run.
    pub fn click(&self) {
        self.ops.click(&*self.node);
    }
}

pub trait PressableOps {
    fn click(&self, node: &dyn Any);
    /// Viewport-relative rect, mirroring [`ButtonOps::rect`]. Used
    /// when a `Pressable` is the anchor target of an `Overlay`.
    #[allow(unused_variables)]
    fn rect(&self, node: &dyn Any) -> primitives::portal::ViewportRect {
        primitives::portal::ViewportRect { x: 0.0, y: 0.0, width: 0.0, height: 0.0 }
    }
}

impl primitives::portal::AnchorableHandle for PressableHandle {
    fn rect(&self) -> primitives::portal::ViewportRect {
        self.ops.rect(&*self.node)
    }
}

/// A handle to a mounted `View` primitive.
#[derive(Clone)]
pub struct ViewHandle {
    node: Rc<dyn Any>,
    #[allow(dead_code)]
    ops: &'static dyn ViewOps,
}

impl ViewHandle {
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn ViewOps) -> Self {
        Self { node, ops }
    }

    /// Type-erased access to the backend's native node. Each
    /// backend stores its own `Node` type behind an `Rc<dyn Any>`
    /// here; framework helpers (notably `LayoutPlan`'s outlet
    /// resolution) downcast it back to the concrete type.
    pub fn as_any(&self) -> &dyn Any {
        &*self.node
    }
}

pub trait ViewOps {
    /// Viewport-relative rect for overlay anchoring. Returns the zero
    /// rect as a sentinel meaning "centered fallback" — overlays rely
    /// on this contract, so it stays non-`Option`. Public callers
    /// should prefer [`ViewOps::absolute_frame`] which distinguishes
    /// "not yet mounted" from "at origin".
    #[allow(unused_variables)]
    fn rect(&self, node: &dyn Any) -> primitives::portal::ViewportRect {
        primitives::portal::ViewportRect { x: 0.0, y: 0.0, width: 0.0, height: 0.0 }
    }

    /// Parent-relative rect. `None` when the view isn't laid out yet
    /// or the backend doesn't expose it.
    #[allow(unused_variables)]
    fn frame(&self, node: &dyn Any) -> Option<primitives::portal::ViewportRect> {
        None
    }

    /// Viewport/window-relative rect. `None` when the view isn't
    /// mounted in a window yet or the backend doesn't expose it.
    /// Symmetric with `frame` — neither method invents a zero rect
    /// to paper over the unmounted case.
    #[allow(unused_variables)]
    fn absolute_frame(&self, node: &dyn Any) -> Option<primitives::portal::ViewportRect> {
        None
    }

    /// Write a per-frame scalar animation property (opacity, scale,
    /// translateX/Y, rotateZ) onto the backend's native node. This is
    /// what `AnimatedValue::bind` dispatches through — author code
    /// holds an `AnimatedValue<f32>` and a `Ref<ViewHandle>`, the
    /// framework routes per-frame writes through here so no
    /// per-platform plumbing leaks into app code.
    ///
    /// Backends downcast `node` to their concrete `Node` type and
    /// call their existing `set_animated_f32` writer. Default is a
    /// silent no-op so backends without animated-prop support don't
    /// have to override.
    #[allow(unused_variables)]
    fn set_animated_f32(
        &self,
        node: &dyn Any,
        prop: crate::animation::AnimProp,
        value: f32,
    ) {
    }

    /// Color-family counterpart of [`ViewOps::set_animated_f32`].
    /// Handles `BackgroundColor`, `ForegroundColor`, and per-stop
    /// `GradientStopColor(idx)`. `value` is sRGB `[r, g, b, a]` with
    /// all channels in `0..=1`.
    #[allow(unused_variables)]
    fn set_animated_color(
        &self,
        node: &dyn Any,
        prop: crate::animation::AnimProp,
        value: [f32; 4],
    ) {
    }
}

impl ViewHandle {
    /// Rect in the parent's coordinate system. `None` if the view
    /// isn't laid out yet or the backend doesn't expose it.
    pub fn frame(&self) -> Option<primitives::portal::ViewportRect> {
        self.ops.frame(&*self.node)
    }

    /// Rect in viewport (window) coordinates. `None` if the view
    /// isn't mounted in a window yet.
    pub fn absolute_frame(&self) -> Option<primitives::portal::ViewportRect> {
        self.ops.absolute_frame(&*self.node)
    }

    /// Write a scalar animation property onto the underlying node.
    /// Thin wrapper around [`ViewOps::set_animated_f32`]; the framework's
    /// [`AnimatedValue::bind`](crate::animation::AnimatedValue::bind)
    /// helper calls this from inside its subscriber so authors don't
    /// have to write per-platform downcast code.
    pub fn set_animated_f32(&self, prop: crate::animation::AnimProp, value: f32) {
        self.ops.set_animated_f32(&*self.node, prop, value);
    }

    /// Write a color animation property onto the underlying node.
    /// Mirror of [`Self::set_animated_f32`]; called from
    /// [`AnimatedValue::bind_color`](crate::animation::AnimatedValue::bind_color)
    /// and [`AnimatedValue::bind_gradient_stop`](crate::animation::AnimatedValue::bind_gradient_stop).
    pub fn set_animated_color(&self, prop: crate::animation::AnimProp, value: [f32; 4]) {
        self.ops.set_animated_color(&*self.node, prop, value);
    }
}

impl primitives::portal::AnchorableHandle for ViewHandle {
    fn rect(&self) -> primitives::portal::ViewportRect {
        self.ops.rect(&*self.node)
    }
}

/// A handle to a mounted `Text` primitive.
#[derive(Clone)]
pub struct TextHandle {
    #[allow(dead_code)]
    node: Rc<dyn Any>,
    #[allow(dead_code)]
    ops: &'static dyn TextOps,
}

impl TextHandle {
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn TextOps) -> Self {
        Self { node, ops }
    }

    /// Backend-erased view of the mounted node. Downcast to the
    /// backend's concrete node type (e.g. `IosNode`, `web_sys::Node`)
    /// to perform backend-specific work — same shape as
    /// [`ViewHandle::as_any`].
    pub fn as_any(&self) -> &dyn Any {
        &*self.node
    }
}

pub trait TextOps {
    /// Write a color animation property onto the text node — almost
    /// always `AnimProp::ForegroundColor`, which maps to
    /// `UILabel.textColor` on iOS, `TextView.setTextColor` on Android,
    /// and inline `style.color` on web. Bound from author code via
    /// [`AnimatedValue::bind_text_color`](crate::animation::AnimatedValue::bind_text_color).
    /// Default is a silent no-op.
    #[allow(unused_variables)]
    fn set_animated_color(
        &self,
        node: &dyn Any,
        prop: crate::animation::AnimProp,
        value: [f32; 4],
    ) {
    }
}

impl TextHandle {
    /// Write a color animation property onto the underlying text
    /// node. Thin wrapper around [`TextOps::set_animated_color`].
    pub fn set_animated_color(&self, prop: crate::animation::AnimProp, value: [f32; 4]) {
        self.ops.set_animated_color(&*self.node, prop, value);
    }
}

// =============================================================================
// RefOps + RefFill
// =============================================================================

/// Per-backend bundle of `Ops` trait objects, returned from
/// `Backend::ref_ops()`. The framework asks the backend for these once
/// (during render setup) and uses them to construct primitive handles
/// at mount time.
pub struct RefOps {
    pub button: &'static dyn ButtonOps,
    pub view: &'static dyn ViewOps,
    pub text: &'static dyn TextOps,
}

/// The mount-time closure that populates a `Ref<H>` slot. One variant
/// per primitive kind so the framework can build the correctly-typed
/// handle without runtime kind-matching on the closure itself. The
/// closure is monomorphic to `H`, so type-checking against the
/// call-site `Ref<H>` happens at `.bind()`. User code never constructs
/// this directly; it's exposed only because `Primitive`'s variants
/// carry it.
pub enum RefFill {
    Button(Box<dyn FnOnce(ButtonHandle)>),
    Pressable(Box<dyn FnOnce(PressableHandle)>),
    View(Box<dyn FnOnce(ViewHandle)>),
    Text(Box<dyn FnOnce(TextHandle)>),
    Icon(Box<dyn FnOnce(primitives::icon::IconHandle)>),
    Image(Box<dyn FnOnce(primitives::image::ImageHandle)>),
    TextInput(Box<dyn FnOnce(primitives::text_input::TextInputHandle)>),
    TextArea(Box<dyn FnOnce(primitives::text_area::TextAreaHandle)>),
    Toggle(Box<dyn FnOnce(primitives::toggle::ToggleHandle)>),
    ScrollView(Box<dyn FnOnce(primitives::scroll_view::ScrollViewHandle)>),
    Slider(Box<dyn FnOnce(primitives::slider::SliderHandle)>),
    ActivityIndicator(Box<dyn FnOnce(primitives::activity_indicator::ActivityIndicatorHandle)>),
    Virtualizer(Box<dyn FnOnce(primitives::virtualizer::VirtualizerHandle)>),
    Graphics(Box<dyn FnOnce(primitives::graphics::GraphicsHandle)>),
    Link(Box<dyn FnOnce(primitives::link::LinkHandle)>),
    Portal(Box<dyn FnOnce(primitives::portal::PortalHandle)>),
    /// Fill closure for third-party `Primitive::External` primitives.
    /// The framework hands the closure an `Rc<dyn Any>` wrapping the
    /// backend's native node; the third-party facade downcasts to
    /// build the user-facing `ExternalHandle<T>` before filling the
    /// `Ref`.
    External(Box<dyn FnOnce(Rc<dyn Any>)>),
    /// Fill closure for `Primitive::Navigator`. Hands the SDK a
    /// pre-built `NavigatorHandle` wired to the navigator's control
    /// plane; the SDK wraps it in its own kind-specific handle type
    /// (`StackHandle` / `TabsHandle` / `DrawerHandle` / custom) before
    /// filling the user's `Ref`.
    Navigator(Box<dyn FnOnce(primitives::navigator::NavigatorHandle)>),
    Presence(Box<dyn FnOnce(primitives::presence::PresenceHandle)>),
}

#[cfg(test)]
mod tests {
    //! Tests for the handle layer:
    //!
    //! - `StateBits` bitset arithmetic + axis-name mapping
    //! - Type-erased `Rc<dyn Any>` round-trips through `as_any` /
    //!   downcast (matches what backends do at handle construction)
    //! - `click()` / `rect()` route through the `&'static dyn …Ops`
    //!   trait object — verifies the dispatch pipe between user code
    //!   and backend ops
    //! - Cloning a handle is cheap (Rc bump, same underlying node)

    use super::*;
    use std::cell::Cell;

    // -----------------------------------------------------------------------
    // StateBits
    // -----------------------------------------------------------------------

    #[test]
    fn statebits_constants_have_disjoint_bits() {
        let bits = [
            StateBits::HOVERED,
            StateBits::PRESSED,
            StateBits::FOCUSED,
            StateBits::DISABLED,
        ];
        for (i, &a) in bits.iter().enumerate() {
            for &b in &bits[i + 1..] {
                assert_eq!(a.0 & b.0, 0, "{:?} and {:?} overlap", a, b);
            }
        }
    }

    #[test]
    fn statebits_none_contains_nothing_and_with_or_combines() {
        assert_eq!(StateBits::NONE.0, 0);
        let combined = StateBits::HOVERED.with(StateBits::FOCUSED);
        assert!(combined.contains(StateBits::HOVERED));
        assert!(combined.contains(StateBits::FOCUSED));
        assert!(!combined.contains(StateBits::PRESSED));
    }

    #[test]
    fn statebits_without_clears_listed_bits_only() {
        let combined = StateBits::HOVERED
            .with(StateBits::FOCUSED)
            .with(StateBits::PRESSED);
        let cleared = combined.without(StateBits::HOVERED);
        assert!(!cleared.contains(StateBits::HOVERED));
        assert!(cleared.contains(StateBits::FOCUSED));
        assert!(cleared.contains(StateBits::PRESSED));
    }

    #[test]
    fn statebits_axis_name_maps_to_known_names() {
        assert_eq!(StateBits::HOVERED.axis_name(), Some("__state_hovered"));
        assert_eq!(StateBits::PRESSED.axis_name(), Some("__state_pressed"));
        assert_eq!(StateBits::FOCUSED.axis_name(), Some("__state_focused"));
        assert_eq!(StateBits::DISABLED.axis_name(), Some("__state_disabled"));
        assert_eq!(StateBits::NONE.axis_name(), None);
        // Combined bits aren't a single named axis.
        let combined = StateBits::HOVERED.with(StateBits::FOCUSED);
        assert_eq!(combined.axis_name(), None);
    }

    #[test]
    fn statebits_active_axes_yields_set_bits_in_canonical_order() {
        let combined = StateBits::HOVERED
            .with(StateBits::FOCUSED)
            .with(StateBits::DISABLED);
        let axes: Vec<&'static str> = combined.active_axes().collect();
        // Iteration order: HOVERED, PRESSED, FOCUSED, DISABLED.
        // PRESSED isn't set so it's skipped.
        assert_eq!(
            axes,
            vec!["__state_hovered", "__state_focused", "__state_disabled"],
        );
    }

    #[test]
    fn statebits_active_axes_empty_when_none() {
        let axes: Vec<&'static str> = StateBits::NONE.active_axes().collect();
        assert!(axes.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test-only Ops impls
    // -----------------------------------------------------------------------

    /// Records that `click` was called and the `node` payload it
    /// received (downcast to `u32`).
    struct RecordingButtonOps {
        last_node: Cell<u32>,
        click_count: Cell<u32>,
    }

    // Make the struct shareable via static references in tests. We
    // need `&'static dyn ButtonOps` for `ButtonHandle::new`; a
    // `Box::leak` gives us that without unsafe.
    impl ButtonOps for RecordingButtonOps {
        fn click(&self, node: &dyn Any) {
            self.click_count.set(self.click_count.get() + 1);
            if let Some(&id) = node.downcast_ref::<u32>() {
                self.last_node.set(id);
            }
        }

        fn rect(&self, _node: &dyn Any) -> primitives::portal::ViewportRect {
            primitives::portal::ViewportRect {
                x: 10.0,
                y: 20.0,
                width: 100.0,
                height: 50.0,
            }
        }
    }

    fn make_recording_button_ops() -> &'static RecordingButtonOps {
        Box::leak(Box::new(RecordingButtonOps {
            last_node: Cell::new(0),
            click_count: Cell::new(0),
        }))
    }

    /// View ops that return non-default rects so we can confirm
    /// the handle's rect/frame methods route through.
    struct RecordingViewOps;
    impl ViewOps for RecordingViewOps {
        fn rect(&self, _node: &dyn Any) -> primitives::portal::ViewportRect {
            primitives::portal::ViewportRect {
                x: 1.0,
                y: 2.0,
                width: 3.0,
                height: 4.0,
            }
        }

        fn frame(&self, _node: &dyn Any) -> Option<primitives::portal::ViewportRect> {
            Some(primitives::portal::ViewportRect {
                x: 5.0,
                y: 6.0,
                width: 7.0,
                height: 8.0,
            })
        }

        fn absolute_frame(
            &self,
            _node: &dyn Any,
        ) -> Option<primitives::portal::ViewportRect> {
            Some(primitives::portal::ViewportRect {
                x: 9.0,
                y: 10.0,
                width: 11.0,
                height: 12.0,
            })
        }
    }

    struct StubTextOps;
    impl TextOps for StubTextOps {}

    // -----------------------------------------------------------------------
    // ButtonHandle
    // -----------------------------------------------------------------------

    #[test]
    fn button_handle_click_routes_to_ops_with_node_payload() {
        let ops = make_recording_button_ops();
        let handle = ButtonHandle::new(Rc::new(42u32), ops);

        assert_eq!(ops.click_count.get(), 0);
        handle.click();
        assert_eq!(ops.click_count.get(), 1);
        assert_eq!(ops.last_node.get(), 42, "ops.click received node payload");

        handle.click();
        assert_eq!(ops.click_count.get(), 2);
    }

    #[test]
    fn button_handle_anchorable_rect_returns_ops_rect() {
        use primitives::portal::AnchorableHandle;
        let ops = make_recording_button_ops();
        let handle = ButtonHandle::new(Rc::new(0u32), ops);
        let rect = handle.rect();
        assert_eq!(rect.x, 10.0);
        assert_eq!(rect.y, 20.0);
        assert_eq!(rect.width, 100.0);
        assert_eq!(rect.height, 50.0);
    }

    #[test]
    fn button_handle_clone_is_cheap_and_shares_node() {
        let ops = make_recording_button_ops();
        let payload: Rc<u32> = Rc::new(7u32);
        // 1 strong ref before wrapping
        assert_eq!(Rc::strong_count(&payload), 1);

        let handle: ButtonHandle = ButtonHandle::new(payload.clone() as Rc<dyn Any>, ops);
        // 2: original + the Rc<dyn Any> coercion
        assert_eq!(Rc::strong_count(&payload), 2);

        let h2 = handle.clone();
        // 3: the clone bumps the inner Rc<dyn Any>'s count
        assert_eq!(Rc::strong_count(&payload), 3);

        // Both clones invoke the same ops with the same payload.
        h2.click();
        assert_eq!(ops.click_count.get(), 1);
        assert_eq!(ops.last_node.get(), 7);

        drop(h2);
        // Back to 2 after the clone drops.
        assert_eq!(Rc::strong_count(&payload), 2);
    }

    // -----------------------------------------------------------------------
    // ViewHandle
    // -----------------------------------------------------------------------

    #[test]
    fn view_handle_as_any_round_trips_to_concrete_node() {
        static OPS: RecordingViewOps = RecordingViewOps;
        // Pretend the backend's Node is `String`.
        let handle = ViewHandle::new(Rc::new("v1".to_string()), &OPS);
        let any: &dyn Any = handle.as_any();
        let s: &String = any.downcast_ref().expect("downcast to backend node type");
        assert_eq!(s, "v1");
    }

    #[test]
    fn view_handle_frame_and_absolute_frame_delegate_to_ops() {
        static OPS: RecordingViewOps = RecordingViewOps;
        let handle = ViewHandle::new(Rc::new(0u32), &OPS);

        let frame = handle.frame().expect("ops returns Some");
        assert_eq!(frame.x, 5.0);
        assert_eq!(frame.y, 6.0);

        let abs = handle.absolute_frame().expect("ops returns Some");
        assert_eq!(abs.x, 9.0);
        assert_eq!(abs.y, 10.0);
    }

    #[test]
    fn view_handle_anchorable_rect_returns_ops_rect() {
        use primitives::portal::AnchorableHandle;
        static OPS: RecordingViewOps = RecordingViewOps;
        let handle = ViewHandle::new(Rc::new(0u32), &OPS);
        let rect = handle.rect();
        assert_eq!(rect.x, 1.0);
        assert_eq!(rect.y, 2.0);
        assert_eq!(rect.width, 3.0);
        assert_eq!(rect.height, 4.0);
    }

    // -----------------------------------------------------------------------
    // TextHandle
    // -----------------------------------------------------------------------

    #[test]
    fn text_handle_as_any_round_trips_to_concrete_node() {
        static OPS: StubTextOps = StubTextOps;
        let handle = TextHandle::new(Rc::new(123_i64), &OPS);
        let any = handle.as_any();
        let v: &i64 = any.downcast_ref().expect("downcast to text node payload");
        assert_eq!(*v, 123);
    }

    #[test]
    fn text_handle_clone_shares_payload_rc() {
        static OPS: StubTextOps = StubTextOps;
        let payload: Rc<u32> = Rc::new(5);
        assert_eq!(Rc::strong_count(&payload), 1);
        let handle = TextHandle::new(payload.clone() as Rc<dyn Any>, &OPS);
        assert_eq!(Rc::strong_count(&payload), 2);
        let _h2 = handle.clone();
        assert_eq!(Rc::strong_count(&payload), 3);
    }
}
