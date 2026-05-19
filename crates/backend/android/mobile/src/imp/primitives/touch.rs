//! Raw touch event delivery for the Android backend.
//!
//! Installs a `RustTouchListener` (a Kotlin
//! `View.OnTouchListener`) on the given View, with a leaked
//! `TouchCallback` carrying the framework's [`TouchHandler`].
//! Each `MotionEvent` dispatch trampolines into Rust via
//! `nativeInvokeTouch`; see `jni_exports::Java_..._nativeInvokeTouch`.
//!
//! Claim protocol (`Backend::claim_touch`) calls
//! `requestDisallowInterceptTouchEvent(true)` on the View's parent
//! — Android's canonical "ancestors keep your hands off this
//! gesture" flag. See `docs/native-touch-backends-plan.md` for the
//! design.

use super::super::callbacks::{leak, TouchCallback};
use crate::imp::{with_env, AndroidBackend};
use jni::objects::{GlobalRef, JValue};
use jni::sys::jlong;
use std::cell::RefCell;

pub(crate) fn install(_b: &AndroidBackend, node: &GlobalRef, handler: framework_core::TouchHandler) {
    with_env(|env| {
        let ptr: jlong = leak(TouchCallback {
            inner: RefCell::new(Some(handler)),
        });
        let listener_class = env
            .find_class("io/idealyst/runtime/RustTouchListener")
            .expect("RustTouchListener class missing — bundle the kotlin runtime");
        let listener = env
            .new_object(&listener_class, "(J)V", &[JValue::Long(ptr)])
            .expect("failed to construct RustTouchListener");
        env.call_method(
            node.as_obj(),
            "setOnTouchListener",
            "(Landroid/view/View$OnTouchListener;)V",
            &[JValue::Object(&listener)],
        )
        .expect("setOnTouchListener call failed");
    });
}

/// Implementation of `Backend::claim_touch` for Android.
///
/// `requestDisallowInterceptTouchEvent(true)` on the parent
/// propagates up the entire ancestor chain — every
/// `ViewGroup.onInterceptTouchEvent` honors the flag for the
/// remainder of this gesture and lets touches pass through.
/// `ScrollView`, `NestedScrollView`, `RecyclerView`, and custom
/// containers all respect it. The flag resets on the next
/// `ACTION_DOWN`.
///
/// Note: the Kotlin listener already calls this inline whenever a
/// touch returns `claim: true`, so the Rust trait method is a
/// belt-and-suspenders entry point for any future code path that
/// wants to claim from outside a `MotionEvent` dispatch.
pub(crate) fn claim(_b: &AndroidBackend, node: &GlobalRef) {
    with_env(|env| {
        let parent = env.call_method(node.as_obj(), "getParent", "()Landroid/view/ViewParent;", &[]);
        let Ok(parent) = parent.and_then(|p| p.l()) else {
            return;
        };
        if parent.is_null() {
            return;
        }
        let _ = env.call_method(
            &parent,
            "requestDisallowInterceptTouchEvent",
            "(Z)V",
            &[JValue::Bool(1)],
        );
    });
}
