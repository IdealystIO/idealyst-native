//! Android local notifications via `NotificationManager` +
//! `NotificationChannel` + `Notification.Builder`, over JNI.
//!
//! **Compile-checked only ⚠️** — this drives real Android framework classes
//! through JNI; it has not been exercised on a device/emulator from this
//! crate. The shape mirrors the documented NotificationManager flow.
//!
//! Mechanism (immediate `notify`):
//! 1. Get the `NotificationManager` system service from the host context.
//! 2. Ensure a `NotificationChannel` exists (API 26+): create one with a
//!    stable id + `IMPORTANCE_DEFAULT` and register it. Idempotent — the
//!    platform de-dupes by channel id.
//! 3. Build a `Notification.Builder(context, channelId)`, set the small
//!    icon (the app's launcher icon), `setContentTitle` / `setContentText`
//!    (and `setSubText` for the subtitle), `build()`.
//! 4. `manager.notify(intId, notification)`, where `intId` is a stable hash
//!    of the string id so re-posting the same id *replaces* the
//!    notification (Android keys by an int tag).
//!
//! JNI work runs on the calling thread (attached per op via the host
//! `JavaVM` from `ndk_context`, mirroring the `storage`/`net` SDKs).
//!
//! ## Scheduling + FCM = host seams
//!
//! - **`schedule`** by delay needs `AlarmManager` + a `BroadcastReceiver`
//!   the app registers in its manifest (the receiver re-posts the
//!   notification when the alarm fires). A `BroadcastReceiver` is host
//!   manifest wiring, not something a pure-JNI library can register
//!   cleanly, so `schedule` returns `NotSupported` here and immediate
//!   `notify` is implemented fully. See the README push/scheduling seam.
//! - **`push_token`** is the FCM registration token from
//!   `FirebaseMessaging.getInstance().getToken()`, which needs a Firebase
//!   project + `google-services.json` the app supplies. Host seam →
//!   `NotSupported`.
//!
//! Needs the `POST_NOTIFICATIONS` runtime permission on API 33+ (declared
//! by this crate's capability metadata, granted through the `permissions`
//! crate).

use jni::objects::{JObject, JValue};
use jni::JavaVM;

use crate::{resolve_id, Notification, NotificationId, NotifyError, PushToken};

// android.app.NotificationManager.IMPORTANCE_DEFAULT
const IMPORTANCE_DEFAULT: i32 = 3;
// The single channel this SDK posts on. A richer API would expose channels;
// the unopinionated capability uses one default channel.
const CHANNEL_ID: &str = "idealyst_default";
const CHANNEL_NAME: &str = "Notifications";

fn java_vm() -> Result<JavaVM, NotifyError> {
    let ctx = ndk_context::android_context();
    let vm_ptr = ctx.vm() as *mut jni::sys::JavaVM;
    unsafe { JavaVM::from_raw(vm_ptr) }
        .map_err(|e| NotifyError::Backend(format!("invalid JavaVM pointer: {e}")))
}

fn map_jni(e: jni::errors::Error) -> NotifyError {
    NotifyError::Backend(format!("JNI: {e}"))
}

/// A stable int tag from the string id — Android keys notifications by an
/// int, and re-posting the same int replaces the notification (the update
/// semantics our public API promises). FNV-1a over the id bytes.
fn int_tag(id: &str) -> i32 {
    let mut hash: u32 = 0x811c_9dc5;
    for b in id.as_bytes() {
        hash ^= *b as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash as i32
}

/// `context.getSystemService(Context.NOTIFICATION_SERVICE)` → the
/// `NotificationManager`.
fn notification_manager<'a>(
    env: &mut jni::JNIEnv<'a>,
    activity: &JObject<'a>,
) -> Result<JObject<'a>, NotifyError> {
    let name = env.new_string("notification").map_err(map_jni)?;
    env.call_method(
        activity,
        "getSystemService",
        "(Ljava/lang/String;)Ljava/lang/Object;",
        &[JValue::Object(&JObject::from(name))],
    )
    .map_err(map_jni)?
    .l()
    .map_err(map_jni)
}

/// Create + register the default `NotificationChannel` (API 26+).
/// Idempotent — `createNotificationChannel` replaces a same-id channel.
fn ensure_channel<'a>(
    env: &mut jni::JNIEnv<'a>,
    manager: &JObject<'a>,
) -> Result<(), NotifyError> {
    let id = env.new_string(CHANNEL_ID).map_err(map_jni)?;
    let name = env.new_string(CHANNEL_NAME).map_err(map_jni)?;
    let channel = env
        .new_object(
            "android/app/NotificationChannel",
            "(Ljava/lang/String;Ljava/lang/CharSequence;I)V",
            &[
                JValue::Object(&JObject::from(id)),
                JValue::Object(&JObject::from(name)),
                JValue::Int(IMPORTANCE_DEFAULT),
            ],
        )
        .map_err(map_jni)?;
    env.call_method(
        manager,
        "createNotificationChannel",
        "(Landroid/app/NotificationChannel;)V",
        &[JValue::Object(&channel)],
    )
    .map_err(map_jni)?;
    Ok(())
}

