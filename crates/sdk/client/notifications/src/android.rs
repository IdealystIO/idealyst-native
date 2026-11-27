//! Android local notifications and responses: `NotificationManager`,
//! notification channels, `AlarmManager`, and the Kotlin shim
//! `runtime/kotlin/io/idealyst/notifications/RustNotifications.kt`, over JNI.
//!
//! **Compile-checked only ⚠️** — this drives real Android framework classes
//! through JNI and the shim; it has not been exercised on a device/emulator
//! from this crate.
//!
//! ## Posting
//!
//! `notify` / `schedule` ask the shim for a `Notification.Builder` with the
//! shared fields set (`RustNotifications.builder`): title, body, sub-text
//! (subtitle), group, the channel — one per [`Priority`], plus one silent
//! channel for [`Sound::Silent`] (since Android 8 the sound belongs to the
//! channel, so a silent notification is posted on a channel without one, at
//! normal importance whatever its priority), each named from
//! [`crate::labels`] — the image as a large icon
//! and big picture, up to three action buttons (with a `RemoteInput` for a
//! text field), and the tap / dismiss intents. The app's
//! [`Notification::android`] hook then edits the builder, and
//! `RustNotifications.show` posts it — `manager.notify(tag, …)` now, or for
//! `schedule` an `AlarmManager` alarm whose intent carries the *built*
//! notification, so the alarm posts exactly what was built (hooks
//! included). The int `tag` is a stable hash of the string id, so
//! re-posting the same id *replaces* the notification — alerting again,
//! unless [`Notification::alert_once`] sets the builder's
//! `setOnlyAlertOnce`.
//!
//! ## Responses
//!
//! The shim's `NotificationOpenActivity` (taps, actions that open the app)
//! and `NotificationReceiver` (other actions, dismissals) report each
//! response to [`nativeResponse`] when this process's Rust side has
//! attached, and queue it in `SharedPreferences` otherwise — the tap that
//! cold-starts the app happens before Rust boots, and a background action
//! may arrive while the app is not running at all. [`init`] attaches and
//! drains the queue. Both components are declared by this crate's manifest
//! fragment; the generated `MainActivity` is untouched.
//!
//! Needs the `POST_NOTIFICATIONS` runtime permission on API 33+ (declared
//! by this crate's capability metadata, granted through the `permissions`
//! crate).
//!
//! ## FCM = host seam
//!
//! [`push_token`] is the FCM registration token, which needs a Firebase
//! project + `google-services.json` the app supplies → `NotSupported`.
//!
//! [`nativeResponse`]: Java_io_idealyst_notifications_RustNotifications_nativeResponse

use std::collections::HashMap;

use jni::objects::{JClass, JObject, JString, JValue};
use jni::{JNIEnv, JavaVM};
use runtime_core::__serde_json::{self as serde_json, json, Value};

use crate::{
    deliver_response, labels, resolve_id, visible_actions, Notification, NotificationId, NotifyError, Priority,
    PushToken, ResponseAction, Sound,
};

const HELPER: &str = "io/idealyst/notifications/RustNotifications";
/// Android shows at most three action buttons.
const MAX_ACTIONS: usize = 3;
/// The shim's response encodings (`RustNotifications.OPEN` / `DISMISS` /
/// `ACTION_PREFIX`).
const OPEN: &str = "open";
const DISMISS: &str = "dismiss";
const ACTION_PREFIX: &str = "action:";

fn java_vm() -> Result<JavaVM, NotifyError> {
    let ctx = ndk_context::android_context();
    let vm_ptr = ctx.vm() as *mut jni::sys::JavaVM;
    unsafe { JavaVM::from_raw(vm_ptr) }.map_err(|e| NotifyError::Backend(format!("invalid JavaVM pointer: {e}")))
}

fn map_jni(e: jni::errors::Error) -> NotifyError {
    NotifyError::Backend(format!("JNI: {e}"))
}

fn context<'a>() -> JObject<'a> {
    let ctx = ndk_context::android_context();
    unsafe { JObject::from_raw(ctx.context().cast()) }
}

