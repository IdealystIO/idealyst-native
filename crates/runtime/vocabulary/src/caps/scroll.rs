//! Scrolling surfaces, safe-area treatment, and the virtualized list.

use std::rc::Rc;

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives;
use runtime_core::{SafeAreaSides, VirtualizerCallbacks};
use runtime_scene::Host;

use super::noop;
use super::ExternalOps;

/// The `scroll_view` primitive plus generic node scroll-offset access
/// (the navigator substrate's URL-sync scroll snapshot/restore reads
/// any node's offset, so `node_scroll`/`set_node_scroll` live here with
/// the scrolling capability rather than on the navigator). Serves
/// `walker/scroll_view.rs` and `walker/navigator.rs`.
pub trait ScrollOps: Host {
    /// Create a scrolling container (`horizontal` selects the axis;
    /// `on_scroll` fires per offset change in CSS px / native points).
    #[allow(unused_variables)]
    fn create_scroll_view(
        &mut self,
        horizontal: bool,
        on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        unimplemented!("create_scroll_view not implemented for this backend")
    }

    /// Read `node`'s scroll offset `(x, y)`; `(0, 0)` when the node
    /// isn't a scroll surface.
    #[allow(unused_variables)]
    fn node_scroll(&self, node: &Self::Node) -> (f32, f32) {
        (0.0, 0.0)
    }

    /// Set `node`'s scroll offset (harmless no-op on non-scrollers,
    /// matching DOM semantics).
    #[allow(unused_variables)]
    fn set_node_scroll(&mut self, node: &Self::Node, x: f32, y: f32) {}

    /// Imperative-ref handle for a scroll view. Default: no-op.
    #[allow(unused_variables)]
    fn make_scroll_view_handle(&self, node: &Self::Node) -> primitives::scroll_view::ScrollViewHandle {
        primitives::scroll_view::ScrollViewHandle::new(Rc::new(()), &noop::NoopScrollViewOps)
    }
}

/// Safe-area treatment for opted-in containers (`.safe_area(...)`).
/// Serves the safe-area effect in `walker/style.rs`; the walker picks
/// padding for views and content insets for scroll views.
pub trait SafeAreaOps: Host {
    /// Add the platform's safe-area insets to the flagged sides'
    /// padding (combining with author padding, not clobbering).
    #[allow(unused_variables)]
    fn apply_safe_area_padding(&mut self, node: &Self::Node, sides: SafeAreaSides) {
        // default: no-op
    }

    /// Scroll-view-correct safe area: surface bleeds edge-to-edge,
    /// content origin is inset. Default falls back to
    /// [`apply_safe_area_padding`](Self::apply_safe_area_padding).
    #[allow(unused_variables)]
    fn apply_scroll_view_safe_area_inset(&mut self, node: &Self::Node, sides: SafeAreaSides) {
        self.apply_safe_area_padding(node, sides);
    }
}

/// The `flat_list` / `virtualizer` primitive — backend-owned windowed
/// rendering over framework-mounted rows. Serves
/// `walker/virtualizer.rs` + `walker/cleanup.rs`.
pub trait VirtualizerOps: ExternalOps {
    /// Create a virtualized list; the backend owns the visible-window
    /// math and calls the callback bundle to mount/release rows.
    #[allow(unused_variables)]
    fn create_virtualizer(
        &mut self,
        callbacks: VirtualizerCallbacks<Self::Node>,
        overscan: f32,
        layout: primitives::virtualizer::VirtualLayout,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.missing_primitive_placeholder("virtualizer (backend compiled without `prim-virtualizer`)")
    }

    /// The data set changed: re-query count/key/size and diff.
    #[allow(unused_variables)]
    fn virtualizer_data_changed(&mut self, node: &Self::Node) {}

    /// Tear down backend-side state (listeners, closure handles) so
    /// queued platform events can't call into freed per-item scopes.
    #[allow(unused_variables)]
    fn release_virtualizer(&mut self, node: &Self::Node) {
        // default no-op
    }

    /// Imperative-ref handle for a virtualizer. Default: no-op.
    #[allow(unused_variables)]
    fn make_virtualizer_handle(&self, node: &Self::Node) -> primitives::virtualizer::VirtualizerHandle {
        primitives::virtualizer::VirtualizerHandle::new(Rc::new(()), &noop::NoopVirtualizerOps)
    }
}
