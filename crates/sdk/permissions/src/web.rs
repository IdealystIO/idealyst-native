//! Web permission backend.
//!
//! Two distinct browser APIs cover the permissions this crate models:
//!
//! - **Notifications** have an explicit, prompting request:
//!   `Notification.permission` reads status, `Notification.requestPermission()`
//!   prompts and resolves to the new state. This is the one web permission
//!   with a first-class request flow, so it's the genuinely-runnable path.
//! - **Geolocation** is queryable through the Permissions API
//!   (`navigator.permissions.query({name:"geolocation"})`) but has **no**
//!   explicit request method — the prompt only fires on the first
//!   `getCurrentPosition` / `watchPosition`. So [`request`] for location
//!   can't honestly prompt; it reads status and, when that's
//!   [`Undetermined`](PermissionStatus::Undetermined), returns it unchanged
//!   (the caller surfaces the prompt by actually calling geolocation). This
//!   is documented rather than faked.
//!
//! `Camera`/`Microphone` likewise have only a Permissions-API status read on
//! web (the prompt fires on `getUserMedia`), mirroring the geolocation shape.

use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::Notification;

use crate::{Permission, PermissionStatus};

pub(super) async fn status(permission: Permission) -> PermissionStatus {
    match permission {
        Permission::Notifications => notification_status(),
        Permission::LocationWhenInUse | Permission::LocationAlways => {
            permissions_query("geolocation").await
        }
        Permission::Camera => permissions_query("camera").await,
        Permission::Microphone => permissions_query("microphone").await,
    }
}

pub(super) async fn request(permission: Permission) -> PermissionStatus {
    match permission {
        Permission::Notifications => request_notifications().await,
        // Geolocation / camera / microphone have no explicit web request
        // API — the prompt fires on first use (`getCurrentPosition`,
        // `getUserMedia`). We can't honestly prompt here, so report the
        // current status; an `Undetermined` means "will prompt on first
        // use". See the module docs.
        Permission::LocationWhenInUse | Permission::LocationAlways => {
            permissions_query("geolocation").await
        }
        Permission::Camera => permissions_query("camera").await,
        Permission::Microphone => permissions_query("microphone").await,
    }
}

/// `Notification.permission` — synchronous status, no prompt.
fn notification_status() -> PermissionStatus {
    // `Notification` is absent in some contexts (older browsers, workers
    // without it); treat that as no-such-permission rather than a crash.
    match Notification::permission() {
        web_sys::NotificationPermission::Granted => PermissionStatus::Granted,
        web_sys::NotificationPermission::Denied => PermissionStatus::Denied,
        web_sys::NotificationPermission::Default => PermissionStatus::Undetermined,
        _ => PermissionStatus::Undetermined,
    }
}

/// `Notification.requestPermission()` — prompts when undetermined and
/// resolves to the resulting state.
async fn request_notifications() -> PermissionStatus {
    let Ok(promise) = Notification::request_permission() else {
        // No Notification API in this context.
        return PermissionStatus::Unsupported;
    };
    match JsFuture::from(promise).await {
        Ok(value) => match value.as_string().as_deref() {
            Some("granted") => PermissionStatus::Granted,
            Some("denied") => PermissionStatus::Denied,
            Some("default") => PermissionStatus::Undetermined,
            // Some browsers resolve with `undefined` (the callback form);
            // re-read the now-updated synchronous status.
            _ => notification_status(),
        },
        Err(_) => notification_status(),
    }
}

/// `navigator.permissions.query({name})` — the passive status read. Support
/// is uneven (some browsers lack a given descriptor, or the API entirely),
/// so any failure degrades to [`PermissionStatus::Unsupported`].
async fn permissions_query(name: &str) -> PermissionStatus {
    let Some(window) = web_sys::window() else {
        return PermissionStatus::Unsupported;
    };
    let Ok(permissions) = window.navigator().permissions() else {
        return PermissionStatus::Unsupported;
    };
    let desc = js_sys::Object::new();
    if js_sys::Reflect::set(
        &desc,
        &JsValue::from_str("name"),
        &JsValue::from_str(name),
    )
    .is_err()
    {
        return PermissionStatus::Unsupported;
    }
    let Ok(promise) = permissions.query(&desc) else {
        return PermissionStatus::Unsupported;
    };
    let Ok(result) = JsFuture::from(promise).await else {
        return PermissionStatus::Unsupported;
    };
    match result.dyn_into::<web_sys::PermissionStatus>() {
        Ok(status) => match status.state() {
            web_sys::PermissionState::Granted => PermissionStatus::Granted,
            web_sys::PermissionState::Denied => PermissionStatus::Denied,
            web_sys::PermissionState::Prompt => PermissionStatus::Undetermined,
            _ => PermissionStatus::Undetermined,
        },
        Err(_) => PermissionStatus::Unsupported,
    }
}
