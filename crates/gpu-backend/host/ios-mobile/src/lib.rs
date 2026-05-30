//! iOS native shell for the wgpu render backend — embed a live
//! idealyst preview inside another idealyst app.
//!
//! The framework's `Element::Graphics` primitive on iOS
//! (`backend-ios-mobile`) creates a `UIView` whose backing layer is
//! a `CAMetalLayer` and packs an `IosSurfaceProvider` (raw_window_handle
//! `UiKitWindowHandle`) into the `GraphicsSurface` it hands to the
//! `on_ready` callback. We take that `GraphicsSurface`, build a wgpu
//! Metal surface against it, spin up the `render_wgpu::Host` +
//! `Renderer`, mount the caller's UI, and drive per-frame paint via
//! `runtime_core::driver::render_loop` (installed by `backend-ios-core`
//! as an NSTimer).
//!
//! The returned [`IosHostHandle`] owns the wgpu objects and the
//! render-loop subscription; drop it (or pass it through the
//! `Graphics` primitive's `on_lost` callback) to tear everything
//! down. On `on_resize`, call [`IosHostHandle::resize`] with the new
//! physical-pixel size so the wgpu surface reconfigures.
//!
//! See [`host_web`] for the web sibling — same shape, different
//! backend feature flags + no pointer-event plumbing.

#![allow(clippy::new_without_default)]

#[cfg(target_os = "ios")]
mod ios;

#[cfg(target_os = "ios")]
pub use ios::{mount, IosHostHandle, MountError};

#[cfg(target_os = "ios")]
pub use render_api::DeviceProfile;

// Re-export `render_wgpu::Painter` so consumers (Simulator
// components, future preview embeds) don't need a direct
// `render-wgpu` dep just to name the painter type. Mirrors
// `host-web`'s re-export and keeps the `Simulator(...)` call sites
// symmetric across web / iOS.
#[cfg(target_os = "ios")]
pub use render_wgpu::Painter;

// Non-iOS targets: empty crate. Lets the website / docs / other
// consumers list `host-ios-mobile` as an unconditional dep without a
// `cfg(target_os = "ios")` gate at each call site — the
// `Graphics::on_ready` closure stays cross-target-clean because the
// actual mount path is only reachable when the `Graphics` primitive
// is wired to an iOS backend.
