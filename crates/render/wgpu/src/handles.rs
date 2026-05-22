//! Static `*Ops` impls for the wgpu backend's primitive handles.
//!
//! Each Ops trait is implemented by a zero-sized type with a
//! `'static` instance the backend references from `make_*_handle`.
//! The methods receive a type-erased `&dyn Any` (the cloned
//! [`WgpuNode`]) which we downcast back to fetch the Taffy frame
//! through the globally-installed backend.
//!
//! Without these, framework helpers like `view_ref.frame()` and
//! `view_ref.absolute_frame()` (used by the welcome example's
//! raf-driver to read viewport dims) silently return `None`. The
//! defaults in `framework_core::handles` are no-op stubs.

use std::any::Any;

use framework_core::primitives::portal::ViewportRect;
use framework_core::{TextOps, ViewOps};

use crate::node::WgpuNode;

/// Look up the Taffy frame for `node` through the globally-installed
/// backend (see [`crate::backend_impl::install_global_self`]). Returns
/// `None` if no backend is installed, the install has been dropped, or
/// the backend is currently borrowed (e.g. mid-`apply_style`).
fn frame_of(node: &dyn Any) -> Option<ViewportRect> {
    let n = node.downcast_ref::<WgpuNode>()?;
    let weak = crate::backend_impl::global_self()?;
    let rc = weak.upgrade()?;
    let backend = rc.try_borrow().ok()?;
    use framework_core::Backend;
    backend.frame(n)
}

/// Like [`frame_of`] but returns the absolute (viewport-relative)
/// rect. Walks the wgpu node tree on every call — cheap because the
/// tree is small and the walk is read-only.
fn absolute_frame_of(node: &dyn Any) -> Option<ViewportRect> {
    let n = node.downcast_ref::<WgpuNode>()?;
    let weak = crate::backend_impl::global_self()?;
    let rc = weak.upgrade()?;
    let backend = rc.try_borrow().ok()?;
    use framework_core::Backend;
    backend.absolute_frame(n)
}

pub(crate) struct WgpuViewOps;
pub(crate) static WGPU_VIEW_OPS: WgpuViewOps = WgpuViewOps;

impl ViewOps for WgpuViewOps {
    fn rect(&self, node: &dyn Any) -> ViewportRect {
        absolute_frame_of(node).unwrap_or_default()
    }
    fn frame(&self, node: &dyn Any) -> Option<ViewportRect> {
        frame_of(node)
    }
    fn absolute_frame(&self, node: &dyn Any) -> Option<ViewportRect> {
        absolute_frame_of(node)
    }
    // Without these the trait's default no-op runs, so every
    // `AnimatedValue::bind` write silently drops on the floor —
    // the welcome example's planets/vignette/glare all stay at
    // their stylesheet (opacity 0) values forever.
    fn set_animated_f32(
        &self,
        node: &dyn Any,
        prop: framework_core::animation::AnimProp,
        value: f32,
    ) {
        if let Some(n) = node.downcast_ref::<WgpuNode>() {
            crate::backend_impl::set_animated_f32(n, prop, value);
        }
    }
    fn set_animated_color(
        &self,
        node: &dyn Any,
        prop: framework_core::animation::AnimProp,
        value: [f32; 4],
    ) {
        if let Some(n) = node.downcast_ref::<WgpuNode>() {
            crate::backend_impl::set_animated_color(n, prop, value);
        }
    }
}

pub(crate) struct WgpuTextOps;
pub(crate) static WGPU_TEXT_OPS: WgpuTextOps = WgpuTextOps;

impl TextOps for WgpuTextOps {
    fn set_animated_color(
        &self,
        node: &dyn Any,
        prop: framework_core::animation::AnimProp,
        value: [f32; 4],
    ) {
        if let Some(n) = node.downcast_ref::<WgpuNode>() {
            crate::backend_impl::set_animated_color(n, prop, value);
        }
    }
}
