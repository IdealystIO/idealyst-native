//! Android share via `Intent.ACTION_SEND` (or `ACTION_SEND_MULTIPLE`) wrapped
//! in `Intent.createChooser`, started on the current Activity.
//!
//! Text and URL sharing are fully self-contained — no Kotlin shim, no
//! `MainActivity` edits: we build the intent through JNI against
//! `android.content.Intent`, set `EXTRA_TEXT` (and `EXTRA_SUBJECT` for a
//! title), `createChooser`, and `startActivity` on the Activity from
//! `ndk_context::android_context()`.
//!
//! ## The file-sharing seam
//!
//! Attaching files (`EXTRA_STREAM`) requires a `content://` URI from a
//! `FileProvider` declared in the app's manifest with a matching
//! `<paths>` resource — you cannot legally put a raw `file://` URI in an
//! `ACTION_SEND` on modern Android (it throws `FileUriExposedException`). That
//! manifest/provider wiring is app-level configuration this SDK can't inject
//! generically, so **file sharing is a documented seam**: `share(...)` with
//! files but no text/url returns [`ShareError::NotSupported`] on Android (the
//! README flags it). Text/URL share works fully. A future layer can add a
//! FileProvider shim (like `file-export`'s Kotlin helper).
//!
//! ## Outcome mapping
//!
//! `startActivity(createChooser(...))` does not report back whether the user
//! picked a target — there's no result callback for a plain chooser. So we
//! report [`ShareOutcome::Completed`] once the chooser has been launched
//! successfully (see `ShareOutcome`'s best-effort note). A JNI failure
//! launching it is a real [`ShareError::Backend`].
//!
//! VERIFICATION: compile-checked for `aarch64-linux-android`; the Intent /
//! Activity path resolves only at runtime on a device (same posture as
//! `file-export`).

use jni::objects::{JObject, JValue};
use jni::{JNIEnv, JavaVM};

use crate::{ShareContent, ShareError, ShareOutcome};

pub(crate) async fn share(content: &ShareContent) -> Result<ShareOutcome, ShareError> {
    // File sharing needs a FileProvider content URI we can't synthesize here.
    // If the *only* thing to share is files, that's NotSupported on Android;
    // otherwise we share the text/url and ignore the files.
    let has_text_or_url = content.text.is_some() || content.url.is_some();
    if !has_text_or_url && !content.files.is_empty() {
        return Err(ShareError::NotSupported);
    }

    let vm = java_vm()?;
    let mut env = vm.attach_current_thread().map_err(jni_err)?;
    let activity = android_context();

    launch_chooser(&mut env, &activity, content)?;
    Ok(ShareOutcome::Completed)
}

/// Build `ACTION_SEND` with the text/url body + optional subject, wrap in
/// `createChooser`, and `startActivity`.
fn launch_chooser(
    env: &mut JNIEnv,
    activity: &JObject,
    content: &ShareContent,
) -> Result<(), ShareError> {
    // The shared text: text and url joined by a newline if both are present
    // (Android's ACTION_SEND carries a single EXTRA_TEXT string).
    let body = match (&content.text, &content.url) {
        (Some(t), Some(u)) => format!("{t}\n{u}"),
        (Some(t), None) => t.clone(),
        (None, Some(u)) => u.clone(),
        (None, None) => String::new(),
    };

    // intent = new Intent(); intent.setAction(Intent.ACTION_SEND);
    let intent = env
        .new_object("android/content/Intent", "()V", &[])
        .map_err(jni_err)?;
    let action_send = env.new_string("android.intent.action.SEND").map_err(jni_err)?;
    env.call_method(
        &intent,
        "setAction",
        "(Ljava/lang/String;)Landroid/content/Intent;",
        &[JValue::Object(&JObject::from(action_send))],
    )
    .map_err(jni_err)?;

    // intent.setType("text/plain");
    let mime = env.new_string("text/plain").map_err(jni_err)?;
    env.call_method(
        &intent,
        "setType",
        "(Ljava/lang/String;)Landroid/content/Intent;",
        &[JValue::Object(&JObject::from(mime))],
    )
    .map_err(jni_err)?;

    // intent.putExtra(Intent.EXTRA_TEXT, body);
    put_string_extra(env, &intent, "android.intent.extra.TEXT", &body)?;

    // intent.putExtra(Intent.EXTRA_SUBJECT, title) when present.
    if let Some(title) = &content.title {
        put_string_extra(env, &intent, "android.intent.extra.SUBJECT", title)?;
    }

    // chooser = Intent.createChooser(intent, null);
    let chooser = env
        .call_static_method(
            "android/content/Intent",
            "createChooser",
            "(Landroid/content/Intent;Ljava/lang/CharSequence;)Landroid/content/Intent;",
            &[
                JValue::Object(&intent),
                JValue::Object(&JObject::null()),
            ],
        )
        .map_err(jni_err)?
        .l()
        .map_err(jni_err)?;

    // FLAG_ACTIVITY_NEW_TASK = 0x10000000 — required when starting from a
    // non-Activity context can occur; harmless from an Activity.
    const FLAG_ACTIVITY_NEW_TASK: i32 = 0x1000_0000;
    env.call_method(
        &chooser,
        "addFlags",
        "(I)Landroid/content/Intent;",
        &[JValue::Int(FLAG_ACTIVITY_NEW_TASK)],
    )
    .map_err(jni_err)?;

    // activity.startActivity(chooser);
    env.call_method(
        activity,
        "startActivity",
        "(Landroid/content/Intent;)V",
        &[JValue::Object(&chooser)],
    )
    .map_err(jni_err)?;

    Ok(())
}

/// `intent.putExtra(name, value)` for a `String` extra.
fn put_string_extra(
    env: &mut JNIEnv,
    intent: &JObject,
    name: &str,
    value: &str,
) -> Result<(), ShareError> {
    let jname = env.new_string(name).map_err(jni_err)?;
    let jvalue = env.new_string(value).map_err(jni_err)?;
    env.call_method(
        intent,
        "putExtra",
        "(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;",
        &[
            JValue::Object(&JObject::from(jname)),
            JValue::Object(&JObject::from(jvalue)),
        ],
    )
    .map_err(jni_err)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// JNI helpers (same shape as file-export's android backend).
// ---------------------------------------------------------------------------

fn java_vm() -> Result<JavaVM, ShareError> {
    let ctx = ndk_context::android_context();
    let vm_ptr = ctx.vm() as *mut jni::sys::JavaVM;
    unsafe { JavaVM::from_raw(vm_ptr) }
        .map_err(|e| ShareError::Backend(format!("invalid JavaVM pointer: {e}")))
}

fn jni_err(e: jni::errors::Error) -> ShareError {
    ShareError::Backend(format!("JNI: {e}"))
}

fn android_context<'a>() -> JObject<'a> {
    let ctx = ndk_context::android_context();
    unsafe { JObject::from_raw(ctx.context().cast()) }
}
