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
//!
//! Every browser call is a web-glue binding declared here (own-web-bindings
//! phase 3); Promises are awaited as `web_glue::JsFuture`.

use web_glue::{cast, string, JsFuture, JsValue};

use crate::{Permission, PermissionStatus};

web_glue::import! {
    // `Notification.permission` written to `out`, or 0 when the
    // Notification API is absent (iOS Safari outside an installed web app,
    // older engines, workers) — where the web-sys port's non-catching
    // static getter threw a ReferenceError through the wasm frames.
    fn js_notification_permission(out: usize) -> u32 =
        "(o) => { if (typeof Notification === 'undefined') return 0; \
           G.retStr(String(Notification.permission), o); return 1; }";
    // `Notification.requestPermission()` as a Promise. `Promise.resolve`
    // because older Safari implements only the callback form and returns
    // `undefined` — that settles as `undefined` here, and the caller
    // re-reads the synchronous status. Throws (→ Err) when `Notification`
    // is absent.
    #[catch]
    fn js_request_notification_permission() -> u32 =
        "() => G.add(Promise.resolve(Notification.requestPermission()))";
    // `navigator.permissions.query({ name })` → its Promise, or 0 without
    // a window / the Permissions API. Throws (→ Err) for a descriptor the
    // engine doesn't know (Firefox: "camera").
    #[catch]
    fn js_permissions_query(p: usize, l: usize) -> u32 =
        "(p, l) => { if (typeof window === 'undefined' || window.navigator.permissions == null) return 0; \
           return G.add(window.navigator.permissions.query({ name: G.str(p, l) })); }";
}

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
    // without it, iOS Safari outside an installed web app); treat that as
    // no-such-permission rather than a crash. Regression:
    // `tests/web_permissions.rs`.
    let mut has = 0;
    let state = string::receive(|o| has = unsafe { js_notification_permission(o) });
    if has == 0 {
        return PermissionStatus::Unsupported;
    }
    match state.as_str() {
        "granted" => PermissionStatus::Granted,
        "denied" => PermissionStatus::Denied,
        _ => PermissionStatus::Undetermined,
    }
}

/// `Notification.requestPermission()` — prompts when undetermined and
/// resolves to the resulting state.
async fn request_notifications() -> PermissionStatus {
    let Ok(promise) = (unsafe { js_request_notification_permission() }) else {
        // No Notification API in this context.
        return PermissionStatus::Unsupported;
    };
    // SAFETY: a fresh `G.add` slot the snippet minted for us.
    let promise = unsafe { JsValue::from_raw(promise) };
    match JsFuture::new(&promise).await {
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
    let (p, l) = string::abi(name);
    let promise = match unsafe { js_permissions_query(p, l) } {
        // SAFETY: a fresh `G.add` slot the snippet minted for us.
        Ok(h) if h != 0 => unsafe { JsValue::from_raw(h) },
        _ => return PermissionStatus::Unsupported,
    };
    let Ok(result) = JsFuture::new(&promise).await else {
        return PermissionStatus::Unsupported;
    };
    if !cast::instance_of(&result, "PermissionStatus") {
        return PermissionStatus::Unsupported;
    }
    match result.get("state").ok().and_then(|s| s.as_string()).as_deref() {
        Some("granted") => PermissionStatus::Granted,
        Some("denied") => PermissionStatus::Denied,
        _ => PermissionStatus::Undetermined,
    }
}
