//! Target-agnostic wgpu host. Pick the right per-platform mount
//! based on the active target; consumers call [`mount`] without
//! `cfg` and get web / iOS / (eventually) Android / macOS routing
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

use runtime_core::primitives::graphics::GraphicsSurface;
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

#[cfg(not(any(
    target_arch = "wasm32",
    all(target_os = "ios", not(target_arch = "wasm32"))
)))]
#[derive(Debug)]
pub enum MountError {
    /// No wgpu host is wired for this target yet. Returned by
    /// [`mount`] on macOS-AppKit, terminal, Android (until
    /// `host-android-mobile` lands), etc. so consumers can show a
    /// fallback (the chassis-around-an-empty-CAMetalLayer state for
    /// the simulator preview) without confusing this with a real
    /// init failure.
    Unsupported,
}

#[cfg(not(any(
    target_arch = "wasm32",
    all(target_os = "ios", not(target_arch = "wasm32"))
)))]
impl std::fmt::Display for MountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "host-wgpu: no wgpu host wired for this target")
    }
}

#[cfg(not(any(
    target_arch = "wasm32",
    all(target_os = "ios", not(target_arch = "wasm32"))
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

#[cfg(not(any(
    target_arch = "wasm32",
    all(target_os = "ios", not(target_arch = "wasm32"))
)))]
pub struct HostHandle {
    _no_construct: (),
}

#[cfg(not(any(
    target_arch = "wasm32",
    all(target_os = "ios", not(target_arch = "wasm32"))
)))]
impl HostHandle {
    /// No-op on unsupported targets. The handle can't be constructed
    /// because [`mount`] returns `Err` before reaching the `Ok` arm,
    /// so this method is unreachable in practice; it exists to keep
    /// the consumer-facing API symmetric across targets.
    pub fn resize(&self, _size: (u32, u32)) {}
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
/// the same shape — surface, physical-pixel size, device profile,
/// painter skin, and a builder for the embedded Element tree — and
/// hands back a `HostHandle` that owns the wgpu objects and the
/// render-loop subscription.
///
/// Authors typically call this from inside their `Graphics`
/// primitive's `on_ready` callback and stash the returned handle so
/// `on_resize` can call [`HostHandle::resize`] and `on_lost` can
/// drop it.
pub async fn mount<F>(
    surface: GraphicsSurface,
    size: (u32, u32),
    profile: DeviceProfile,
    painter: Rc<dyn Painter>,
    build_ui: F,
) -> Result<HostHandle, MountError>
where
    F: FnOnce() -> Element + 'static,
{
    #[cfg(target_arch = "wasm32")]
    {
        host_web::mount(surface, size, profile, painter, build_ui).await
    }
    #[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
    {
        host_ios_mobile::mount(surface, size, profile, painter, build_ui).await
    }
    #[cfg(not(any(
        target_arch = "wasm32",
        all(target_os = "ios", not(target_arch = "wasm32"))
    )))]
    {
        // Bind the args so the function signature stays honest
        // (no "unused parameter" warnings on unsupported targets).
        let _ = (surface, size, profile, painter, build_ui);
        Err(MountError::Unsupported)
    }
}
