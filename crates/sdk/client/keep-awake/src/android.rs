//! Android: `RustKeepAwake` puts `FLAG_KEEP_SCREEN_ON` on the app's Activity
//! windows. The flag needs no permission, also stops the dimming that comes
//! before the timeout, and counts only while its window is on screen.

use jni::objects::{GlobalRef, JClass, JObject, JValue};
use jni::{JNIEnv, JavaVM};
use runtime_shared::logging::{log, LogLevel};
use std::sync::OnceLock;

const HELPER: &str = "io.idealyst.keepawake.RustKeepAwake";

pub(crate) fn set(on: bool) {
    if let Err(e) = try_set(on) {
        log(LogLevel::Error, &format!("keep-awake: {e}"));
    }
}

fn try_set(on: bool) -> Result<(), String> {
    let host = ndk_context::android_context();
    if host.vm().is_null() || host.context().is_null() {
        return Err("no Android context (the app has not attached yet)".into());
    }
    let vm = unsafe { JavaVM::from_raw(host.vm().cast()) }.map_err(|e| e.to_string())?;
    let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
    // SAFETY: `ndk_context` holds a process-lifetime global ref to the
    // Context; this is a non-owning wrapper that never deletes it.
    let ctx = unsafe { JObject::from_raw(host.context().cast()) };
    let class = helper_class(&mut env, &ctx)?;
    let class: &JClass = class.as_obj().into();
    env.call_static_method(
        class,
        "set",
        "(Landroid/content/Context;Z)V",
        &[JValue::Object(&ctx), JValue::Bool(on.into())],
    )
    .map(|_| ())
    .map_err(|e| {
        // Cleared: a pending exception would crash the next JNI call.
        let _ = env.exception_clear();
        e.to_string()
    })
}

/// `RustKeepAwake`, through the Context's class loader: `find_class` on a
/// thread the JVM did not start cannot see app classes.
fn helper_class(env: &mut JNIEnv<'_>, ctx: &JObject<'_>) -> Result<&'static GlobalRef, String> {
    static CLASS: OnceLock<GlobalRef> = OnceLock::new();
    if let Some(c) = CLASS.get() {
        return Ok(c);
    }
    let load = |env: &mut JNIEnv<'_>| -> jni::errors::Result<GlobalRef> {
        let loader = env.call_method(ctx, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])?.l()?;
        let name = env.new_string(HELPER)?;
        let class = env
            .call_method(&loader, "loadClass", "(Ljava/lang/String;)Ljava/lang/Class;", &[JValue::Object(&name)])?
            .l()?;
        env.new_global_ref(class)
    };
    let class = load(env).map_err(|e| {
        let _ = env.exception_clear();
        format!("{HELPER} is not in the APK ({e}); was the app built by the idealyst CLI?")
    })?;
    Ok(CLASS.get_or_init(|| class))
}

pub(crate) fn is_supported() -> bool {
    true
}
