//! iOS / macOS / tvOS local notifications via `UNUserNotificationCenter`
//! (objc2).
//!
//! **Compile-checked only ⚠️** — this messages real UserNotifications APIs
//! through the Obj-C runtime; it has not been exercised on a device from
//! this crate. The shape mirrors the documented UNUserNotificationCenter
//! flow.
//!
//! Mechanism:
//! - `notify` / `schedule` build a `UNMutableNotificationContent`
//!   (`setTitle:` / `setSubtitle:` / `setBody:` / `setUserInfo:`), wrap it
//!   in a `UNNotificationRequest` keyed by the resolved id, and hand it to
//!   `[center addNotificationRequest:withCompletionHandler:]`. Immediate
//!   posts use a `nil` trigger; delayed posts use
//!   `UNTimeIntervalNotificationTrigger` with `timeInterval:` =
//!   `after.as_secs_f64()`.
//! - `cancel` calls `removePendingNotificationRequestsWithIdentifiers:` and
//!   `removeDeliveredNotificationsWithIdentifiers:`; `cancel_all` calls the
//!   `…All…` variants.
//!
//! Authorization is **not** requested here — the public `authorize()` goes
//! through the `permissions` crate's `UNUserNotificationCenter`
//! authorization flow. We only post; an un-granted post is silently dropped
//! by the OS, so callers must gate on `authorize()` (the public API
//! documents this).
//!
//! Invariant: `UNUserNotificationCenter.currentNotificationCenter` is a
//! process-wide singleton, so we hold its raw pointer for the call without
//! retaining — same approach the `storage`/`microphone` SDKs use for their
//! Obj-C singletons.
//!
//! ## APNs token = host seam
//!
//! [`push_token`] needs `[[UIApplication sharedApplication]
//! registerForRemoteNotifications]` and the token delivered to the app
//! delegate's `didRegisterForRemoteNotificationsWithDeviceToken:`. That
//! callback lives in the host app, not in a library, and remote push also
//! requires the `aps-environment` entitlement + a `remote-notification`
//! background mode the app declares. Until the host installs a bridge that
//! stashes the token for us, we report `NotSupported`. This is the
//! documented push seam — not a stub we can fill from inside the crate.

use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use objc2_foundation::NSString;
use std::ffi::c_void;

use crate::{resolve_id, Notification, NotificationId, NotifyError, PushToken};

/// `[UNUserNotificationCenter currentNotificationCenter]` — a process-wide
/// singleton; the raw pointer is valid for the program's lifetime without
/// retaining.
unsafe fn center() -> *mut AnyObject {
    msg_send![class!(UNUserNotificationCenter), currentNotificationCenter]
}

/// Build a `UNMutableNotificationContent*` from `n`. Returns an
/// autoreleased object owned by the surrounding autorelease pool.
unsafe fn make_content(n: &Notification) -> *mut AnyObject {
    let content: *mut AnyObject = msg_send![class!(UNMutableNotificationContent), new];
    let title = NSString::from_str(&n.title);
    let _: () = msg_send![content, setTitle: &*title];
    let body = NSString::from_str(&n.body);
    let _: () = msg_send![content, setBody: &*body];
    if let Some(sub) = &n.subtitle {
        let sub = NSString::from_str(sub);
        let _: () = msg_send![content, setSubtitle: &*sub];
    }
    if !n.data.is_empty() {
        // userInfo carries the opaque app payload (surfaced to the tap
        // handler). Build an NSMutableDictionary<NSString*, NSString*> by
        // raw messaging — the typed `NSDictionary` helpers require
        // `NSString: IsRetainable`, which this objc2-foundation version
        // doesn't provide, so we go through the runtime directly.
        let dict: *mut AnyObject = msg_send![class!(NSMutableDictionary), dictionary];
        for (k, v) in &n.data {
            let key = NSString::from_str(k);
            let val = NSString::from_str(v);
            let _: () = msg_send![dict, setObject: &*val, forKey: &*key];
        }
        let _: () = msg_send![content, setUserInfo: dict];
    }
    content
}

/// Post `content` under `id` with an optional `trigger` (nil = immediate).
unsafe fn add_request(id: &NotificationId, content: *mut AnyObject, trigger: *mut AnyObject) {
    let ident = NSString::from_str(id.as_str());
    let request: *mut AnyObject = msg_send![
        class!(UNNotificationRequest),
        requestWithIdentifier: &*ident,
        content: content,
        trigger: trigger,
    ];
    // nil completion handler — fire-and-forget; the OS queues it.
    let nil: *mut c_void = std::ptr::null_mut();
    let _: () = msg_send![center(), addNotificationRequest: request, withCompletionHandler: nil];
}

pub(super) async fn notify(n: Notification) -> Result<NotificationId, NotifyError> {
    let id = resolve_id(&n);
    unsafe {
        let content = make_content(&n);
        // nil trigger = deliver immediately.
        add_request(&id, content, std::ptr::null_mut());
    }
    Ok(id)
}

pub(super) async fn schedule(
    n: Notification,
    after: std::time::Duration,
) -> Result<NotificationId, NotifyError> {
    let id = resolve_id(&n);
    // UNTimeIntervalNotificationTrigger requires a strictly-positive
    // interval; clamp a zero/sub-second delay up so the OS accepts it.
    let secs = after.as_secs_f64().max(0.001);
    unsafe {
        let content = make_content(&n);
        let trigger: *mut AnyObject = msg_send![
            class!(UNTimeIntervalNotificationTrigger),
            triggerWithTimeInterval: secs,
            repeats: false,
        ];
        add_request(&id, content, trigger);
    }
    Ok(id)
}

pub(super) async fn cancel(id: &NotificationId) {
    unsafe {
        let ident = NSString::from_str(id.as_str());
        // `[NSArray arrayWithObject:]` — raw messaging (the typed
        // `NSArray::from_slice` requires `NSString: IsRetainable`, absent
        // in this objc2-foundation version).
        let arr: *mut AnyObject = msg_send![class!(NSArray), arrayWithObject: &*ident];
        let c = center();
        let _: () = msg_send![c, removePendingNotificationRequestsWithIdentifiers: arr];
        let _: () = msg_send![c, removeDeliveredNotificationsWithIdentifiers: arr];
    }
}

pub(super) async fn cancel_all() {
    unsafe {
        let c = center();
        let _: () = msg_send![c, removeAllPendingNotificationRequests];
        let _: () = msg_send![c, removeAllDeliveredNotifications];
    }
}

pub(super) async fn push_token() -> Result<PushToken, NotifyError> {
    // APNs token comes from `registerForRemoteNotifications` + the app
    // delegate callback — a host seam (see module docs). No in-library path.
    Err(NotifyError::NotSupported)
}
