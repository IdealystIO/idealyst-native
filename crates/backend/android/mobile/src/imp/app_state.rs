//! App-situation observers for the Android backend: window focus,
//! Activity started/stopped, the "Remove animations" setting and the
//! connected pointing devices, reported into
//! `runtime_shared::app_state` (author API: `app_focused()`,
//! `app_visible()`, `reduce_motion()`, `primary_pointer()`).
//!
//! Installed once from `AndroidBackend::new` by constructing a Kotlin
//! `RustAppStateObserver` and calling `attach(root)`. The observer reports
//! through static-shaped trampolines (no Rust pointer — it holds no Rust
//! state) into [`crate::newcore::report_app_state`], which reports and
//! schedules the backend's flush. The JVM keeps the observer alive through
//! the listeners it registers; it unregisters all of them itself when its
//! Activity is destroyed, so nothing reports into a torn-down app.
//!
//! | Fact | Source | At boot |
//! |---|---|---|
//! | `Focused` | root `ViewTreeObserver.OnWindowFocusChangeListener` | only if the root is already attached (`hasWindowFocus()`); otherwise the first focus callback |
//! | `Visible` | `Application.ActivityLifecycleCallbacks` started/stopped for the root's Activity | not reported (boot runs in `onCreate`, right before `onStart`; the default `true` is what the user is about to see) |
//! | `ReduceMotion` | `Settings.Global.ANIMATOR_DURATION_SCALE == 0` + `ContentObserver` | yes |
//! | `PrimaryPointer` | `InputManager` devices (`SOURCE_MOUSE`, non-virtual) + `InputDeviceListener`; `FEATURE_TOUCHSCREEN` | yes |
//!
//! `UserActivated` is web-only (native apps may play audio at any time) and
//! is not reported.

use crate::imp::{with_env, AndroidBackend};
use jni::objects::JValue;

/// Construct + attach the observer. Best-effort: a missing class (a stale
/// staged Kotlin runtime) clears the JNI exception and leaves the defaults.
pub(crate) fn install(backend: &AndroidBackend) {
    let root = backend.root.clone();
    with_env(|env| {
        let class = match env.find_class("io/idealyst/runtime/RustAppStateObserver") {
            Ok(c) => c,
            Err(_) => {
                let _ = env.exception_clear();
                return;
            }
        };
        let observer = match env.new_object(&class, "()V", &[]) {
            Ok(o) => o,
            Err(_) => {
                let _ = env.exception_clear();
                return;
            }
        };
        let _ = env.call_method(&observer, "attach", "(Landroid/view/View;)V", &[JValue::Object(root.as_obj())]);
        let _ = env.exception_clear();
    });
}
