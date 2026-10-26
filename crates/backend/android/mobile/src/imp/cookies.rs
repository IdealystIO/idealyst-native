//! The process cookie jar for the Android backend.
//!
//! `net`'s Android transport is a bare `HttpURLConnection`, which keeps
//! cookies only when a default `java.net.CookieHandler` is installed.
//! Installed once, unconditionally, from `AndroidBackend::new` — before
//! the app mounts, so its first request already has the jar — by calling
//! the Kotlin `RustCookies.install(context)`. Read the Kotlin class docs
//! for what it stores and what it persists.

use crate::imp::{with_env, AndroidBackend};
use jni::objects::JValue;

/// Best-effort: a missing class (a stale staged Kotlin runtime) clears the
/// JNI exception and leaves the app without a cookie jar instead of
/// breaking boot.
pub(crate) fn install(backend: &AndroidBackend) {
    let context = backend.context.clone();
    with_env(|env| {
        let class = match env.find_class("io/idealyst/runtime/RustCookies") {
            Ok(c) => c,
            Err(_) => {
                let _ = env.exception_clear();
                return;
            }
        };
        let _ = env.call_static_method(
            &class,
            "install",
            "(Landroid/content/Context;)V",
            &[JValue::Object(context.as_obj())],
        );
        let _ = env.exception_clear();
    });
}
