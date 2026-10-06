//! Attach-safe `focus()` for the Android text fields (the
//! `TextInputOps::focus` / `TextAreaOps::focus` contract).
//!
//! `requestFocus()` on an `EditText` that is not attached to a window does not
//! take, and the IME has no window to show for. Realize builds a subtree before
//! inserting it, and a modal's content attaches later still, so `focus()` right
//! after mount used to be lost.
//!
//! A focus on a detached field is recorded in the shared [`PendingFocus`] slot
//! and a `RustAttachFocus` (`View.OnAttachStateChangeListener`) is installed on
//! it. On attach the listener removes itself and posts
//! `nativeAttached(view)` to the next main-loop turn — the attach runs inside
//! the backend's `addView`, and focusing there would fire the focus listener
//! (a style re-apply) mid-insert. [`attached`] then focuses the view if it is
//! still the pending one (a later `focus()` elsewhere or a `blur()` cancels).

use std::cell::RefCell;

use jni::objects::{GlobalRef, JObject, JValue};
use jni::JNIEnv;
use runtime_shared::primitives::text_input::{FocusRequest, PendingFocus};

use crate::imp::with_env;

thread_local! {
    static PENDING: RefCell<PendingFocus<GlobalRef>> = const { RefCell::new(PendingFocus::new()) };
}

fn is_attached(env: &mut JNIEnv, view: &JObject) -> bool {
    env.call_method(view, "isAttachedToWindow", "()Z", &[])
        .ok()
        .and_then(|v| v.z().ok())
        .unwrap_or(false)
}

fn same(env: &JNIEnv, a: &JObject, b: &JObject) -> bool {
    env.is_same_object(a, b).unwrap_or(false)
}

/// Focus `node` now if it is attached, else when it attaches. `focus_now`
/// is the backend's requestFocus + show-IME.
pub(crate) fn request_focus(node: &GlobalRef, focus_now: fn(&GlobalRef)) {
    let attached = with_env(|env| is_attached(env, node.as_obj()));
    let req = PENDING.with(|p| p.borrow_mut().request(node.clone(), attached));
    match req {
        FocusRequest::Now(node) => focus_now(&node),
        FocusRequest::Deferred => with_env(|env| {
            let Ok(class) = env.find_class("io/idealyst/runtime/RustAttachFocus") else { return };
            let Ok(listener) = env.new_object(&class, "()V", &[]) else { return };
            let _ = env.call_method(
                node.as_obj(),
                "addOnAttachStateChangeListener",
                "(Landroid/view/View$OnAttachStateChangeListener;)V",
                &[JValue::Object(&listener)],
            );
        }),
    }
}

/// A `blur()` on `node`: drop its pending (pre-attach) focus. The listener
/// still fires once on attach and finds nothing to do.
pub(crate) fn cancel(node: &GlobalRef) {
    with_env(|env| PENDING.with(|p| p.borrow_mut().cancel(|g| same(env, g.as_obj(), node.as_obj()))));
}

/// `RustAttachFocus` reported `view` attached (one main-loop turn later).
pub(crate) fn attached(env: &mut JNIEnv, view: &JObject, focus_now: fn(&GlobalRef)) {
    if !is_attached(env, view) {
        return;
    }
    let ready = PENDING.with(|p| p.borrow_mut().take_attached(|g| same(env, g.as_obj(), view)));
    if let Some(node) = ready {
        focus_now(&node);
    }
}
