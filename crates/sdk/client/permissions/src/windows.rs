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
//! - **Location** (when-in-use and always alike — Windows does not
//!   distinguish background use for desktop apps) — the privacy setting, read
//!   without prompting through `AppCapability::CheckAccess("location")`:
//!   allowed → `Granted`, off by the user → `Denied`, off by the system or
//!   policy → `Restricted`, not decided yet → `Undetermined`. [`request`]
//!   asks through `Geolocator::RequestAccessAsync`, which shows Windows'
//!   "Let Windows and apps access your location?" prompt when undecided.
//! - **Camera / microphone** — the privacy settings, read the same way
//!   (`AppCapability` `"webcam"` / `"microphone"`); [`request`] asks with
//!   `AppCapability::RequestAccessAsync` when undecided.

use windows::core::{HRESULT, HSTRING};
use windows::Devices::Geolocation::{GeolocationAccessStatus, Geolocator};
use windows::Security::Authorization::AppCapabilityAccess::{AppCapability, AppCapabilityAccessStatus};
use windows::UI::Notifications::{NotificationSetting, ToastNotificationManager};

use super::{Permission, PermissionStatus};

pub(super) async fn status(permission: Permission) -> PermissionStatus {
    match permission {
        Permission::Notifications => notifications(),
        Permission::LocationWhenInUse | Permission::LocationAlways => capability("location"),
        Permission::Camera => capability("webcam"),
        Permission::Microphone => capability("microphone"),
    }
}

pub(super) async fn request(permission: Permission) -> PermissionStatus {
    match permission {
        Permission::LocationWhenInUse | Permission::LocationAlways => {
            if status(permission).await != PermissionStatus::Undetermined {
                return status(permission).await;
            }
            match Geolocator::RequestAccessAsync() {
                Ok(op) => match op.await {
                    Ok(GeolocationAccessStatus::Allowed) => PermissionStatus::Granted,
                    Ok(GeolocationAccessStatus::Denied) => PermissionStatus::Denied,
                    _ => capability("location"),
                },
                Err(_) => capability("location"),
            }
        }
        Permission::Camera => request_capability("webcam").await,
        Permission::Microphone => request_capability("microphone").await,
        Permission::Notifications => status(permission).await,
    }
}

/// `capability(name)`, asking first when undecided.
async fn request_capability(name: &str) -> PermissionStatus {
    if capability(name) != PermissionStatus::Undetermined {
        return capability(name);
    }
    let asked = match AppCapability::Create(&HSTRING::from(name)).and_then(|c| c.RequestAccessAsync()) {
        Ok(op) => op.await,
        Err(e) => Err(e),
    };
    match asked {
        Ok(access) => from_access(access),
        Err(_) => capability(name),
    }
}

/// A privacy-settings capability's state, without prompting.
fn capability(name: &str) -> PermissionStatus {
    match AppCapability::Create(&HSTRING::from(name)).and_then(|c| c.CheckAccess()) {
        Ok(access) => from_access(access),
        Err(_) => PermissionStatus::Unsupported,
    }
}

fn from_access(access: AppCapabilityAccessStatus) -> PermissionStatus {
    match access {
        AppCapabilityAccessStatus::Allowed => PermissionStatus::Granted,
        AppCapabilityAccessStatus::DeniedByUser => PermissionStatus::Denied,
        AppCapabilityAccessStatus::DeniedBySystem => PermissionStatus::Restricted,
        AppCapabilityAccessStatus::UserPromptRequired => PermissionStatus::Undetermined,
        // Packaged apps that did not declare it; an unpackaged app never
        // sees this. Leave the decision to the capturing API.
        _ => PermissionStatus::Unsupported,
    }
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
        let _ = pollster::block_on(status(Permission::Camera));
    }

    #[test]
    fn capability_access_maps_to_statuses() {
        assert_eq!(from_access(AppCapabilityAccessStatus::Allowed), PermissionStatus::Granted);
        assert_eq!(from_access(AppCapabilityAccessStatus::DeniedByUser), PermissionStatus::Denied);
        assert_eq!(from_access(AppCapabilityAccessStatus::DeniedBySystem), PermissionStatus::Restricted);
        assert_eq!(from_access(AppCapabilityAccessStatus::UserPromptRequired), PermissionStatus::Undetermined);
    }

    /// Location read `Unsupported` on Windows, though Windows 11 has a real
    /// location privacy setting and prompt. The status is now the setting's
    /// (read without prompting) — on the test VM, whatever it is, never
    /// `Unsupported`.
    #[test]
    fn regression_windows_location_status_is_the_privacy_setting() {
        let s = pollster::block_on(status(Permission::LocationWhenInUse));
        assert_ne!(s, PermissionStatus::Unsupported);
        assert_eq!(pollster::block_on(status(Permission::LocationAlways)), s);
    }

    /// Camera and microphone read `Unsupported` on Windows; they are the
    /// privacy settings now (never `Unsupported` on Windows 11).
    #[test]
    fn regression_windows_camera_and_microphone_status_are_the_privacy_settings() {
        assert_ne!(pollster::block_on(status(Permission::Camera)), PermissionStatus::Unsupported);
        assert_ne!(pollster::block_on(status(Permission::Microphone)), PermissionStatus::Unsupported);
    }
}
