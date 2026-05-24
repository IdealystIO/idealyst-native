//! `Primitive::Toggle` — `android.widget.Switch` with a Kotlin-side
//! `RustToggleListener` bridge.

use crate::imp::callbacks::{leak, ToggleChangeCallback};
use backend_android_core::helpers::apply_default_layout_params;
use crate::imp::{with_env, AndroidBackend};
use jni::objects::{GlobalRef, JValue};
use jni::sys::jlong;
use std::rc::Rc;

pub(crate) fn create(
    b: &AndroidBackend,
    initial_value: bool,
    on_change: Rc<dyn Fn(bool)>,
) -> GlobalRef {
    with_env(|env| {
        let class = env.find_class("android/widget/Switch").unwrap();
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .unwrap();
        // Initial value.
        let _ = env.call_method(
            &local,
            "setChecked",
            "(Z)V",
            &[JValue::Bool(if initial_value { 1 } else { 0 })],
        );
        // Wire the listener.
        let ptr: jlong = leak(ToggleChangeCallback(on_change));
        let listener_class = env
            .find_class("io/idealyst/runtime/RustToggleListener")
            .unwrap();
        let listener = env
            .new_object(&listener_class, "(J)V", &[JValue::Long(ptr)])
            .unwrap();
        let _ = env.call_method(
            &local,
            "setOnCheckedChangeListener",
            "(Landroid/widget/CompoundButton$OnCheckedChangeListener;)V",
            &[JValue::Object(&listener)],
        );
        // Stash the listener on the Switch's tag so update_value can
        // retrieve it later and flip its `suppress` flag without
        // having to detach + reattach.
        let _ = env.call_method(
            &local,
            "setTag",
            "(Ljava/lang/Object;)V",
            &[JValue::Object(&listener)],
        );
        apply_default_layout_params(env, &local);
        env.new_global_ref(local).unwrap()
    })
}

/// Apply a programmatic value to the toggle. Suppresses the
/// `OnCheckedChangeListener` for the duration of the `setChecked`
/// call — without this guard, an runtime-server-driven value update (server
/// emits the authoritative `value` over the wire) would race a
/// recent user tap: `setChecked` fires the listener → listener
/// sends the new value back to the server as an event → server
/// rewrites `is_dark`/etc. and re-emits → loop. The visible
/// symptom is the toggle flipping on its own a few times after a
/// spam-click. We retrieve the listener via the tag set in `create`
/// rather than detach-and-reattach, which would race with other
/// callbacks scheduled on the same JNI thread.
pub(crate) fn update_value(node: &GlobalRef, value: bool) {
    with_env(|env| {
        let tag = env
            .call_method(node.as_obj(), "getTag", "()Ljava/lang/Object;", &[])
            .ok()
            .and_then(|v| v.l().ok());
        if let Some(ref listener) = tag {
            if !listener.is_null() {
                let _ = env.set_field(listener, "suppress", "Z", JValue::Bool(1));
            }
        }
        let _ = env.call_method(
            node.as_obj(),
            "setChecked",
            "(Z)V",
            &[JValue::Bool(if value { 1 } else { 0 })],
        );
        if let Some(ref listener) = tag {
            if !listener.is_null() {
                let _ = env.set_field(listener, "suppress", "Z", JValue::Bool(0));
            }
        }
    });
}
