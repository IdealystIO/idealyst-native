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

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
pub type MountError = host_android_mobile::MountError;

#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
pub type MountError = host_macos_desktop::MountError;

#[cfg(not(any(
    target_arch = "wasm32",
    all(target_os = "ios", not(target_arch = "wasm32")),
    all(target_os = "android", not(target_arch = "wasm32")),
    all(target_os = "macos", not(target_arch = "wasm32"))
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
    all(target_os = "macos", not(target_arch = "wasm32"))
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
    all(target_os = "macos", not(target_arch = "wasm32"))
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

#[cfg(not(any(
    target_arch = "wasm32",
    all(target_os = "ios", not(target_arch = "wasm32")),
    all(target_os = "android", not(target_arch = "wasm32")),
    all(target_os = "macos", not(target_arch = "wasm32"))
)))]
pub struct HostHandle {
    _no_construct: (),
}

#[cfg(not(any(
    target_arch = "wasm32",
    all(target_os = "ios", not(target_arch = "wasm32")),
    all(target_os = "android", not(target_arch = "wasm32")),
    all(target_os = "macos", not(target_arch = "wasm32"))
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
pub async fn mount(
    surface: GraphicsSurface,
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
        host_web::mount(surface, size, profile, painter, build_ui).await
    }
    #[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
    {
        host_ios_mobile::mount(surface, size, profile, painter, build_ui).await
    }
    #[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
    {
        host_android_mobile::mount(surface, size, profile, painter, build_ui).await
    }
    #[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
    {
        host_macos_desktop::mount(surface, size, profile, painter, build_ui).await
    }
    #[cfg(not(any(
        target_arch = "wasm32",
        all(target_os = "ios", not(target_arch = "wasm32")),
        all(target_os = "android", not(target_arch = "wasm32")),
        all(target_os = "macos", not(target_arch = "wasm32"))
    )))]
    {
        // Bind the args so the function signature stays honest
        // (no "unused parameter" warnings on unsupported targets).
        let _ = (surface, size, profile, painter, build_ui);
        Err(MountError::Unsupported)
    }
}

// ---------------------------------------------------------------------------
// mount_newcore — the idea-lite-migration mount seam, routed like
// `mount`. The build closure returns a `runtime_scene::Element` (what a
// new-core graph's `app()` produces), and the tree realizes through the
// per-host new-core boot instead of the old walker.
// ---------------------------------------------------------------------------

/// A new-core scene tree — what `mount_newcore`'s build closure
/// returns. Re-exported so consumers spell one name.
#[cfg(feature = "new-core")]
pub use runtime_scene::Element as SceneElement;

/// Failure modes of [`mount_newcore`]. Separate from [`MountError`]
/// because the per-platform host error enums have no "no new-core port
/// on this target" variant to alias.
#[cfg(feature = "new-core")]
#[derive(Debug)]
pub enum NewCoreMountError {
    /// This target's wgpu host has no new-core mount yet. Only
    /// `host-web` is ported today — the iOS / Android / macOS sim
    /// hosts still mount old-core-authored trees through the old
    /// walker; their ports follow the same `start_in_world` seam and
    /// are tracked in the idea-lite migration log. Consumers fall back
    /// exactly like `MountError::Unsupported` (chassis, no preview).
    Unsupported,
    /// The underlying platform host failed — same error the old-core
    /// `mount` would report.
    Platform(MountError),
}

#[cfg(feature = "new-core")]
impl std::fmt::Display for NewCoreMountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NewCoreMountError::Unsupported => write!(
                f,
                "host-wgpu: no new-core wgpu host wired for this target yet \
                 (web is the ported host; native sim hosts still mount the old core)"
            ),
            NewCoreMountError::Platform(e) => write!(f, "{e}"),
        }
    }
}

#[cfg(feature = "new-core")]
impl std::error::Error for NewCoreMountError {}

/// New-core sibling of [`mount`]: mount a wgpu render backend behind a
/// `Graphics`-primitive surface, realizing `build_ui`'s scene tree
/// into the embedding host's world (on web: the page's own new-core
/// world, so the page's flush driver commits the embedded app's
/// staged writes — see `host_web::mount_newcore`).
#[cfg(feature = "new-core")]
pub async fn mount_newcore(
    surface: GraphicsSurface,
    size: (u32, u32),
    profile: DeviceProfile,
    painter: Rc<dyn Painter>,
    build_ui: Rc<dyn Fn() -> SceneElement + 'static>,
) -> Result<HostHandle, NewCoreMountError> {
    #[cfg(target_arch = "wasm32")]
    {
        host_web::mount_newcore(surface, size, profile, painter, build_ui)
            .await
            .map_err(NewCoreMountError::Platform)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (surface, size, profile, painter, build_ui);
        Err(NewCoreMountError::Unsupported)
    }
}
