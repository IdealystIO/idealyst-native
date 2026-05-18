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
        apply_default_layout_params(env, &local);
        env.new_global_ref(local).unwrap()
    })
}

pub(crate) fn update_value(node: &GlobalRef, value: bool) {
    with_env(|env| {
        let _ = env.call_method(
            node.as_obj(),
            "setChecked",
            "(Z)V",
            &[JValue::Bool(if value { 1 } else { 0 })],
        );
    });
}
