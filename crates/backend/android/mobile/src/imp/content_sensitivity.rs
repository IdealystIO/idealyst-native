//! Whether a text field may show in another app's screen capture
//! (`View.setContentSensitivity`, API 35).
//!
//! Android 15+ treats a window as `FLAG_SECURE` for the length of a
//! MediaProjection session while it holds a sensitive view. `TextView`
//! counts itself sensitive for any password input type: `getAutofillHints`
//! adds `passwordAuto`, which is on the sensitive-hints list. The app turns
//! that off for all its fields with `content_sensitivity = "not_sensitive"`
//! in `[package.metadata.idealyst.app.android]`, or per field.
//!
//! Set as a field is made, before it is in a window: the protection is per
//! window and is dropped only after the window's next draw.

use std::sync::OnceLock;

use jni::objects::{GlobalRef, JObject, JString, JValue};
use jni::JNIEnv;
use runtime_shared::primitives::text_input::ContentSensitivity;

use super::style::android_sdk_int;
use super::with_env;

/// The API level `setContentSensitivity` arrived in.
const API_LEVEL: i32 = 35;
/// `View.CONTENT_SENSITIVITY_*`.
const AUTO: i32 = 0;
const SENSITIVE: i32 = 1;
const NOT_SENSITIVE: i32 = 2;
/// The `<meta-data>` the CLI writes; `build-ios`'s
/// `CONTENT_SENSITIVITY_META_DATA`.
const META_DATA: &str = "io.idealyst.CONTENT_SENSITIVITY";
/// `PackageManager.GET_META_DATA`.
const GET_META_DATA: i32 = 128;

/// A text input was just made: give it the app's setting.
pub(crate) fn created(field: &GlobalRef) {
    with_env(|env| {
        if android_sdk_int(env) >= API_LEVEL {
            let app = app_wide(env, field.as_obj());
            if app != ContentSensitivity::Auto {
                set(env, field, app);
            }
        }
    });
}

/// The field's own setting.
pub(crate) fn apply(field: &GlobalRef, sensitivity: ContentSensitivity) {
    with_env(|env| {
        if android_sdk_int(env) >= API_LEVEL {
            let app = app_wide(env, field.as_obj());
            set(env, field, sensitivity.or(app));
        }
    });
}

fn set(env: &mut JNIEnv<'_>, field: &GlobalRef, sensitivity: ContentSensitivity) {
    let mode = match sensitivity {
        ContentSensitivity::Auto => AUTO,
        ContentSensitivity::Sensitive => SENSITIVE,
        ContentSensitivity::NotSensitive => NOT_SENSITIVE,
    };
    if env.call_method(field.as_obj(), "setContentSensitivity", "(I)V", &[JValue::Int(mode)]).is_err() {
        let _ = env.exception_clear();
    }
}

/// The manifest's setting, read once: it is fixed for the installed app.
fn app_wide(env: &mut JNIEnv<'_>, view: &JObject<'_>) -> ContentSensitivity {
    static APP: OnceLock<ContentSensitivity> = OnceLock::new();
    *APP.get_or_init(|| match read_meta_data(env, view) {
        Ok(Some(v)) if v == "not_sensitive" => ContentSensitivity::NotSensitive,
        Ok(_) => ContentSensitivity::Auto,
        Err(_) => {
            let _ = env.exception_clear();
            ContentSensitivity::Auto
        }
    })
}

fn read_meta_data(env: &mut JNIEnv<'_>, view: &JObject<'_>) -> jni::errors::Result<Option<String>> {
    let ctx = env.call_method(view, "getContext", "()Landroid/content/Context;", &[])?.l()?;
    let pm = env.call_method(&ctx, "getPackageManager", "()Landroid/content/pm/PackageManager;", &[])?.l()?;
    let pkg = env.call_method(&ctx, "getPackageName", "()Ljava/lang/String;", &[])?.l()?;
    let info = env
        .call_method(
            &pm,
            "getApplicationInfo",
            "(Ljava/lang/String;I)Landroid/content/pm/ApplicationInfo;",
            &[JValue::Object(&pkg), JValue::Int(GET_META_DATA)],
        )?
        .l()?;
    let bundle = env.get_field(&info, "metaData", "Landroid/os/Bundle;")?.l()?;
    if bundle.is_null() {
        return Ok(None);
    }
    let key = env.new_string(META_DATA)?;
    let value = env
        .call_method(&bundle, "getString", "(Ljava/lang/String;)Ljava/lang/String;", &[JValue::Object(&key)])?
        .l()?;
    if value.is_null() {
        return Ok(None);
    }
    Ok(Some(env.get_string(&JString::from(value))?.into()))
}
