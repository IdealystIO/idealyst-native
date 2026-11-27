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
//! Activity is destroyed, so nothing reports into a torn-down app. Only the
//! newest observer reports: an Activity re-created before the old one
//! stopped would otherwise get the old one's late "stopped" / "focus lost"
//! and read as hidden while on screen.
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
//!
//! **Splash dismissed** (`runtime_shared::app_state::report_splash_dismissed`):
//! `attach` reports `false` (boot runs in `onCreate`, before the root build)
//! and a one-shot `ViewTreeObserver.OnDrawListener` on the root reports
//! `true` from a message posted at the front of the looper queue by the
//! root's first `onDraw` — once that draw pass has returned and the frame
//! went to the RenderThread. That first frame is what the system splash
//! waits for on every API level: the API 31+ `SplashScreen` is removed when
//! the app draws its first frame (the app need not own its exit), and
//! below 31 the theme's `windowBackground` is the window's background
//! until the content draws over it. The listener is added to the root's
//! live `ViewTreeObserver` once the root is attached: before API 26 a
//! detached view's floating observer does not carry `OnDrawListener`s over
//! when it merges into the window's. Only a working Kotlin side reports
//! `false`, so a stale staged runtime keeps the `true` default instead of
//! hiding the app's loading screen forever. The JVM half has no unit-test
//! harness here; `newcore`'s `report_splash_dismissed_commits_through_flush`
//! covers the Rust half, and the first-draw moment needs a device run.

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
