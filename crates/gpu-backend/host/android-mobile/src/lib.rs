//! Android native shell for the wgpu render backend — embed a live
//! idealyst preview inside another idealyst app.
//!
//! The framework's `Element::Graphics` primitive on Android
//! (`backend-android-mobile`) creates a `SurfaceView`, hooks
//! `SurfaceHolder.Callback`, and converts the `Surface` to an
//! `ANativeWindow*` which it packs into a `GraphicsSurface`
//! (`AndroidNdkWindowHandle`). We take that `GraphicsSurface`, build
//! a wgpu Vulkan (or GLES, where Vulkan isn't available) surface
//! against it, spin up the `render_wgpu::Host` + `Renderer`, mount
//! the caller's UI, and drive per-frame paint via
//! `runtime_core::driver::render_loop` — already installed by
//! `backend-android-mobile`'s Choreographer-driven raf loop.
//!
//! The returned [`AndroidHostHandle`] owns the wgpu objects and the
//! render-loop subscription; drop it (or pass through the `Graphics`
//! primitive's `on_lost`) to tear everything down. On `on_resize`,
//! call [`AndroidHostHandle::resize`] with the new physical-pixel
//! size so the wgpu surface reconfigures.
//!
//! See [`host_ios_mobile`] for the iOS sibling — same shape, Metal
//! instead of Vulkan, plus a visibility-gate that checks the
//! UIView's window/hidden/alpha chain. The Android equivalent (walk
//! the View tree looking for `getVisibility() != VISIBLE` /
//! `getWindowToken() == null`) is intentionally deferred until we
//! have a real navigator-hidden-preview use case to validate
//! against — the current `Graphics` lifecycle (on_lost on detach)
//! covers the common dispose-on-blur path.

#![allow(clippy::new_without_default)]

#[cfg(target_os = "android")]
mod android;

#[cfg(target_os = "android")]
pub use android::{mount, AndroidHostHandle, MountError};
