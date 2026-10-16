//! Windows permission state.
//!
//! - **Notifications** — the app's notification setting
//!   (`ToastNotifier::Setting`, for the app's AppUserModelID from
//!   `backend_windows::app_identity`). Windows never prompts for it: the
//!   user turns an app's notifications on or off in Settings, so
//!   [`request`] reports the same as [`status`]. Enabled → `Granted`;
//!   turned off by the user (for this app or for all apps) → `Denied`;
//!   turned off by group policy or the app manifest → `Restricted`.
//!   An unpackaged app Windows has not listed yet (it is added to the
//!   notification settings on its first toast) makes `Setting` fail with
//!   `ERROR_NOT_FOUND`; such an app's notifications are on — the default —
//!   so that reads as `Granted`.
//! - **Location / camera / microphone** — `Unsupported` (usable): the
//!   desktop privacy switches for these are read by the capturing SDKs when
//!   they open the device, which reports a denial as its own error.

use windows::core::{HRESULT, HSTRING};
use windows::UI::Notifications::{NotificationSetting, ToastNotificationManager};

use super::{Permission, PermissionStatus};

pub(super) async fn status(permission: Permission) -> PermissionStatus {
    match permission {
        Permission::Notifications => notifications(),
        _ => PermissionStatus::Unsupported,
    }
}

pub(super) async fn request(permission: Permission) -> PermissionStatus {
    status(permission).await
}

fn notifications() -> PermissionStatus {
    let id = HSTRING::from(backend_windows::app_identity::current());
    match ToastNotificationManager::CreateToastNotifierWithId(&id).and_then(|n| n.Setting()) {
        Ok(setting) => from_setting(setting),
        Err(e) if e.code() == NOT_LISTED_YET => PermissionStatus::Granted,
        // No notifier for this id: nothing will be shown.
        Err(_) => PermissionStatus::Denied,
    }
}

/// `HRESULT_FROM_WIN32(ERROR_NOT_FOUND)` — `Setting` for an app not yet in
/// the notification settings (see the module docs).
const NOT_LISTED_YET: HRESULT = HRESULT(0x8007_0490_u32 as i32);

fn from_setting(setting: NotificationSetting) -> PermissionStatus {
    match setting {
        NotificationSetting::Enabled => PermissionStatus::Granted,
        NotificationSetting::DisabledForApplication | NotificationSetting::DisabledForUser => {
            PermissionStatus::Denied
        }
        _ => PermissionStatus::Restricted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_settings_map_to_statuses() {
        assert_eq!(from_setting(NotificationSetting::Enabled), PermissionStatus::Granted);
        assert_eq!(from_setting(NotificationSetting::DisabledForUser), PermissionStatus::Denied);
        assert_eq!(from_setting(NotificationSetting::DisabledForApplication), PermissionStatus::Denied);
        assert_eq!(from_setting(NotificationSetting::DisabledByGroupPolicy), PermissionStatus::Restricted);
        assert_eq!(from_setting(NotificationSetting::DisabledByManifest), PermissionStatus::Restricted);
    }

    /// Windows reported every permission as `Unsupported`, so a disabled
    /// notification setting still read as usable and posts vanished. The
    /// real setting is read now. A fresh, never-notified app id is one
    /// Windows has not listed yet: its notifications are on (`Granted`) —
    /// reading that as `Denied` blocked every first post.
    #[test]
    fn regression_windows_notification_status_is_the_real_setting() {
        let fresh = format!("idealyst.test.permissions-{}", std::process::id());
        backend_windows::app_identity::set(&fresh, "Permissions test").unwrap();
        let s = pollster::block_on(status(Permission::Notifications));
        assert_eq!(s, PermissionStatus::Granted);
        assert_eq!(pollster::block_on(request(Permission::Notifications)), s);
        assert_eq!(pollster::block_on(status(Permission::Camera)), PermissionStatus::Unsupported);
    }
}
