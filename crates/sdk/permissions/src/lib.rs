//! Cross-platform **runtime permission** requests — the shared substrate
//! capability SDKs (`notifications`, `location`, …) build on instead of
//! each re-implementing an OS grant flow.
//!
//! A permission is two facts: the *requirement* (the manifest entry /
//! plist key the build injects — see each capability SDK's
//! `[package.metadata.idealyst] capabilities`) and the *runtime grant*
//! (the user tapping "Allow" in the OS prompt). This crate owns the second
//! half: a tiny, unopinionated surface to read and request a permission's
//! current grant state, uniform across every backend.
//!
//! ```ignore
//! use permissions::{request, Permission, PermissionStatus};
//!
//! # async fn demo() {
//! if request(Permission::Notifications).await == PermissionStatus::Granted {
//!     // … schedule a local notification …
//! }
//! # }
//! ```
//!
//! The public surface here — [`PermissionStatus`], [`Permission`],
//! [`status`], [`request`] — is the contract sibling SDKs depend on. The
//! per-platform grant mechanism (UNUserNotificationCenter / CLLocationManager
//! on Apple, the Android runtime-permission dialog, the web Permissions &
//! Notification APIs) lives behind the `imp` module, chosen per target.

#![deny(missing_docs)]

pub mod recipes;

// ---------------------------------------------------------------------------
// Public types (FROZEN — sibling SDKs key against these names).
// ---------------------------------------------------------------------------

/// The grant state of an OS permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionStatus {
    /// The user granted the permission; the capability may be used.
    Granted,
    /// The user explicitly denied the permission. Re-requesting will not
    /// re-prompt on most platforms — the app must send the user to OS
    /// settings.
    Denied,
    /// Blocked by policy (parental controls / MDM), not by the user.
    Restricted,
    /// Never requested yet — a [`request`] will show the OS prompt.
    Undetermined,
    /// This platform has no concept of this permission (e.g. a permission
    /// that needs no grant on the current target). Treat as usable.
    Unsupported,
}

impl PermissionStatus {
    /// True only for [`PermissionStatus::Granted`].
    pub fn is_granted(self) -> bool {
        matches!(self, PermissionStatus::Granted)
    }

    /// True when the capability may be used — `Granted` *or* `Unsupported`
    /// (a platform that needs no grant). The common gate before invoking
    /// the capability.
    pub fn is_usable(self) -> bool {
        matches!(self, PermissionStatus::Granted | PermissionStatus::Unsupported)
    }
}

/// A known OS permission.
///
/// `#[non_exhaustive]` so new permissions can be added without a breaking
/// change. The first three variants are the contract sibling SDKs key
/// against — **their names are frozen**; an implementation must not rename
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Permission {
    /// Post local / push notifications (Android 13+ `POST_NOTIFICATIONS`;
    /// iOS `UNUserNotificationCenter` authorization; web `Notification`).
    Notifications,
    /// Access device location while the app is in use (iOS
    /// "When In Use"; Android `ACCESS_FINE_LOCATION`; web geolocation).
    LocationWhenInUse,
    /// Access device location in the background as well as the foreground
    /// (iOS "Always"; Android background-location). A superset of
    /// [`Permission::LocationWhenInUse`].
    LocationAlways,
    /// Capture from the device camera (iOS `NSCameraUsageDescription`;
    /// Android `CAMERA`; web `camera` permission descriptor).
    Camera,
    /// Capture from the device microphone (iOS
    /// `NSMicrophoneUsageDescription`; Android `RECORD_AUDIO`; web
    /// `microphone` permission descriptor).
    Microphone,
}

/// The permission's current grant state, **without** prompting the user.
/// Cheap to call repeatedly.
pub async fn status(permission: Permission) -> PermissionStatus {
    imp::status(permission).await
}

/// Request the permission, showing the OS prompt when the state is
/// [`PermissionStatus::Undetermined`]. Resolves to the resulting state.
///
/// On a platform where the permission needs no grant this resolves to
/// [`PermissionStatus::Unsupported`] without prompting.
pub async fn request(permission: Permission) -> PermissionStatus {
    imp::request(permission).await
}

// ---------------------------------------------------------------------------
// async bridge for callback / delegate-based native APIs.
//
// The Apple permission APIs (`requestAuthorizationWithOptions:completion:`,
// CLLocationManager's delegate) and the Android `onRequestPermissionsResult`
// hook deliver their answer through a callback on some later run-loop turn,
// not as a return value. `Oneshot` is the smallest bridge from that
// callback world to our `async fn`: the producing side calls `send` exactly
// once, the awaiting side `.await`s the receiver. No external async-channel
// dependency, no `mem::forget`, `Send`-safe for the threads JNI/dispatch use.
// ---------------------------------------------------------------------------

// Only the Apple + Android backends bridge a callback to the async fn; the
// web backend awaits JS promises directly and the fallback is synchronous.
#[cfg(all(
    not(target_arch = "wasm32"),
    any(
        target_os = "ios",
        target_os = "macos",
        target_os = "tvos",
        target_os = "android"
    )
))]
mod oneshot;

// ---------------------------------------------------------------------------
// Platform implementation. Exactly one `imp` compiles per target; every
// unsupported target falls back to the stub that reports `Unsupported`.
// The public surface above is frozen — only `imp` bodies differ.
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[path = "web.rs"]
mod imp;

#[cfg(all(
    not(target_arch = "wasm32"),
    any(target_os = "ios", target_os = "macos", target_os = "tvos")
))]
#[path = "apple.rs"]
mod imp;

#[cfg(all(not(target_arch = "wasm32"), target_os = "android"))]
#[path = "android.rs"]
mod imp;

// Fallback for every target with no native permission model (desktop
// Windows / Linux, the host running `cargo test`): nothing to grant, so
// every permission reads as `Unsupported` (which `is_usable()` accepts).
#[cfg(all(
    not(target_arch = "wasm32"),
    not(any(target_os = "ios", target_os = "macos", target_os = "tvos")),
    not(target_os = "android")
))]
mod imp {
    use super::{Permission, PermissionStatus};

    pub(super) async fn status(_permission: Permission) -> PermissionStatus {
        PermissionStatus::Unsupported
    }

    pub(super) async fn request(_permission: Permission) -> PermissionStatus {
        PermissionStatus::Unsupported
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_helpers() {
        assert!(PermissionStatus::Granted.is_granted());
        assert!(!PermissionStatus::Denied.is_granted());
        assert!(PermissionStatus::Granted.is_usable());
        assert!(PermissionStatus::Unsupported.is_usable());
        assert!(!PermissionStatus::Denied.is_usable());
    }

    /// On a desktop host with NO native permission model (Linux / Windows)
    /// every permission — including the added `Camera` / `Microphone`
    /// variants — reads as `Unsupported`, i.e. usable, never blocking the
    /// caller. Gated to the fallback target: macOS *does* have a real
    /// backend (UNUserNotificationCenter / CLLocationManager), which an
    /// un-bundled test binary can't message without throwing.
    #[cfg(all(
        not(target_arch = "wasm32"),
        not(any(target_os = "ios", target_os = "macos", target_os = "tvos")),
        not(target_os = "android")
    ))]
    #[tokio::test]
    async fn host_reports_unsupported() {
        for p in [
            Permission::Notifications,
            Permission::LocationWhenInUse,
            Permission::LocationAlways,
            Permission::Camera,
            Permission::Microphone,
        ] {
            assert_eq!(status(p).await, PermissionStatus::Unsupported);
            assert_eq!(request(p).await, PermissionStatus::Unsupported);
        }
    }
}
