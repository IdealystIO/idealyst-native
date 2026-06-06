//! App-level keyboard handling for the Android backend.
//!
//! Unlike the per-`EditText` `on_key_down` (focus-scoped, `RustKeyListener`),
//! this attaches a single `View.OnKeyListener` (`RustGlobalKeyListener`) to the
//! Activity root so it sees every hardware key press regardless of focus, and
//! routes it through the framework's [`KeyDownHandler`]. Drives
//! [`AndroidBackend::set_app_key_handler`](super::AndroidBackend).
//!
//! Hardware-keyboard only (an on-screen IME doesn't deliver these key events) —
//! the mobile in-app gesture path (e.g. the whiteboard's two-finger swipe)
//! covers touch devices.

use crate::imp::callbacks::{leak, KeyDownCallback};
use crate::imp::{with_env, AndroidBackend};
use jni::objects::{JObject, JValue};
use jni::sys::jlong;
use runtime_core::primitives::key::KeyDownHandler;

/// Install (or, with `None`, remove) the app-level key listener on the root view.
/// Replacing first detaches + frees the previous handler.
pub(crate) fn set_app_key_handler(backend: &mut AndroidBackend, handler: Option<KeyDownHandler>) {
    // Detach + free any previous handler.
    if let Some(prev) = backend.app_key_ptr.take() {
        let root = backend.root.clone();
        with_env(|env| {
            let _ = env.call_method(
                &root,
                "setOnKeyListener",
                "(Landroid/view/View$OnKeyListener;)V",
                &[JValue::Object(&JObject::null())],
            );
        });
        // SAFETY: `prev` came from `leak(KeyDownCallback(..))` below and the
        // listener holding it has just been detached, so no JVM thread can still
        // call into it.
        unsafe {
            drop(Box::from_raw(prev as *mut KeyDownCallback));
        }
    }

    let Some(handler) = handler else {
        return;
    };

    let ptr: jlong = leak(KeyDownCallback(handler));
    let root = backend.root.clone();
    let attached = with_env(|env| {
        // `find_class` for a class that isn't in the app (e.g. the CLI wasn't
        // reinstalled after this feature was added, so the staged Kotlin runtime
        // lacks `RustGlobalKeyListener`) throws a JNI exception AND returns Err.
        // The JVM leaves the exception PENDING — if we don't clear it, the very
        // next JNI call in the build crashes the app on boot. So every failure
        // path clears it and fails closed (the app-level keyboard is a best-
        // effort, hardware-keyboard-only nicety; it must never break boot). This
        // mirrors the `exception_clear` discipline in `a11y.rs` / `mod.rs`.
        let class = match env.find_class("io/idealyst/runtime/RustGlobalKeyListener") {
            Ok(c) => c,
            Err(_) => {
                let _ = env.exception_clear();
                return false;
            }
        };
        let listener = match env.new_object(&class, "(J)V", &[JValue::Long(ptr)]) {
            Ok(o) => o,
            Err(_) => {
                let _ = env.exception_clear();
                return false;
            }
        };
        let _ = env.call_method(
            &root,
            "setOnKeyListener",
            "(Landroid/view/View$OnKeyListener;)V",
            &[JValue::Object(&listener)],
        );
        // A `View.OnKeyListener` only fires while the view holds focus; make the
        // root focusable (in touch mode) and grab focus so app-level keys land
        // here when no text input is focused. A focused EditText still wins its
        // own keys (it has focus), so this composes rather than steals.
        let _ = env.call_method(
            &root,
            "setFocusableInTouchMode",
            "(Z)V",
            &[JValue::Bool(jni::sys::JNI_TRUE)],
        );
        let _ = env.call_method(&root, "requestFocus", "()Z", &[]);
        // Defensive: clear any exception a call_method left pending so it can't
        // surface on an unrelated later JNI call.
        let _ = env.exception_clear();
        true
    });
    if attached {
        backend.app_key_ptr = Some(ptr);
    } else {
        // Never attached — free the leaked box instead of orphaning it.
        unsafe {
            drop(Box::from_raw(ptr as *mut KeyDownCallback));
        }
    }
}
