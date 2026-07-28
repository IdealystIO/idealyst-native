//! Accessibility, per-frame animation writes, and dev-time
//! introspection / geometry / screenshots.

use runtime_core::accessibility::{AccessibilityProps, AccessibilityTree, LiveRegionPriority, Role};
use runtime_core::animation::AnimProp;
use runtime_core::introspect::NativeNode;
use runtime_core::primitives::portal::ViewportRect;
use runtime_core::Screenshot;
use runtime_scene::Host;

/// The accessibility surface that does NOT live on `create_*` prop
/// bags: reactive prop-bag replacement, live-region announcements, and
/// the GPU-backend parallel semantics tree. Serves the walker's
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
/// Serves `runtime_core::animation::binding` (through handles) and the
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
/// screenshots. Serves `walker.rs`'s robot verbs (`frame`,
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
}
