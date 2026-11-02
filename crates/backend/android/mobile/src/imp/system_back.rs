//! System Back → the app's `on_back` handlers (`runtime_shared::system_back`).
//!
//! [`install`] constructs the Kotlin `RustSystemBack` for the backend's
//! Activity from `AndroidBackend::new`, i.e. at attach and before the app
//! tree mounts — so its `OnBackPressedCallback` is registered before any
//! stack presenter's, and the dispatcher (newest first) lets navigators
//! take Back ahead of it. Its callback is off until
//! [`AndroidBackend::set_system_back_wanted_impl`] turns it on (a handler
//! was registered); a Back it receives comes back through the
//! `nativeDispatch` export (`jni_exports.rs`). [`AndroidBackend::dispose_system_back`]
//! removes the callback at `newcore::stop`: the dispatcher must not keep
//! routing Back into an app that is gone.
//!
//! Additive Kotlin: a runtime staged before `RustSystemBack` existed has no
//! such class, and an older one may lack a method. Both clear the JNI
//! exception, log once, and leave Back to the platform rather than failing
//! the boot.

use crate::imp::{with_env, AndroidBackend};
use jni::objects::{GlobalRef, JValue};

/// Construct + register the Kotlin side. `None` when the class is missing.
pub(crate) fn install(context: &GlobalRef) -> Option<GlobalRef> {
    with_env(|env| {
        let class = match env.find_class("io/idealyst/runtime/RustSystemBack") {
            Ok(c) => c,
            Err(_) => {
                let _ = env.exception_clear();
                log::warn!(
                    "RustSystemBack unavailable; on_back handlers will not run. \
                     Rebuild with the current idealyst CLI to pick up the runtime change."
                );
                return None;
            }
        };
        let obj = match env.new_object(&class, "(Landroid/content/Context;)V", &[JValue::Object(context.as_obj())]) {
            Ok(o) => o,
            Err(_) => {
                let _ = env.exception_clear();
                return None;
            }
        };
        env.new_global_ref(obj).ok()
    })
}

fn call_soft(obj: &GlobalRef, method: &str, sig: &str, args: &[JValue]) {
    with_env(|env| {
        if env.call_method(obj.as_obj(), method, sig, args).is_err() {
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_clear();
            }
            log::warn!("RustSystemBack.{method} failed; system Back is left to the platform");
        }
    });
}

impl AndroidBackend {
    /// `AppEnvOps::set_system_back_wanted`: enable the Kotlin callback
    /// while a handler is registered.
    pub(crate) fn set_system_back_wanted_impl(&mut self, wanted: bool) {
        if let Some(obj) = &self.system_back {
            call_soft(obj, "setWanted", "(Z)V", &[JValue::Bool(wanted.into())]);
        }
    }

    /// Remove the callback from the Activity's dispatcher (`newcore::stop`).
    /// Idempotent.
    pub(crate) fn dispose_system_back(&mut self) {
        if let Some(obj) = self.system_back.take() {
            call_soft(&obj, "dispose", "()V", &[]);
        }
    }
}
