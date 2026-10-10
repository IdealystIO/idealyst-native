//! App-level keyboard source for the Android backend.
//!
//! Unlike the per-`EditText` `on_key_down` (focus-scoped, `RustKeyListener`),
//! this attaches one `RustGlobalKeyListener` to the Activity root and feeds
//! the framework's [`KeyboardSink`]: every hardware key DOWN and UP the root
//! receives (modifier keys included — Android delivers Shift/Ctrl/Alt/Meta as
//! ordinary key events), with the auto-repeat flag and the physical `code`
//! (see `crate::app_key_policy`). Drives
//! [`AndroidBackend::set_keyboard_sink`](super::AndroidBackend).
//!
//! # What the root sees — and when it stops
//!
//! A `View.OnKeyListener` only fires while its view holds focus, so the root
//! is made focusable-in-touch-mode and takes focus on install (unless a view
//! already holds focus — a focused text input is never robbed). Keys then land
//! here whenever no other view holds focus. When something else takes focus —
//! a focused text input (its keys go to its own `on_key_down`, by design: an
//! app-level game control must not eat typing), a modal overlay, or a
//! focusable child reached by D-pad/Tab navigation — the root stops
//! receiving keys, including the RELEASES of keys still held. The listener
//! therefore reports focus loss (focus moving off the root, or the window
//! losing focus) through `nativeKeyFocusLost` → [`KeyboardSink::focus_lost`],
//! and the dispatcher synthesizes the missing key-ups. The root does not
//! re-take focus by itself afterwards; keys resume once focus returns to it.
//!
//! Hardware-keyboard only: an on-screen IME doesn't deliver key events to a
//! view `OnKeyListener`.

use crate::imp::callbacks::{leak, KeyboardSinkCallback};
use crate::imp::{with_env, AndroidBackend};
use jni::objects::{GlobalRef, JValue};
use jni::sys::jlong;
use runtime_shared::primitives::key::KeyboardSink;

/// The installed native source: the leaked sink box and the Kotlin listener
/// object that holds a copy of its pointer (needed to `detach()` it).
pub(crate) struct AppKeySource {
    ptr: jlong,
    listener: GlobalRef,
}

/// Install (or, with `None`, remove) the app-level keyboard source on the
/// root view. Replacing first detaches + frees the previous source.
///
/// Called with `&mut AndroidBackend` from the dispatcher's install path, which
/// can run from INSIDE a key dispatch (a listener that removes the last
/// listener). That is safe: the trampoline cloned the sink out of the box
/// before calling it, and `detach()` zeroes the Kotlin-side pointer before we
/// free the box, so no later JVM callback can reach the freed memory.
pub(crate) fn set_keyboard_sink(backend: &mut AndroidBackend, sink: Option<KeyboardSink>) {
    if let Some(prev) = backend.app_key.take() {
        with_env(|env| {
            // `detach()` removes the key + focus listeners and zeroes the
            // listener's pointer so a callback already queued on the looper
            // becomes a no-op.
            let _ = env.call_method(prev.listener.as_obj(), "detach", "()V", &[]);
            let _ = env.exception_clear();
        });
        // SAFETY: `prev.ptr` came from `leak(KeyboardSinkCallback(..))` below,
        // and the only JVM holder of it (the listener) has just zeroed its
        // copy on this — the UI — thread, the only thread that dispatches it.
        unsafe {
            drop(Box::from_raw(prev.ptr as *mut KeyboardSinkCallback));
        }
    }

    let Some(sink) = sink else {
        return;
    };

    let ptr: jlong = leak(KeyboardSinkCallback(sink));
    let root = backend.root.clone();
    let listener = with_env(|env| {
        // The Kotlin runtime is compiled from this crate's own
        // `runtime/kotlin/` tree (see `tools/run/android/kotlin_runtime.rs`),
        // so the class is normally present. Still, a failed `find_class`
        // throws a JNI exception AND returns Err, and the JVM leaves the
        // exception PENDING — if it isn't cleared, the very next JNI call
        // crashes the app. So every failure path clears it and fails closed:
        // the app-level keyboard must never break boot. Mirrors the
        // `exception_clear` discipline in `a11y.rs` / `mod.rs`.
        let class = match env.find_class("io/idealyst/runtime/RustGlobalKeyListener") {
            Ok(c) => c,
            Err(_) => {
                let _ = env.exception_clear();
                return None;
            }
        };
        let listener = match env.new_object(&class, "(J)V", &[JValue::Long(ptr)]) {
            Ok(o) => o,
            Err(_) => {
                let _ = env.exception_clear();
                return None;
            }
        };
        // `attach` sets the root's `OnKeyListener`, registers the window- and
        // global-focus observers, and makes the root focusable + focused.
        let attached = env
            .call_method(&listener, "attach", "(Landroid/view/View;)V", &[JValue::Object(root.as_obj())])
            .is_ok();
        let global = env.new_global_ref(&listener).ok();
        // Clear any exception a call left pending so it can't surface on an
        // unrelated later JNI call.
        let _ = env.exception_clear();
        match (attached, global) {
            (true, Some(g)) => Some(g),
            _ => {
                // Half-attached: undo so the listener can't call a box we're
                // about to free.
                let _ = env.call_method(&listener, "detach", "()V", &[]);
                let _ = env.exception_clear();
                None
            }
        }
    });
    match listener {
        Some(listener) => backend.app_key = Some(AppKeySource { ptr, listener }),
        // Never attached — free the leaked box instead of orphaning it.
        None => unsafe {
            drop(Box::from_raw(ptr as *mut KeyboardSinkCallback));
        },
    }
}