/// `context.getApplicationInfo().icon` — the app's launcher icon resource id,
/// required by `setSmallIcon` (a notification with no small icon is rejected).
fn app_icon<'a>(
    env: &mut jni::JNIEnv<'a>,
    activity: &JObject<'a>,
) -> Result<i32, NotifyError> {
    let info = env
        .call_method(
            activity,
            "getApplicationInfo",
            "()Landroid/content/pm/ApplicationInfo;",
            &[],
        )
        .map_err(map_jni)?
        .l()
        .map_err(map_jni)?;
    env.get_field(&info, "icon", "I").map_err(map_jni)?.i().map_err(map_jni)
}

pub(super) async fn notify(n: Notification) -> Result<NotificationId, NotifyError> {
    let id = resolve_id(&n);
    let vm = java_vm()?;
    let mut env = vm.attach_current_thread().map_err(map_jni)?;

    let ctx = ndk_context::android_context();
    let activity = unsafe { JObject::from_raw(ctx.context().cast()) };

    let manager = notification_manager(&mut env, &activity)?;
    ensure_channel(&mut env, &manager)?;
    let icon = app_icon(&mut env, &activity)?;

    // Notification.Builder(context, channelId) (API 26+).
    let channel = env.new_string(CHANNEL_ID).map_err(map_jni)?;
    let builder = env
        .new_object(
            "android/app/Notification$Builder",
            "(Landroid/content/Context;Ljava/lang/String;)V",
            &[
                JValue::Object(&activity),
                JValue::Object(&JObject::from(channel)),
            ],
        )
        .map_err(map_jni)?;

    let title = env.new_string(&n.title).map_err(map_jni)?;
    env.call_method(
        &builder,
        "setContentTitle",
        "(Ljava/lang/CharSequence;)Landroid/app/Notification$Builder;",
        &[JValue::Object(&JObject::from(title))],
    )
    .map_err(map_jni)?;

    let body = env.new_string(&n.body).map_err(map_jni)?;
    env.call_method(
        &builder,
        "setContentText",
        "(Ljava/lang/CharSequence;)Landroid/app/Notification$Builder;",
        &[JValue::Object(&JObject::from(body))],
    )
    .map_err(map_jni)?;

    if let Some(sub) = &n.subtitle {
        let sub = env.new_string(sub).map_err(map_jni)?;
        env.call_method(
            &builder,
            "setSubText",
            "(Ljava/lang/CharSequence;)Landroid/app/Notification$Builder;",
            &[JValue::Object(&JObject::from(sub))],
        )
        .map_err(map_jni)?;
    }

    env.call_method(
        &builder,
        "setSmallIcon",
        "(I)Landroid/app/Notification$Builder;",
        &[JValue::Int(icon)],
    )
    .map_err(map_jni)?;

    let notification = env
        .call_method(&builder, "build", "()Landroid/app/Notification;", &[])
        .map_err(map_jni)?
        .l()
        .map_err(map_jni)?;

    env.call_method(
        &manager,
        "notify",
        "(ILandroid/app/Notification;)V",
        &[JValue::Int(int_tag(id.as_str())), JValue::Object(&notification)],
    )
    .map_err(map_jni)?;

    Ok(id)
}

pub(super) async fn schedule(
    _n: Notification,
    _after: std::time::Duration,
) -> Result<NotificationId, NotifyError> {
    // Delay scheduling needs an AlarmManager + a manifest-registered
    // BroadcastReceiver (host seam — see module docs). Immediate notify is
    // implemented; scheduling is the documented follow-on.
    Err(NotifyError::NotSupported)
}

pub(super) async fn cancel(id: &NotificationId) {
    let _ = cancel_inner(id);
}

fn cancel_inner(id: &NotificationId) -> Result<(), NotifyError> {
    let vm = java_vm()?;
    let mut env = vm.attach_current_thread().map_err(map_jni)?;
    let ctx = ndk_context::android_context();
    let activity = unsafe { JObject::from_raw(ctx.context().cast()) };
    let manager = notification_manager(&mut env, &activity)?;
    env.call_method(
        &manager,
        "cancel",
        "(I)V",
        &[JValue::Int(int_tag(id.as_str()))],
    )
    .map_err(map_jni)?;
    Ok(())
}

pub(super) async fn cancel_all() {
    let _ = cancel_all_inner();
}

fn cancel_all_inner() -> Result<(), NotifyError> {
    let vm = java_vm()?;
    let mut env = vm.attach_current_thread().map_err(map_jni)?;
    let ctx = ndk_context::android_context();
    let activity = unsafe { JObject::from_raw(ctx.context().cast()) };
    let manager = notification_manager(&mut env, &activity)?;
    env.call_method(&manager, "cancelAll", "()V", &[])
        .map_err(map_jni)?;
    Ok(())
}

pub(super) async fn push_token() -> Result<PushToken, NotifyError> {
    // FCM token needs a Firebase project + google-services.json the app
    // supplies (host seam — see module docs). No in-library path.
    Err(NotifyError::NotSupported)
}
