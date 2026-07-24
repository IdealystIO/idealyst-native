//! Windows native shell for the wgpu render backend — embed a live
//! idealyst preview inside another idealyst app.
//!
//! The framework's `Element::Graphics` primitive on Windows
//! (`backend-windows`) creates a plain child HWND and packs a
//! `Win32SurfaceProvider` (raw_window_handle `Win32WindowHandle`)
//! into the `GraphicsSurface` it hands to the `on_ready` callback.
//! We take that `GraphicsSurface`, build a wgpu DX12 surface against
//! it, spin up the `render_wgpu::Host` + `Renderer`, mount the
//! caller's UI, and drive per-frame paint via
//! `runtime_core::driver::render_loop` (the raf-backed driver
//! `host-win32` installs at boot).
//!
//! The returned [`WindowsHostHandle`] owns the wgpu objects and the
//! render-loop subscription; drop it (or pass it through the
//! `Graphics` primitive's `on_lost` callback) to tear everything
//! down. On `on_resize`, call [`WindowsHostHandle::resize`] with the
//! new physical-pixel size so the wgpu surface reconfigures.
//!
//! See `host-macos-desktop` for the AppKit sibling — same shape,
//! NSView/Metal instead of HWND/DX12.

#![allow(clippy::new_without_default)]

#[cfg(target_os = "windows")]
mod windows_impl;

#[cfg(target_os = "windows")]
pub use windows_impl::{mount, MountError, WindowsHostHandle};

#[cfg(target_os = "windows")]
pub use render_api::DeviceProfile;

// Re-export `render_wgpu::Painter` so consumers (Simulator
// components, future preview embeds) don't need a direct
// `render-wgpu` dep just to name the painter type. Mirrors the
// `host-web` / `host-macos-desktop` re-exports.
#[cfg(target_os = "windows")]
pub use render_wgpu::Painter;

// Non-Windows targets: empty crate. Lets consumers list
// `host-windows-desktop` as an unconditional dep without a
// `cfg(target_os = "windows")` gate at each call site — the actual
// mount path is only reachable when the `Graphics` primitive is
// wired to the Windows backend.
