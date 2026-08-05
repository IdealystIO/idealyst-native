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
//! - [`MountError`] — failure modes: no wgpu host on this target, or
//!   a [`PlatformMountError`] from the underlying platform host.
//!
//! See [`render_wgpu`] for the rendering engine and per-platform
//! crates (`host_web`, `host_ios_mobile`) for the actual wgpu init.

#![allow(clippy::new_without_default)]

use std::rc::Rc;

pub use render_api::DeviceProfile;
pub use render_wgpu::Painter;

use runtime_shared::primitives::graphics::GraphicsSurface;

// ---------------------------------------------------------------------------
// Re-export the platform host's error per platform — each underlying
// host crate has its own enum and its own `Display`/`Error` impls.
// Aliasing rather than inventing a new enum keeps the error messages
// honest (the consumer sees the same string the underlying crate
// reports) and avoids From shims at every call site.
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
pub type PlatformMountError = host_web::MountError;

#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub type PlatformMountError = host_ios_mobile::MountError;

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
pub type PlatformMountError = host_android_mobile::MountError;

#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
pub type PlatformMountError = host_macos_desktop::MountError;

#[cfg(not(any(
    target_arch = "wasm32",
    all(target_os = "ios", not(target_arch = "wasm32")),
    all(target_os = "android", not(target_arch = "wasm32")),
    all(target_os = "macos", not(target_arch = "wasm32"))
)))]
#[derive(Debug)]
pub enum PlatformMountError {
    /// No wgpu host is wired for this target at all (terminal,
    /// headless, Windows/Linux desktop). Unreachable in practice —
    /// [`mount`] returns [`MountError::Unsupported`] before it could
    /// construct one — but the type must exist so the signatures stay
    /// uniform across targets.
    Unsupported,
}

#[cfg(not(any(
    target_arch = "wasm32",
    all(target_os = "ios", not(target_arch = "wasm32")),
    all(target_os = "android", not(target_arch = "wasm32")),
    all(target_os = "macos", not(target_arch = "wasm32"))
)))]
impl std::fmt::Display for PlatformMountError {
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
impl std::error::Error for PlatformMountError {}

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
// mount — the one entry point, routed per target. The build closure
// returns a `runtime_scene::Element` (what an app's `app()` produces)
// and the tree realizes through the per-host boot.
// ---------------------------------------------------------------------------

/// A scene tree — what [`mount`]'s build closure returns. Re-exported
/// so consumers spell one name.
pub use runtime_scene::Element as SceneElement;

/// Failure modes of [`mount`]. Distinct from [`PlatformMountError`]
/// because the per-platform host error enums have no "no wgpu host on
/// this target" variant to alias.
#[derive(Debug)]
pub enum MountError {
    /// This target has no wgpu host. Web and the native sim hosts
    /// (macOS / iOS / Android) are wired — each realizes the embedded
    /// tree into its page/app backend's mounted world through the
    /// shared `start_in_world` seam. The remaining targets (terminal,
    /// headless, Windows/Linux desktop) have no wgpu host at all, so
    /// consumers fall back to the chassis-around-an-empty-surface
    /// state without confusing this with a real init failure.
    Unsupported,
    /// The underlying platform host failed.
    Platform(PlatformMountError),
}

/// Compatibility alias. This enum was `NewCoreMountError` while the
/// framework carried two cores; there is one mount now and its error
/// is [`MountError`].
pub use MountError as NewCoreMountError;

impl std::fmt::Display for MountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MountError::Unsupported => write!(
                f,
                "host-wgpu: no wgpu host wired for this target \
                 (web + macOS/iOS/Android sim hosts are wired; terminal, \
                 headless, and Windows/Linux desktop have no wgpu host)"
            ),
            MountError::Platform(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for MountError {}

/// Mount a wgpu render backend behind a `Graphics`-primitive surface,
/// realizing `build_ui`'s scene tree into the embedding host's world —
/// on web the page's own world (`host_web::mount`), on macOS / iOS /
/// Android the app's own world (`host_macos_desktop::mount` and
/// siblings) — so the embedding host's flush driver commits the
/// embedded app's staged writes: one thread, one world, one logical
/// update stream.
pub async fn mount(
    surface: GraphicsSurface,
    size: (u32, u32),
    profile: DeviceProfile,
    painter: Rc<dyn Painter>,
    build_ui: Rc<dyn Fn() -> SceneElement + 'static>,
) -> Result<HostHandle, MountError> {
    #[cfg(target_arch = "wasm32")]
    {
        host_web::mount(surface, size, profile, painter, build_ui)
            .await
            .map_err(MountError::Platform)
    }
    #[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
    {
        host_ios_mobile::mount(surface, size, profile, painter, build_ui)
            .await
            .map_err(MountError::Platform)
    }
    #[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
    {
        host_android_mobile::mount(surface, size, profile, painter, build_ui)
            .await
            .map_err(MountError::Platform)
    }
    #[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
    {
        host_macos_desktop::mount(surface, size, profile, painter, build_ui)
            .await
            .map_err(MountError::Platform)
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

/// Compatibility alias. This entry was `mount_newcore` while the
/// framework carried two cores; there is one mount now and it is
/// [`mount`]. Kept so existing call sites (the website Simulator)
/// keep resolving.
pub use self::mount as mount_newcore;