/// A stable int tag from the string id — Android keys notifications by an
/// int, and re-posting the same int replaces the notification. FNV-1a over
/// the id bytes.
pub(crate) fn int_tag(id: &str) -> i32 {
    let mut hash: u32 = 0x811c_9dc5;
    for b in id.as_bytes() {
        hash ^= *b as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash as i32
}

/// The shim's priority index (`RustNotifications.builder`).
fn priority_index(p: Priority) -> i32 {
    match p {
        Priority::Low => 0,
        Priority::Default => 1,
        Priority::High => 2,
        Priority::Urgent => 3,
    }
}

/// The action list as the shim's JSON.
pub(crate) fn actions_json(n: &Notification) -> String {
    let actions: Vec<Value> = visible_actions(n, MAX_ACTIONS)
        .iter()
        .map(|a| {
            json!({
                "id": a.id,
                "title": a.title,
                "input": a.input.as_ref().map(|i| json!({ "button": i.button_title, "placeholder": i.placeholder })),
                "opens_app": a.opens_app,
            })
        })
        .collect();
    Value::Array(actions).to_string()
}

/// A shim response encoding as a [`ResponseAction`].
pub(crate) fn parse_action(action: &str) -> ResponseAction {
    match action {
        OPEN => ResponseAction::Open,
        DISMISS => ResponseAction::Dismiss,
        other => ResponseAction::Action(other.strip_prefix(ACTION_PREFIX).unwrap_or(other).to_string()),
    }
}

fn parse_data(json: &str) -> HashMap<String, String> {
    serde_json::from_str::<HashMap<String, Value>>(json)
        .map(|m| {
            m.into_iter()
                .filter_map(|(k, v)| match v {
                    Value::String(s) => Some((k, s)),
                    Value::Null => None,
                    other => Some((k, other.to_string())),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Deliver the shim's queued responses (`attach`'s JSON array).
pub(crate) fn deliver_queued(json: &str) {
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(json) else { return };
    for item in items {
        let Some(id) = item["id"].as_str() else { continue };
        let action = parse_action(item["action"].as_str().unwrap_or(OPEN));
        let text = item["text"].as_str().map(str::to_string);
        let data = parse_data(&item["data"].to_string());
        deliver_response(NotificationId(id.to_string()), action, text, data);
    }
}

pub(super) fn init() {
    let attached = (|| -> Result<String, NotifyError> {
        let vm = java_vm()?;
        let mut env = vm.attach_current_thread().map_err(map_jni)?;
        let ctx = context();
        let queued = env
            .call_static_method(HELPER, "attach", "(Landroid/content/Context;)Ljava/lang/String;", &[JValue::Object(&ctx)])
            .map_err(map_jni)?
            .l()
            .map_err(map_jni)?;
        let queued = JString::from(queued);
        let s: String = env.get_string(&queued).map_err(map_jni)?.into();
        Ok(s)
    })();
    match attached {
        Ok(queued) => deliver_queued(&queued),
        Err(e) => eprintln!("notifications: attaching the Android shim failed: {e}"),
    }
}

/// Build and post `n` — now, or after `delay_ms`.
fn post(n: Notification, delay_ms: i64) -> Result<NotificationId, NotifyError> {
    let id = resolve_id(&n);
    let tag = int_tag(id.as_str());
    let vm = java_vm()?;
    let mut env = vm.attach_current_thread().map_err(map_jni)?;
    let ctx = context();

    let data = serde_json::to_string(&n.data).map_err(|e| NotifyError::Backend(format!("payload: {e}")))?;
    let jid = env.new_string(id.as_str()).map_err(map_jni)?;
    let title = env.new_string(&n.title).map_err(map_jni)?;
    let body = env.new_string(&n.body).map_err(map_jni)?;
    let subtitle = optional_string(&mut env, n.subtitle.as_deref())?;
    let group = optional_string(&mut env, n.group.as_deref())?;
    let labels = labels();
    let channel_name = if n.sound == Sound::Silent { &labels.silent } else { labels.for_priority(n.priority) };
    let channel_name = env.new_string(channel_name).map_err(map_jni)?;
    let image = optional_string(&mut env, n.image.as_deref())?;
    let data = env.new_string(data).map_err(map_jni)?;
    let actions = env.new_string(actions_json(&n)).map_err(map_jni)?;

    let builder = env
        .call_static_method(
            HELPER,
            "builder",
            "(Landroid/content/Context;Ljava/lang/String;ILjava/lang/String;Ljava/lang/String;Ljava/lang/String;\
             Ljava/lang/String;ILjava/lang/String;ZLjava/lang/String;Ljava/lang/String;Ljava/lang/String;)\
             Landroid/app/Notification$Builder;",
            &[
                JValue::Object(&ctx),
                JValue::Object(&jid),
                JValue::Int(tag),
                JValue::Object(&title),
                JValue::Object(&body),
                JValue::Object(&subtitle),
                JValue::Object(&group),
                JValue::Int(priority_index(n.priority)),
                JValue::Object(&channel_name),
                JValue::Bool((n.sound == Sound::Silent) as u8),
                JValue::Object(&image),
                JValue::Object(&data),
                JValue::Object(&actions),
            ],
        )
        .map_err(map_jni)?
        .l()
        .map_err(map_jni)?;

    // Android's own "alert once": a re-post under a tag that is still shown
    // updates it without sound, vibration or heads-up. `schedule` always
    // alerts (`Notification::alert_once` is decided when posting).
    if n.alert_once && delay_ms == 0 {
        env.call_method(&builder, "setOnlyAlertOnce", "(Z)Landroid/app/Notification$Builder;", &[JValue::Bool(1)])
            .map_err(map_jni)?;
    }
    if let Some(hook) = &n.hooks.android {
        hook(&mut env, &builder).map_err(map_jni)?;
    }

    env.call_static_method(
        HELPER,
        "show",
        "(Landroid/content/Context;ILandroid/app/Notification$Builder;J)V",
        &[JValue::Object(&ctx), JValue::Int(tag), JValue::Object(&builder), JValue::Long(delay_ms)],
    )
    .map_err(map_jni)?;
    Ok(id)
}

/// A Java string for `s`, or a null reference.
fn optional_string<'a>(env: &mut JNIEnv<'a>, s: Option<&str>) -> Result<JObject<'a>, NotifyError> {
    match s {
        Some(s) => Ok(JObject::from(env.new_string(s).map_err(map_jni)?)),
        None => Ok(JObject::null()),
    }
}

pub(super) async fn notify(n: Notification) -> Result<NotificationId, NotifyError> {
    post(n, 0)
}

pub(super) async fn schedule(n: Notification, after: std::time::Duration) -> Result<NotificationId, NotifyError> {
    // At least 1 ms: the shim posts immediately for a zero delay.
    post(n, (after.as_millis() as i64).max(1))
}

pub(super) async fn cancel(id: &NotificationId) {
    let _ = call_with_context("cancel", "(Landroid/content/Context;I)V", Some(int_tag(id.as_str())));
}

pub(super) async fn cancel_all() {
    let _ = call_with_context("cancelAll", "(Landroid/content/Context;)V", None);
}

fn call_with_context(method: &str, sig: &str, tag: Option<i32>) -> Result<(), NotifyError> {
    let vm = java_vm()?;
    let mut env = vm.attach_current_thread().map_err(map_jni)?;
    let ctx = context();
    match tag {
        Some(tag) => env.call_static_method(HELPER, method, sig, &[JValue::Object(&ctx), JValue::Int(tag)]),
        None => env.call_static_method(HELPER, method, sig, &[JValue::Object(&ctx)]),
    }
    .map_err(map_jni)?;
    Ok(())
}

pub(super) async fn push_token() -> Result<PushToken, NotifyError> {
    // FCM token needs a Firebase project + google-services.json the app
    // supplies (host seam — see module docs).
    Err(NotifyError::NotSupported)
}

/// `RustNotifications.nativeResponse` — one response, on the main thread.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_notifications_RustNotifications_nativeResponse(
    mut env: JNIEnv,
    _class: JClass,
    id: JString,
    action: JString,
    text: JString,
    data: JString,
) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let read = |env: &mut JNIEnv, s: &JString| -> Option<String> {
            if s.is_null() {
                return None;
            }
            env.get_string(s).ok().map(Into::into)
        };
        let Some(id) = read(&mut env, &id) else { return };
        let action = parse_action(&read(&mut env, &action).unwrap_or_else(|| OPEN.into()));
        let text = read(&mut env, &text);
        let data = parse_data(&read(&mut env, &data).unwrap_or_default());
        deliver_response(NotificationId(id), action, text, data);
    }));
    if result.is_err() {
        eprintln!("notifications: panic in nativeResponse; aborting");
        std::process::abort();
    }
}

