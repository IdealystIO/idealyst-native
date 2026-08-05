//! macOS native shell for the wgpu render backend — embed a live
//! idealyst preview inside another idealyst app.
//!
//! The framework's `Element::Graphics` primitive on macOS
//! (`backend-macos`) creates an `NSView` whose backing layer is a
//! `CAMetalLayer` and packs a `MacosSurfaceProvider` (raw_window_handle
//! `AppKitWindowHandle`) into the `GraphicsSurface` it hands to the
//! `on_ready` callback. We take that `GraphicsSurface`, build a wgpu
//! Metal surface against it, spin up the `render_wgpu::Host` +
//! `Renderer`, mount the caller's UI, and drive per-frame paint via
//! `runtime_shared::driver::render_loop` (the NSTimer driver installed
//! by `host-appkit` at boot).
//!
//! The returned [`MacosHostHandle`] owns the wgpu objects and the
//! render-loop subscription; drop it (or pass it through the
//! `Graphics` primitive's `on_lost` callback) to tear everything
//! down. On `on_resize`, call [`MacosHostHandle::resize`] with the
//! new physical-pixel size so the wgpu surface reconfigures.
//!
//! See [`host_ios_mobile`] for the iOS sibling — same shape, UiKit
//! handle + NSTimer installed by the app's `register_extensions`
//! instead of the host shell.

#![allow(clippy::new_without_default)]

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::{MacosHostHandle, MountError};

#[cfg(target_os = "macos")]
pub use macos::mount;

/// Compatibility alias. This entry was `mount_newcore` while the
/// framework carried two cores; there is one mount now and it is
/// [`mount`]. Kept so existing call sites keep resolving.
#[cfg(target_os = "macos")]
pub use macos::mount as mount_newcore;

#[cfg(target_os = "macos")]
pub use render_api::DeviceProfile;

// Re-export `render_wgpu::Painter` so consumers (Simulator
// components, future preview embeds) don't need a direct
// `render-wgpu` dep just to name the painter type. Mirrors the
// `host-web` / `host-ios-mobile` re-exports.
#[cfg(target_os = "macos")]
pub use render_wgpu::Painter;

// Non-macOS targets: empty crate. Lets consumers list
// `host-macos-desktop` as an unconditional dep without a
// `cfg(target_os = "macos")` gate at each call site — the actual
// mount path is only reachable when the `Graphics` primitive is
// wired to the macOS backend.
