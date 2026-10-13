//! App-wide user-activity source for the Android backend — feeds
//! `runtime_shared::user_activity` (author API: `user_activity()`).
//!
//! Installed once, unconditionally, from `AndroidBackend::new` by calling
//! the Kotlin `RustUserActivity.install(root)`, which wraps the Activity
//! window's `Window.Callback` in a pass-through observer. Reports arrive
//! through the static `nativeReport` trampoline (`jni_exports.rs`) into
//! [`crate::newcore::report_user_activity`]. Read the Kotlin class docs for
//! which events report, why the window callback is the hook, how the
//! private-layer window and soft-keyboard typing are covered, and why the
//! wrapper can never consume or reorder an event.
//!
//! The trampoline carries no Rust pointer, so nothing is freed on
//! teardown: a report after the app stopped only bumps the counter.

use crate::imp::{with_env, AndroidBackend};
use jni::objects::JValue;

/// Best-effort: a missing class (a stale staged Kotlin runtime) clears the
/// JNI exception and leaves `user_activity()` at `0` instead of breaking
/// boot.
pub(crate) fn install(backend: &AndroidBackend) {
    let root = backend.root.clone();
    with_env(|env| {
        let class = match env.find_class("io/idealyst/runtime/RustUserActivity") {
            Ok(c) => c,
            Err(_) => {
                let _ = env.exception_clear();
                return;
            }
        };
        let _ = env.call_static_method(
            &class,
            "install",
            "(Landroid/view/View;)V",
            &[JValue::Object(root.as_obj())],
        );
        let _ = env.exception_clear();
    });
}
