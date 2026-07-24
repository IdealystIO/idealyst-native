//! Target-agnostic wgpu host. Pick the right per-platform mount
//! based on the active target; consumers call [`mount`] without
//! `cfg` and get web / iOS / Android / macOS routing
//! transparently.
//!
//! Re-exports:
//! - [`DeviceProfile`] — logical viewport + color scheme + window
//!   title, defined in `render-api`.
//! - [`Painter`] — the platform-skin trait from `render-wgpu`. iOS
//!   sim, Android sim, and any future SDK-supplied skins implement it.
//! - [`HostHandle`] — the live preview handle. Drop it to tear down
//!   the host; call [`HostHandle::resize`] when the surface size
//!   changes.
//! - [`MountError`] — failure modes from the underlying platform host.
//!
//! See [`render_wgpu`] for the rendering engine and per-platform
//! crates (`host_web`, `host_ios_mobile`) for the actual wgpu init.

#![allow(clippy::new_without_default)]

use std::rc::Rc;

pub use render_api::DeviceProfile;
pub use render_wgpu::Painter;

use runtime_core::primitives::graphics::GraphicsTarget;
use runtime_core::Element;

// ---------------------------------------------------------------------------
// Re-export `MountError` per platform — each underlying host crate
// has its own enum and its own `Display`/`Error` impls. Aliasing
// rather than inventing a new enum keeps the error messages honest
// (the consumer sees the same string the underlying crate reports)
// and avoids From shims at every call site.
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
pub type MountError = host_web::MountError;

#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub type MountError = host_ios_mobile::MountError;

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
pub type MountError = host_android_mobile::MountError;

#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
pub type MountError = host_macos_desktop::MountError;

#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
pub type MountError = host_windows_desktop::MountError;

#[cfg(all(target_os = "linux", not(target_arch = "wasm32")))]
pub type MountError = host_linux_desktop::MountError;

#[cfg(not(any(
    target_arch = "wasm32",
    all(target_os = "ios", not(target_arch = "wasm32")),
    all(target_os = "android", not(target_arch = "wasm32")),
    all(target_os = "macos", not(target_arch = "wasm32")),
    all(target_os = "windows", not(target_arch = "wasm32")),
    all(target_os = "linux", not(target_arch = "wasm32"))
)))]
#[derive(Debug)]
pub enum MountError {
    /// No wgpu host is wired for this target yet. Returned by
    /// [`mount`] on terminal, headless, etc. so consumers can
    /// show a fallback (the chassis-around-an-empty-surface state
    /// for the simulator preview) without confusing this with a
    /// real init failure.
    Unsupported,
}

#[cfg(not(any(
    target_arch = "wasm32",
    all(target_os = "ios", not(target_arch = "wasm32")),
    all(target_os = "android", not(target_arch = "wasm32")),
    all(target_os = "macos", not(target_arch = "wasm32")),
    all(target_os = "windows", not(target_arch = "wasm32")),
    all(target_os = "linux", not(target_arch = "wasm32"))
)))]
impl std::fmt::Display for MountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "host-wgpu: no wgpu host wired for this target")
    }
}

#[cfg(not(any(
    target_arch = "wasm32",
    all(target_os = "ios", not(target_arch = "wasm32")),
    all(target_os = "android", not(target_arch = "wasm32")),
    all(target_os = "macos", not(target_arch = "wasm32")),
    all(target_os = "windows", not(target_arch = "wasm32")),
    all(target_os = "linux", not(target_arch = "wasm32"))
)))]
impl std::error::Error for MountError {}

// ---------------------------------------------------------------------------
// HostHandle — type-aliased per platform. Both the web and iOS handles
// expose the same `resize(size)` method so consumers can call it
// uniformly.
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
pub type HostHandle = host_web::WebHostHandle;

#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub type HostHandle = host_ios_mobile::IosHostHandle;

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
pub type HostHandle = host_android_mobile::AndroidHostHandle;

#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
pub type HostHandle = host_macos_desktop::MacosHostHandle;

#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
pub type HostHandle = host_windows_desktop::WindowsHostHandle;

#[cfg(all(target_os = "linux", not(target_arch = "wasm32")))]
pub type HostHandle = host_linux_desktop::LinuxHostHandle;

#[cfg(not(any(
    target_arch = "wasm32",
    all(target_os = "ios", not(target_arch = "wasm32")),
    all(target_os = "android", not(target_arch = "wasm32")),
    all(target_os = "macos", not(target_arch = "wasm32")),
    all(target_os = "windows", not(target_arch = "wasm32")),
    all(target_os = "linux", not(target_arch = "wasm32"))
)))]
pub struct HostHandle {
    _no_construct: (),
}

#[cfg(not(any(
    target_arch = "wasm32",
    all(target_os = "ios", not(target_arch = "wasm32")),
    all(target_os = "android", not(target_arch = "wasm32")),
    all(target_os = "macos", not(target_arch = "wasm32")),
    all(target_os = "windows", not(target_arch = "wasm32")),
    all(target_os = "linux", not(target_arch = "wasm32"))
)))]
impl HostHandle {
    /// No-op on unsupported targets. The handle can't be constructed
    /// because [`mount`] returns `Err` before reaching the `Ok` arm,
    /// so these methods are unreachable in practice; they exist to
    /// keep the consumer-facing API symmetric across targets.
    pub fn resize(&self, _size: (u32, u32)) {}
    pub fn pause(&self) {}
    pub fn resume(&self) {}
    pub fn is_running(&self) -> bool { false }
}


