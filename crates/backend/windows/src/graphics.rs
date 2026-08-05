//! `Element::Graphics` — a DirectComposition visual handed to wgpu.
//!
//! Formerly a child HWND; now a visual in the backend's
//! [`crate::dcomp::CompositionTree`] (see that module's header for the
//! why — scroll-atomic movement, antialiased rounded clips, no
//! `SetWindowPos`/`SetWindowRgn`). What survives from the HWND era:
//!
//! - `on_ready` fires DEFERRED, from the layout pass, once the node
//!   has a real laid-out size (the `OnReadyEvent` contract: "the
//!   surface is in the layout tree and has a real size"). The
//!   readiness decision is [`layout_event`]; dispatch plumbing lives
//!   in `lib.rs`.
//! - `scale` is reported as `1.0` — the Win32 host is DPI-unaware
//!   today, and `1.0` is the documented "not yet reported" value.
//!
//! The surface handed to the author is a
//! [`GraphicsSurface::new_composed`] wrapper: `window_handle()`
//! answers `Unavailable` (there is deliberately no HWND — a consumer
//! that ignored the composed target would otherwise build a swapchain
//! over the whole host window), and the [`ComposedTarget`] capability
//! carries the visual pointer, the device commit hook, and the live
//! visibility flag the wgpu host polls each frame.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WindowHandle,
    WindowsDisplayHandle,
};
use runtime_shared::primitives::graphics::{
    ComposedTarget, GraphicsSurface, OnLost, OnReady, OnResize,
};
use windows::core::Interface;
use windows::Win32::Graphics::DirectComposition::{IDCompositionDevice, IDCompositionVisual};

// =========================================================================
// Per-node state
// =========================================================================

/// Everything the backend keeps for one live graphics node, keyed by
/// node id in `WindowsBackend::graphics`.
pub(crate) struct GraphicsState {
    /// The composed-provider wrapper, cloned into each `OnReadyEvent`.
    pub surface: GraphicsSurface,
    /// The node's visual chain (square-clip container + content the
    /// swapchain binds to) — positioned/clipped by
    /// `position_native_children`, detached by `remove_subtree`.
    pub visuals: crate::dcomp::VisualPair,
    /// Shared with the provider's [`ComposedTarget::is_visible`];
    /// written by the positioning walk (portal-hidden ⇒ false).
    pub visible: Arc<AtomicBool>,
    /// Last placement written to the visual — the diff baseline so an
    /// unchanged layout pass writes (and commits) nothing.
    pub last_placement: Option<crate::dcomp::Placement>,
    /// Consumed by the first successful layout — `None` afterward
    /// doubles as the "ready" flag [`layout_event`] keys on.
    pub on_ready: Option<OnReady>,
    /// `Option` so the dispatcher can take-call-restore the `FnMut`
    /// box without holding the backend borrow across author code.
    pub on_resize: Option<OnResize>,
    pub on_lost: Option<OnLost>,
    /// Last physical size reported (seeded by ready, updated by
    /// resize) — the dedupe baseline.
    pub last_size: Option<(u32, u32)>,
}

/// What the layout pass decided a graphics node needs dispatched.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum GfxEvent {
    Ready,
    Resize,
}

/// Readiness / resize decision for a graphics node at its laid-out
/// `frame` (w, h). `None` = nothing to dispatch: a degenerate ≤1px
/// frame (mid-layout — the simulator mounts from a lazy chunk before
/// its column has width), or a size unchanged from the last report.
/// The first real size is `Ready`; later real changes are `Resize`.
pub(crate) fn layout_event(
    ready: bool,
    last: Option<(u32, u32)>,
    frame: (f32, f32),
) -> Option<GfxEvent> {
    let w = frame.0.round().max(0.0) as u32;
    let h = frame.1.round().max(0.0) as u32;
    if w <= 1 || h <= 1 {
        return None;
    }
    if !ready {
        return Some(GfxEvent::Ready);
    }
    if last == Some((w, h)) {
        return None;
    }
    Some(GfxEvent::Resize)
}

// =========================================================================
// The composed surface provider
// =========================================================================

/// Provider behind the `GraphicsSurface`: owns a reference to the
/// node's visual + the composition device, and the shared visibility
/// flag.
///
/// # Send + Sync
///
/// wgpu requires the provider bounds. The COM pointers are only ever
/// dereferenced on the UI thread — the backend, the host's render
/// loop, and every author callback run there (same single-thread
/// argument the old HWND provider documented; the values are freely
/// copyable, the safety contract is lifetime + thread-of-use).
struct WinCompProvider {
    visual: IDCompositionVisual,
    device: IDCompositionDevice,
    visible: Arc<AtomicBool>,
}

unsafe impl Send for WinCompProvider {}
unsafe impl Sync for WinCompProvider {}

impl HasWindowHandle for WinCompProvider {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        // Deliberate: there is no per-surface window. Consumers must
        // use the ComposedTarget capability; handing out the host HWND
        // here would let a naive consumer swapchain over the whole app.
        Err(HandleError::Unavailable)
    }
}

impl HasDisplayHandle for WinCompProvider {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(unsafe { DisplayHandle::borrow_raw(WindowsDisplayHandle::new().into()) })
    }
}

impl ComposedTarget for WinCompProvider {
    fn visual(&self) -> std::ptr::NonNull<std::ffi::c_void> {
        std::ptr::NonNull::new(self.visual.as_raw()).expect("live COM pointer is non-null")
    }

    fn commit(&self) {
        unsafe {
            let _ = self.device.Commit();
        }
    }

    fn is_visible(&self) -> bool {
        self.visible.load(Ordering::Relaxed)
    }
}

/// Wrap a node's visual for the `OnReadyEvent`.
pub(crate) fn make_surface(
    visual: IDCompositionVisual,
    device: IDCompositionDevice,
    visible: Arc<AtomicBool>,
) -> GraphicsSurface {
    let provider = Arc::new(WinCompProvider { visual, device, visible });
    GraphicsSurface::new_composed(provider.clone(), provider)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Port of the macOS `resize_decision` regression suite plus the
    /// ready-first rule: the first non-degenerate layout is `Ready`,
    /// later real changes are `Resize`, degenerate/unchanged sizes
    /// dispatch nothing.
    #[test]
    fn first_real_size_is_ready() {
        assert_eq!(layout_event(false, None, (0.0, 0.0)), None);
        assert_eq!(layout_event(false, None, (1.0, 300.0)), None, "degenerate width");
        assert_eq!(layout_event(false, None, (300.0, 649.0)), Some(GfxEvent::Ready));
    }

    #[test]
    fn regression_resize_fires_on_real_change_after_ready() {
        assert_eq!(
            layout_event(true, Some((420, 748)), (600.0, 337.0)),
            Some(GfxEvent::Resize)
        );
    }

    #[test]
    fn resize_skips_unchanged_and_degenerate() {
        assert_eq!(layout_event(true, Some((420, 748)), (420.0, 748.0)), None);
        assert_eq!(layout_event(true, Some((420, 748)), (1.0, 1.0)), None);
        assert_eq!(layout_event(true, Some((420, 748)), (600.0, 1.0)), None);
    }
}