// Pin the export so the linker keeps it in the app `cdylib`'s dynsym for the
// JVM's `dlsym` resolution.
#[used]
static KEEP_RESPONSE: extern "system" fn(JNIEnv, JClass, JString, JString, JString, JString) =
    Java_io_idealyst_notifications_RustNotifications_nativeResponse;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Action;

    #[test]
    fn shim_action_encodings_parse() {
        assert_eq!(parse_action("open"), ResponseAction::Open);
        assert_eq!(parse_action("dismiss"), ResponseAction::Dismiss);
        assert_eq!(parse_action("action:reply"), ResponseAction::Action("reply".into()));
        // An action id that itself looks like an encoding stays an action.
        assert_eq!(parse_action("action:open"), ResponseAction::Action("open".into()));
    }

    #[test]
    fn actions_json_caps_at_three_and_carries_inputs() {
        let n = Notification::new("T", "B")
            .action(Action::new("reply", "Reply").text_input("Send", "Message"))
            .action(Action::new("a", "A").opens_app(false))
            .action(Action::new("b", "B"))
            .action(Action::new("c", "C"));
        let v: Value = serde_json::from_str(&actions_json(&n)).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 3);
        assert_eq!(v[0]["input"], json!({ "button": "Send", "placeholder": "Message" }));
        assert_eq!(v[1]["opens_app"], json!(false));
        assert_eq!(v[2]["input"], Value::Null);
    }

    #[test]
    fn int_tag_is_stable() {
        assert_eq!(int_tag("note-1"), int_tag("note-1"));
        assert_ne!(int_tag("note-1"), int_tag("note-2"));
    }
}
