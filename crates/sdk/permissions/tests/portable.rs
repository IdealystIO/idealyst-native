//! Host-portable tests for the permissions SDK.
//!
//! Host-portable tests for the permissions SDK.
//!
//! The status-helper test is pure and runs everywhere. The tests that call
//! `status` / `request` are gated to the **fallback** target (Linux /
//! Windows / other non-Apple, non-Android native), where there is no native
//! permission model and every permission resolves to `Unsupported`. They're
//! excluded on macOS, whose real `UNUserNotificationCenter` /
//! `CLLocationManager` backend throws when messaged from an un-bundled test
//! binary — that path is exercised on device, not in `cargo test`.

use permissions::PermissionStatus;

#[test]
fn status_helper_semantics() {
    assert!(PermissionStatus::Granted.is_granted());
    assert!(!PermissionStatus::Denied.is_granted());
    assert!(!PermissionStatus::Undetermined.is_granted());

    // `is_usable` accepts Granted OR Unsupported (a platform that needs no
    // grant), and nothing else.
    assert!(PermissionStatus::Granted.is_usable());
    assert!(PermissionStatus::Unsupported.is_usable());
    assert!(!PermissionStatus::Denied.is_usable());
    assert!(!PermissionStatus::Restricted.is_usable());
    assert!(!PermissionStatus::Undetermined.is_usable());
}

// The remaining tests drive the real `imp`; only the fallback target reports
// the deterministic `Unsupported` they assert.
#[cfg(all(
    not(target_arch = "wasm32"),
    not(any(target_os = "ios", target_os = "macos", target_os = "tvos")),
    not(target_os = "android")
))]
mod fallback {
    use permissions::{request, status, Permission, PermissionStatus};

    #[tokio::test]
    async fn host_status_is_unsupported_for_every_permission() {
        for p in [
            Permission::Notifications,
            Permission::LocationWhenInUse,
            Permission::LocationAlways,
            Permission::Camera,
            Permission::Microphone,
        ] {
            // No native permission backend → resolves immediately to
            // Unsupported and never hangs.
            assert_eq!(status(p).await, PermissionStatus::Unsupported);
        }
    }

    #[tokio::test]
    async fn host_request_never_blocks_and_reports_unsupported() {
        // `request` must resolve even where there's no OS prompt to show — a
        // sibling SDK awaiting it must never deadlock.
        for p in [
            Permission::Notifications,
            Permission::LocationWhenInUse,
            Permission::LocationAlways,
        ] {
            assert_eq!(request(p).await, PermissionStatus::Unsupported);
        }
    }

    #[tokio::test]
    async fn unsupported_is_treated_as_usable() {
        // The contract sibling SDKs lean on: where a permission isn't a
        // concept on this target, the capability is still usable.
        assert!(status(Permission::Notifications).await.is_usable());
    }
}
