//! Android clipboard backend — `ClipboardManager` via JNI.
//!
//! We reach the system `ClipboardManager` from the host Activity context
//! (`context.getSystemService(Context.CLIPBOARD_SERVICE)`), using the
//! `JavaVM` + Activity exposed by `ndk_context` — the same per-op attach
//! pattern the `storage`/`net` SDKs use. ClipboardManager ops are cheap,
//! so attaching per call is fine.
//!
//! - write: `clipboard.setPrimaryClip(ClipData.newPlainText(label, text))`
//! - read:  `clipboard.getPrimaryClip().getItemAt(0).coerceToText(context)`
//!   — `getPrimaryClip()` is null when the clipboard is empty, which we
//!   map to `None`.
//!
//! Compile-checked only ⚠️ — not verified on an Android device/emulator.

use jni::objects::{JObject, JValue};
use jni::JavaVM;

use crate::ClipboardError;

/// Label attached to the `ClipData`. Android requires one; it's a
/// developer-facing description, not shown to the user as the content.
const CLIP_LABEL: &str = "idealyst";

fn java_vm() -> Result<JavaVM, ClipboardError> {
    let ctx = ndk_context::android_context();
    let vm_ptr = ctx.vm() as *mut jni::sys::JavaVM;
    unsafe { JavaVM::from_raw(vm_ptr) }
        .map_err(|e| ClipboardError::Backend(format!("invalid JavaVM pointer: {e}")))
}

fn map_jni(e: jni::errors::Error) -> ClipboardError {
    ClipboardError::Backend(format!("JNI: {e}"))
}

/// `context.getSystemService(Context.CLIPBOARD_SERVICE)` →
/// `android.content.ClipboardManager`.
fn clipboard_manager<'a>(
    env: &mut jni::JNIEnv<'a>,
    activity: &JObject<'a>,
) -> Result<JObject<'a>, ClipboardError> {
    // Context.CLIPBOARD_SERVICE is the constant string "clipboard".
    let service = env.new_string("clipboard").map_err(map_jni)?;
    env.call_method(
        activity,
        "getSystemService",
        "(Ljava/lang/String;)Ljava/lang/Object;",
        &[JValue::Object(&JObject::from(service))],
    )
    .map_err(map_jni)?
    .l()
    .map_err(map_jni)
}

pub(crate) async fn set_text(text: &str) -> Result<(), ClipboardError> {
    let text = text.to_string();
    let vm = java_vm()?;
    let mut env = vm.attach_current_thread().map_err(map_jni)?;
    let ctx = ndk_context::android_context();
    let activity = unsafe { JObject::from_raw(ctx.context().cast()) };

    let manager = clipboard_manager(&mut env, &activity)?;

    // ClipData.newPlainText(CharSequence label, CharSequence text)
    let label_j = env.new_string(CLIP_LABEL).map_err(map_jni)?;
    let text_j = env.new_string(&text).map_err(map_jni)?;
    let clip = env
        .call_static_method(
            "android/content/ClipData",
            "newPlainText",
            "(Ljava/lang/CharSequence;Ljava/lang/CharSequence;)Landroid/content/ClipData;",
            &[
                JValue::Object(&JObject::from(label_j)),
                JValue::Object(&JObject::from(text_j)),
            ],
        )
        .map_err(map_jni)?
        .l()
        .map_err(map_jni)?;

    env.call_method(
        &manager,
        "setPrimaryClip",
        "(Landroid/content/ClipData;)V",
        &[JValue::Object(&clip)],
    )
    .map_err(map_jni)?;
    Ok(())
}

pub(crate) async fn text() -> Result<Option<String>, ClipboardError> {
    let vm = java_vm()?;
    let mut env = vm.attach_current_thread().map_err(map_jni)?;
    let ctx = ndk_context::android_context();
    let activity = unsafe { JObject::from_raw(ctx.context().cast()) };

    let manager = clipboard_manager(&mut env, &activity)?;

    // getPrimaryClip() → ClipData, or null when the clipboard is empty.
    let clip = env
        .call_method(
            &manager,
            "getPrimaryClip",
            "()Landroid/content/ClipData;",
            &[],
        )
        .map_err(map_jni)?
        .l()
        .map_err(map_jni)?;
    if clip.is_null() {
        return Ok(None);
    }

    // getItemCount() == 0 also means no text available.
    let count = env
        .call_method(&clip, "getItemCount", "()I", &[])
        .map_err(map_jni)?
        .i()
        .map_err(map_jni)?;
    if count <= 0 {
        return Ok(None);
    }

    // getItemAt(0).coerceToText(context) → CharSequence (never null for a
    // present item; may be an empty string).
    let item = env
        .call_method(
            &clip,
            "getItemAt",
            "(I)Landroid/content/ClipData$Item;",
            &[JValue::Int(0)],
        )
        .map_err(map_jni)?
        .l()
        .map_err(map_jni)?;

    let text_obj = env
        .call_method(
            &item,
            "coerceToText",
            "(Landroid/content/Context;)Ljava/lang/CharSequence;",
            &[JValue::Object(&activity)],
        )
        .map_err(map_jni)?
        .l()
        .map_err(map_jni)?;
    if text_obj.is_null() {
        return Ok(None);
    }

    // CharSequence.toString() → String.
    let string_obj = env
        .call_method(&text_obj, "toString", "()Ljava/lang/String;", &[])
        .map_err(map_jni)?
        .l()
        .map_err(map_jni)?;
    let s: String = env.get_string(&string_obj.into()).map_err(map_jni)?.into();
    if s.is_empty() {
        Ok(None)
    } else {
        Ok(Some(s))
    }
}
