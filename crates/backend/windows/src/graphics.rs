//! `Element::Graphics` — a plain child HWND handed to wgpu.
//!
//! Mirrors the macOS backend's `MetalView` shape at
//! `crates/backend/macos/src/imp/graphics.rs`, translated to Win32:
//!
//! - The surface is a dedicated `IdealystSurface` child window with a
//!   `DefWindowProc` wndproc. wgpu's DX12 swapchain owns every pixel;
//!   the wndproc answers `WM_ERASEBKGND` with "handled" so GDI never
//!   flashes the class background between presents. `WS_CLIPSIBLINGS`
//!   plus the host's `WS_CLIPCHILDREN` keep the scene painter's blit
//!   off the surface.
//! - wgpu reaches the window via `raw_window_handle::Win32WindowHandle`
//!   ([`Win32SurfaceProvider`]) — the platform handle type changes but
//!   the call surface is identical to iOS/macOS.
//! - `on_ready` fires DEFERRED, from the layout pass, once the node
//!   has a real laid-out size (the `OnReadyEvent` contract: "the
//!   surface is in the layout tree and has a real size"). AppKit gets
//!   this via a 0-delay runloop callback after its own layout;
//!   Win32 layout is the framework's `layout_pass`, so that's where
//!   the readiness decision lives — see [`layout_event`] and the
//!   dispatch plumbing in `lib.rs`.
//!
//! `scale` is reported as `1.0` — the Win32 host is DPI-unaware today
//! (no per-monitor DPI handling anywhere in the backend), and `1.0`
//! is the documented "not yet reported" value.

use std::num::NonZeroIsize;
use std::sync::Arc;

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawWindowHandle,
    Win32WindowHandle, WindowHandle, WindowsDisplayHandle,
};
use runtime_core::primitives::graphics::{GraphicsSurface, OnLost, OnReady, OnResize};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, RegisterClassW, WINDOW_EX_STYLE, WM_ERASEBKGND, WNDCLASSW,
    WS_CHILD, WS_CLIPSIBLINGS, WS_VISIBLE,
};

// =========================================================================
// Per-node state
// =========================================================================

/// Everything the backend keeps for one live graphics node, keyed by
/// node id in `WindowsBackend::graphics`.
pub(crate) struct GraphicsState {
    /// The provider-wrapped child HWND, cloned into each `OnReadyEvent`.
    pub surface: GraphicsSurface,
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
// The surface child window
// =========================================================================

fn class_name() -> PCWSTR {
    PCWSTR(windows::core::w!("IdealystSurface").as_ptr())
}

/// Wndproc for the surface class. Everything defaults except erase:
/// the wgpu swapchain owns every pixel, and letting GDI erase between
/// presents flashes the class background through.
unsafe extern "system" fn surface_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_ERASEBKGND {
        return LRESULT(1);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

fn ensure_class() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let wc = WNDCLASSW {
            lpfnWndProc: Some(surface_wnd_proc),
            hInstance: GetModuleHandleW(None).map(|m| m.into()).unwrap_or_default(),
            lpszClassName: class_name(),
            ..Default::default()
        };
        RegisterClassW(&wc);
    });
}

/// Create the child HWND a wgpu swapchain will attach to. Positioned
/// by `layout_pass` like every native control; hidden/destroyed by
/// the same portal / remove machinery.
pub(crate) fn create_surface_hwnd(parent: HWND) -> HWND {
    ensure_class();
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name(),
            PCWSTR::null(),
            WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
            0,
            0,
            0,
            0,
            parent,
            None,
            None,
            None,
        )
    }
    .unwrap_or(HWND(std::ptr::null_mut()))
}

// =========================================================================
// raw_window_handle provider — bridges the HWND to wgpu.
// =========================================================================

struct Win32SurfaceProvider {
    /// Raw HWND as an integer. Not the `HWND` newtype: wgpu requires
    /// the provider `Send + Sync`, and an integer makes the manual
    /// impls below honest — the value itself is freely copyable; the
    /// safety contract is lifetime, not access: the framework's
    /// `remove_subtree` destroys the HWND only after the author's
    /// `on_lost` has dropped the wgpu surface built on it (same
    /// ordering the macOS provider documents for its NSView).
    hwnd: isize,
    hinstance: isize,
}

unsafe impl Send for Win32SurfaceProvider {}
unsafe impl Sync for Win32SurfaceProvider {}

impl HasWindowHandle for Win32SurfaceProvider {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let hwnd = NonZeroIsize::new(self.hwnd).ok_or(HandleError::Unavailable)?;
        let mut handle = Win32WindowHandle::new(hwnd);
        handle.hinstance = NonZeroIsize::new(self.hinstance);
        Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(handle)) })
    }
}

impl HasDisplayHandle for Win32SurfaceProvider {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(unsafe { DisplayHandle::borrow_raw(WindowsDisplayHandle::new().into()) })
    }
}

/// Wrap `hwnd` for the `OnReadyEvent`.
pub(crate) fn make_surface(hwnd: HWND) -> GraphicsSurface {
    let hinstance = unsafe { GetModuleHandleW(None) }
        .map(|m| m.0 as isize)
        .unwrap_or(0);
    GraphicsSurface::new(Arc::new(Win32SurfaceProvider {
        hwnd: hwnd.0 as isize,
        hinstance,
    }))
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
