//! App-level pointer source for the Android backend — the pointer
//! counterpart of [`keyboard`](super::keyboard). Drives
//! [`AndroidBackend::set_pointer_sink`](super::AndroidBackend) /
//! `set_pointer_lock` / `set_cursor_hidden` through one Kotlin
//! `RustGlobalPointerListener` per installed sink.
//!
//! # Why a `Window.Callback` wrapper (the app-wide hook)
//!
//! The sink must see mouse input anywhere in the app, regardless of which
//! view is under the pointer — including over a focused text input. A
//! listener on the root view does NOT get that: `ViewGroup.dispatchTouchEvent`
//! offers events to the children first, and a child that consumes them (a
//! button, a scroll view, an `EditText`) hides them from the root's
//! `OnTouchListener`/`OnGenericMotionListener`. The backend doesn't own a
//! custom root `ViewGroup` it could override `dispatch*` on either — the root
//! is a plain `FrameLayout` the CLI-generated `MainActivity` creates. The one
//! place the backend can reach that sees every input event of the window
//! BEFORE any view does is the window's `Window.Callback` (the Activity
//! itself, called by `DecorView` for `dispatchTouchEvent` /
//! `dispatchGenericMotionEvent`). The listener installs a delegating wrapper
//! (`RustPointerWindowCallback`) there, inspects mouse/stylus/touchpad events
//! (finger touches pass straight through — `app_pointer_policy::is_pointer_device`),
//! and returns `true` (consumed) only when the sink answered `PreventDefault`
//! for a `Down` or `Wheel` (plus the rest of a swallowed press stream). The
//! same wrapper receives `onWindowFocusChanged` (→ `focus_lost`) and
//! `onPointerCaptureChanged` (→ `lock_changed`). Detach restores the original
//! callback, or — if something wrapped the window after us — leaves our
//! wrapper in place as an inert pass-through.
//!
//! Overlays/portals are views in the same window (`primitives::overlay`), so
//! they are covered. A `Dialog` an SDK opens is its own window and is not.
//!
//! # Pointer lock = pointer capture (API 26+)
//!
//! `View.requestPointerCapture()` hides the cursor, keeps it from moving and
//! delivers raw relative motion (`SOURCE_MOUSE_RELATIVE`) to the FOCUSED
//! view's `OnCapturedPointerListener` — `ViewRootImpl` routes captured events
//! down the focus chain, never through `Window.Callback`. So a lock request
//! makes the root focusable, focuses it and sets the listener there. The
//! grant/loss arrives asynchronously as `onPointerCaptureChanged`; the
//! system drops capture when the window loses focus, and the listener
//! releases it itself when focus moves off the root (captured events would
//! otherwise go to the newly focused view and be lost) — both surface as
//! `lock_changed(false)`. A request while the window lacks focus is ignored
//! by the system: nothing is reported. Below API 26 there is no capture:
//! lock requests do nothing and never report `true`.
//!
//! # Cursor hiding (API 24+)
//!
//! Pointer icons resolve top-down through `onResolvePointerIcon`, children
//! first, so setting `TYPE_NULL` on the root would lose to a text input's
//! I-beam. Instead a non-clickable, non-focusable, accessibility-hidden view
//! with `PointerIcon.TYPE_NULL` is added as the TOP child of the `DecorView`:
//! it wins icon resolution everywhere in the window, while touches, hover and
//! generic motion fall through it (a plain non-clickable `View` handles none
//! of them). Removed on `set_cursor_hidden(false)` and on teardown.
//!
//! The whole source needs API 24 (`ACTION_BUTTON_PRESS`/`getActionButton`
//! are 23, `PointerIcon` and the `Window.Callback` keyboard-shortcut types
//! 24); below that `attach` refuses and no sink is installed.

use crate::app_pointer_policy::PointerTracker;
use crate::imp::callbacks::{leak, PointerSinkCallback, PointerSourceState};
use crate::imp::{with_env, AndroidBackend};
use jni::objects::{GlobalRef, JValue};
use jni::sys::jlong;
use runtime_shared::pointer_input::PointerSink;
use std::cell::RefCell;
use std::rc::Rc;

/// The installed native source: the leaked sink box and the Kotlin listener
/// holding a copy of its pointer (needed to `detach()` it).
pub(crate) struct AppPointerSource {
    ptr: jlong,
    listener: GlobalRef,
}

/// Install (or, with `None`, remove) the app-level pointer source. Replacing
/// first detaches + frees the previous one. Same re-entrancy argument as
/// [`super::keyboard::set_keyboard_sink`]: the trampolines clone the source
/// out of the box before calling the sink, and `detach()` zeroes the Kotlin
/// pointer before the box is freed.
pub(crate) fn set_pointer_sink(backend: &mut AndroidBackend, sink: Option<PointerSink>) {
    if let Some(prev) = backend.app_pointer.take() {
        with_env(|env| {
            // `detach()` releases pointer capture, removes the cursor
            // shield, restores the window callback and zeroes the pointer.
            let _ = env.call_method(prev.listener.as_obj(), "detach", "()V", &[]);
            let _ = env.exception_clear();
        });
        // SAFETY: `prev.ptr` came from `leak(PointerSinkCallback(..))` below
        // and its only JVM holder zeroed its copy just now, on this (the UI)
        // thread — the only thread that dispatches it.
        unsafe {
            drop(Box::from_raw(prev.ptr as *mut PointerSinkCallback));
        }
    }

    let Some(sink) = sink else {
        return;
    };

    let ptr: jlong = leak(PointerSinkCallback(Rc::new(PointerSourceState {
        sink,
        tracker: RefCell::new(PointerTracker::default()),
    })));
    let root = backend.root.clone();
    let listener = with_env(|env| {
        // Fail closed and clear any pending JNI exception on every error
        // path (see `keyboard::set_keyboard_sink` for why).
        let class = match env.find_class("io/idealyst/runtime/RustGlobalPointerListener") {
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
        // `attach` returns false below API 24 or without an Activity window.
        let attached = env
            .call_method(&listener, "attach", "(Landroid/view/View;)Z", &[JValue::Object(root.as_obj())])
            .and_then(|v| v.z())
            .unwrap_or(false);
        let global = env.new_global_ref(&listener).ok();
        let _ = env.exception_clear();
        match (attached, global) {
            (true, Some(g)) => Some(g),
            _ => {
                let _ = env.call_method(&listener, "detach", "()V", &[]);
                let _ = env.exception_clear();
                None
            }
        }
    });
    match listener {
        Some(listener) => backend.app_pointer = Some(AppPointerSource { ptr, listener }),
        // Never attached — free the box instead of orphaning it.
        None => unsafe {
            drop(Box::from_raw(ptr as *mut PointerSinkCallback));
        },
    }
}

/// Ask for / release pointer capture. The result comes back through
/// `nativePointerLockChanged`. No-op without an installed source.
pub(crate) fn set_pointer_lock(backend: &mut AndroidBackend, locked: bool) {
    call_bool(backend, "setLock", locked);
}

/// Show/hide the cursor over the app (the `DecorView` shield).
pub(crate) fn set_cursor_hidden(backend: &mut AndroidBackend, hidden: bool) {
    call_bool(backend, "setCursorHidden", hidden);
}

fn call_bool(backend: &AndroidBackend, method: &str, value: bool) {
    let Some(src) = backend.app_pointer.as_ref() else {
        return;
    };
    with_env(|env| {
        let _ = env.call_method(src.listener.as_obj(), method, "(Z)V", &[JValue::Bool(value as u8)]);
        let _ = env.exception_clear();
    });
}
