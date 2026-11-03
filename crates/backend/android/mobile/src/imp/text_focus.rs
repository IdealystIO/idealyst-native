//! A text field's focus listener: `RustFocusListener` (Kotlin) on every
//! `text_input` / `text_area` `EditText`, holding a leaked
//! [`TextFocusSlots`] the framework fills — the author's `on_focus`
//! (`set_text_input_focus_handler`) and cancelable `on_blur`. Dispatch rules
//! are in `text_focus_policy` (host-tested).
//!
//! The listener goes on at create, so `on_focus` installs into a slot rather
//! than replacing a listener, and the style states' `RustStateListener`
//! (`attach_states_impl`) chains behind it via [`set_other_focus_listener`]
//! instead of taking the view's one focus-listener slot.
//!
//! A staged Kotlin runtime without `RustFocusListener` (CLI not reinstalled)
//! throws `NoClassDefFoundError` from `find_class`; every lookup clears the
//! pending exception and degrades to the old behaviour (no focus
//! callbacks), because a pending exception crashes the next, unrelated JNI
//! call.

use jni::objects::{GlobalRef, JObject, JValue};
use jni::sys::jlong;
use jni::JNIEnv;
use std::rc::Rc;

use crate::imp::callbacks::leak;
use crate::imp::with_env;
use crate::text_focus_policy::TextFocusSlots;
use runtime_shared::primitives::text_input::BlurHandler;

const LISTENER_CLASS: &str = "io/idealyst/runtime/RustFocusListener";

/// Put a `RustFocusListener` on a freshly created field, carrying its
/// `on_blur` (a `text_area` has none).
pub(crate) fn install(env: &mut JNIEnv, field: &JObject, on_blur: Option<BlurHandler>) {
    let slots = TextFocusSlots::default();
    *slots.on_blur.borrow_mut() = on_blur;
    let ptr: jlong = leak(slots);
    let installed = env.call_static_method(
        LISTENER_CLASS,
        "install",
        "(Landroid/view/View;J)V",
        &[JValue::Object(field), JValue::Long(ptr)],
    );
    if installed.is_err() {
        let _ = env.exception_clear();
    }
}

/// The field's slots, when its focus listener is ours.
fn slots_of<'a>(env: &mut JNIEnv, field: &JObject) -> Option<&'a TextFocusSlots> {
    let listener = env
        .call_method(field, "getOnFocusChangeListener", "()Landroid/view/View$OnFocusChangeListener;", &[])
        .and_then(|v| v.l());
    let Ok(listener) = listener else {
        let _ = env.exception_clear();
        return None;
    };
    if listener.is_null() {
        return None;
    }
    let Ok(class) = env.find_class(LISTENER_CLASS) else {
        let _ = env.exception_clear();
        return None;
    };
    if !env.is_instance_of(&listener, &class).unwrap_or(false) {
        return None;
    }
    let ptr = env.call_method(&listener, "getNativePtr", "()J", &[]).and_then(|v| v.j());
    let Ok(ptr) = ptr else {
        let _ = env.exception_clear();
        return None;
    };
    // SAFETY: `install` leaked a `Box<TextFocusSlots>` at this address and
    // never frees it (late focus events can reach a detached view).
    (ptr != 0).then(|| unsafe { &*(ptr as *const TextFocusSlots) })
}

/// `TextInputOps::set_text_input_focus_handler`.
pub(crate) fn set_focus_handler(field: &GlobalRef, handler: Rc<dyn Fn(bool)>) {
    with_env(|env| {
        if let Some(slots) = slots_of(env, field.as_obj()) {
            *slots.on_focus.borrow_mut() = Some(handler);
        }
    });
}

/// Install a non-field focus listener (the style states') on `view`
/// without displacing a field's `RustFocusListener`.
pub(crate) fn set_other_focus_listener(env: &mut JNIEnv, view: &JObject, listener: &JObject) {
    let chained = env.call_static_method(
        LISTENER_CLASS,
        "chainOther",
        "(Landroid/view/View;Landroid/view/View$OnFocusChangeListener;)V",
        &[JValue::Object(view), JValue::Object(listener)],
    );
    if chained.is_err() {
        let _ = env.exception_clear();
        let _ = env.call_method(
            view,
            "setOnFocusChangeListener",
            "(Landroid/view/View$OnFocusChangeListener;)V",
            &[JValue::Object(listener)],
        );
    }
}

/// `IntrospectionOps::is_focused`: `View.isFocused()`, the view's own
/// focus flag (not window focus).
pub(crate) fn is_focused(node: &GlobalRef) -> Option<bool> {
    with_env(|env| match env.call_method(node.as_obj(), "isFocused", "()Z", &[]).and_then(|v| v.z()) {
        Ok(f) => Some(f),
        Err(_) => {
            let _ = env.exception_clear();
            None
        }
    })
}