// ---------------------------------------------------------------------------
// Target/host mismatch
// ---------------------------------------------------------------------------

/// The error a platform host reports when [`mount`] is handed a
/// `GraphicsTarget` shape it cannot drive — a GL context to a swapchain
/// host, or a window handle to the GL host.
///
/// In practice unreachable: each backend produces exactly one target
/// shape and each platform's host consumes that shape. It exists so the
/// routing in [`mount`] can be a total function over the enum instead of
/// unwrapping, and it reuses each host's own "wrong handle" variant so
/// the message stays in that crate's voice.
#[cfg(target_arch = "wasm32")]
fn unsupported_target() -> MountError {
    host_web::MountError::NoCanvas
}

#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
fn unsupported_target() -> MountError {
    host_ios_mobile::MountError::NoUiKitHandle
}

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
fn unsupported_target() -> MountError {
    host_android_mobile::MountError::CreateSurface
}

#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
fn unsupported_target() -> MountError {
    host_macos_desktop::MountError::NoAppKitHandle
}

#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
fn unsupported_target() -> MountError {
    host_windows_desktop::MountError::NoWin32Handle
}

#[cfg(all(target_os = "linux", not(target_arch = "wasm32")))]
fn unsupported_target() -> MountError {
    host_linux_desktop::MountError::AdoptContext
}

// ---------------------------------------------------------------------------
// mount — one entry point. Routes to the per-platform host's `mount`
// and returns its `HostHandle` (aliased as `HostHandle`). On
// unsupported targets returns `Err(MountError::Unsupported)`
// immediately so the call site can fall back to a static preview.
// ---------------------------------------------------------------------------

/// Mount a wgpu render backend behind a `Graphics`-primitive surface.
///
/// Each per-platform host (`host-web`, `host-ios-mobile`, …) takes
/// the same shape — a render target, physical-pixel size, device
/// profile, painter skin, and a builder for the embedded Element tree
/// — and hands back a `HostHandle` that owns the wgpu objects and the
/// render-loop subscription.
///
/// The target arrives as a `GraphicsTarget` rather than a
/// `GraphicsSurface` because not every backend has a window handle to
/// give: GTK4 lends a GL context instead (see `GraphicsTarget::Gl`).
/// Each arm below takes the shape its platform actually produces, and
/// a target that doesn't match the platform's host yields
/// `MountError` rather than being coerced.
///
/// Authors typically call this from inside their `Graphics`
/// primitive's `on_ready` callback and stash the returned handle so
/// `on_resize` can call [`HostHandle::resize`] and `on_lost` can
/// drop it.
pub async fn mount(
    target: GraphicsTarget,
    size: (u32, u32),
    profile: DeviceProfile,
    painter: Rc<dyn Painter>,
    // `Rc<dyn Fn>` instead of `FnOnce` so per-host visibility gates
    // can unmount/remount the embedded reactive scope without losing
    // the build closure. Hosts that don't need this (web today) just
    // call it once.
    build_ui: Rc<dyn Fn() -> Element + 'static>,
) -> Result<HostHandle, MountError> {
    #[cfg(target_arch = "wasm32")]
    {
        let GraphicsTarget::RawWindow(surface) = target else {
            return Err(unsupported_target());
        };
        host_web::mount(surface, size, profile, painter, build_ui).await
    }
    #[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
    {
        let GraphicsTarget::RawWindow(surface) = target else {
            return Err(unsupported_target());
        };
        host_ios_mobile::mount(surface, size, profile, painter, build_ui).await
    }
    #[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
    {
        let GraphicsTarget::RawWindow(surface) = target else {
            return Err(unsupported_target());
        };
        host_android_mobile::mount(surface, size, profile, painter, build_ui).await
    }
    #[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
    {
        let GraphicsTarget::RawWindow(surface) = target else {
            return Err(unsupported_target());
        };
        host_macos_desktop::mount(surface, size, profile, painter, build_ui).await
    }
    #[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
    {
        let GraphicsTarget::RawWindow(surface) = target else {
            return Err(unsupported_target());
        };
        host_windows_desktop::mount(surface, size, profile, painter, build_ui).await
    }
    #[cfg(all(target_os = "linux", not(target_arch = "wasm32")))]
    {
        // The GTK backend lends a GL context; there is no swapchain
        // surface to hand a swapchain host.
        let GraphicsTarget::Gl(gl) = target else {
            return Err(unsupported_target());
        };
        host_linux_desktop::mount(gl, size, profile, painter, build_ui).await
    }
    #[cfg(not(any(
        target_arch = "wasm32",
        all(target_os = "ios", not(target_arch = "wasm32")),
        all(target_os = "android", not(target_arch = "wasm32")),
        all(target_os = "macos", not(target_arch = "wasm32")),
        all(target_os = "windows", not(target_arch = "wasm32")),
        all(target_os = "linux", not(target_arch = "wasm32"))
    )))]
    {
        // Bind the args so the function signature stays honest
        // (no "unused parameter" warnings on unsupported targets).
        let _ = (target, size, profile, painter, build_ui);
        Err(MountError::Unsupported)
    }
}
