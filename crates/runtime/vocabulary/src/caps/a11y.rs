//! Accessibility, per-frame animation writes, and dev-time
//! introspection / geometry / screenshots.

use runtime_shared::accessibility::{AccessibilityProps, AccessibilityTree, LiveRegionPriority, Role};
use runtime_shared::animation::AnimProp;
use runtime_shared::introspect::NativeNode;
use runtime_shared::primitives::portal::ViewportRect;
use runtime_shared::Screenshot;
use runtime_scene::Host;

/// The accessibility surface that does NOT live on `create_*` prop
/// bags: reactive prop-bag replacement, live-region announcements, and
/// the GPU-backend parallel semantics tree. Serves the handlers'
/// reactive a11y effect and the `announce()` free function's installer.
pub trait A11yOps: Host {
    /// Replace the accessibility prop bag on an existing node.
    /// `inferred_role` is the primitive's default role, so update
    /// resolves the same role as create.
    #[allow(unused_variables)]
    fn update_accessibility(
        &mut self,
        node: &Self::Node,
        a11y: &AccessibilityProps,
        inferred_role: Option<Role>,
    ) {
        // default: no-op
    }

    /// Post a one-shot live-region announcement (no focus target).
    #[allow(unused_variables)]
    fn announce_for_accessibility(&mut self, msg: &str, priority: LiveRegionPriority) {
        // default: no-op
    }

    /// GPU/canvas backends: snapshot the parallel semantics tree.
    /// Native widget backends return `None`.
    fn dump_accessibility_tree(&self) -> Option<AccessibilityTree> {
        None
    }
}

/// Per-frame animated property writes from `AnimatedValue` listeners.
/// Serves `runtime_shared::animation::binding` (through handles) and the
/// backends' fast property-write paths.
pub trait AnimationOps: Host {
    /// Per-frame write of an animated scalar property.
    #[allow(unused_variables)]
    fn set_animated_f32(&mut self, node: &Self::Node, prop: AnimProp, value: f32) {
        // default: no-op
    }

    /// Per-frame write of an animated color (`[r, g, b, a]` sRGB,
    /// channels in `0..=1`).
    #[allow(unused_variables)]
    fn set_animated_color(&mut self, node: &Self::Node, prop: AnimProp, value: [f32; 4]) {
        // default: no-op
    }
}

/// Dev/robot-facing geometry and introspection: frames in three
/// coordinate systems, native render-tree reads, and real-surface
/// screenshots. Serves the robot verbs (`frame`,
/// `absolute_frame`, `device_frame`, `introspect_native`,
/// `capture_screenshot`) and overlay anchoring.
pub trait IntrospectionOps: Host {
    /// Node's rect in its parent's coordinate system.
    #[allow(unused_variables)]
    fn frame(&self, node: &Self::Node) -> Option<ViewportRect> {
        None
    }

    /// Node's rect in window/viewport coordinates (logical px).
    #[allow(unused_variables)]
    fn absolute_frame(&self, node: &Self::Node) -> Option<ViewportRect> {
        None
    }

    /// Node's rect in physical device-screen pixels (OS-level input
    /// injection).
    #[allow(unused_variables)]
    fn device_frame(&self, node: &Self::Node) -> Option<ViewportRect> {
        None
    }

    /// Whether the toolkit shows `node`: attached to a window, not hidden
    /// (itself or an ancestor), not fully transparent. The robot's
    /// `visible` is this AND a non-empty laid-out box (the rule Playwright
    /// uses). `None` = this backend can't tell, and the robot falls back on
    /// the box alone.
    #[allow(unused_variables)]
    fn is_shown(&self, node: &Self::Node) -> Option<bool> {
        None
    }

    /// Whether [`introspect_native`](Self::introspect_native) works here.
    fn supports_native_introspection(&self) -> bool {
        false
    }

    /// Read the platform-native render tree rooted at `node` — values
    /// from the live native object, never echoed from style structs.
    #[allow(unused_variables)]
    fn introspect_native(&self, node: &Self::Node) -> Option<NativeNode> {
        None
    }

    /// Note that `node` is a framework-primitive root (introspection
    /// boundary walk).
    #[allow(unused_variables)]
    fn note_introspection_root(&self, node: &Self::Node) {}

    /// Whether [`capture_screenshot`](Self::capture_screenshot) works here.
    fn supports_screenshot(&self) -> bool {
        false
    }

    /// Capture the real rendered surface as PNG bytes, delivered via
    /// `done` (sync backends invoke it inline).
    #[allow(unused_variables)]
    fn capture_screenshot(&self, done: Box<dyn FnOnce(Result<Screenshot, String>)>) {
        done(Err("screenshot capture is not supported on this backend".into()));
    }

    /// Whether the topmost view at `(x, y)` — logical px in
    /// [`absolute_frame`](Self::absolute_frame)'s space, in `node`'s
    /// window — is `node` or one of its descendants: whether a real tap
    /// there would reach it, or something on top (an overlay, a scrim, a
    /// sibling drawn later) would take it. Honors `pointer_events`.
    /// `None` = this backend can't tell.
    #[allow(unused_variables)]
    fn hit_contains(&self, node: &Self::Node, x: f32, y: f32) -> Option<bool> {
        None
    }

    /// Real platform input for `node`'s window: the pointer `steps` (in
    /// [`absolute_frame`](Self::absolute_frame)'s space) as events the
    /// toolkit receives the way it receives a user's — hit testing,
    /// gesture recognizers, the native disabled state, the app pointer
    /// sink. `None` = this backend can't synthesize input.
    ///
    /// Returns the injection rather than performing it: the robot runs it
    /// after releasing the backend borrow, because delivering an event
    /// runs author code that may borrow the backend itself (as a real
    /// event, which arrives with nothing borrowed, can). An injection may
    /// deliver synchronously or queue the events for the toolkit's own
    /// event loop; the robot's callers wait for the effect either way.
    #[allow(unused_variables)]
    fn native_pointer(&self, node: &Self::Node, steps: &[PointerStep]) -> Option<NativeInjection> {
        None
    }
}

/// One step of a [`IntrospectionOps::native_pointer`] gesture: the primary
/// button (or one finger) going down, moving while down, or lifting — or a
/// mouse moving with no button held ([`PointerStepPhase::Hover`]) — at
/// `(x, y)` logical px in the window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointerStep {
    pub phase: PointerStepPhase,
    pub x: f32,
    pub y: f32,
}

/// See [`PointerStep`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerStepPhase {
    Down,
    Move,
    Up,
    /// The pointer moves to the point with no button held: what drives
    /// hover (`on_hover`, the `state hovered` style). Touch backends have
    /// no hovering pointer and answer `None` for a gesture containing it.
    Hover,
}

/// A prepared [`IntrospectionOps::native_pointer`] injection; `Err` says
/// why the platform refused it.
pub type NativeInjection = Box<dyn FnOnce() -> Result<(), String>>;
