//! `Primitive::TextInput` — `android.widget.EditText` with a
//! `RustTextWatcher` for `on_change` bridging.

use crate::imp::callbacks::{leak, TextChangeCallback};
use backend_android_core::helpers::{apply_default_layout_params, set_text};
use crate::imp::{with_env, AndroidBackend};
use jni::objects::{GlobalRef, JObject, JValue};
use jni::sys::jlong;
use std::rc::Rc;

pub(crate) fn create(
    b: &AndroidBackend,
    initial_value: &str,
    placeholder: Option<&str>,
    on_change: Rc<dyn Fn(String)>,
) -> GlobalRef {
    // EditText with a TextWatcher dispatched through Kotlin
    // `RustTextWatcher`. Same lifecycle/leak pattern as
    // RustClickListener: box + leak the on_change closure. The native
    // widget calls back into `Java_io_idealyst_runtime_RustTextWatcher_nativeChanged`
    // on every keystroke.
    with_env(|env| {
        let class = env.find_class("android/widget/EditText").unwrap();
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .unwrap();
        set_text(env, &local, initial_value);
        if let Some(p) = placeholder {
            let java_str = env.new_string(p).unwrap();
            let _ = env.call_method(
                &local,
                "setHint",
                "(Ljava/lang/CharSequence;)V",
                &[JValue::Object(&JObject::from(java_str))],
            );
        }
        // Wire the watcher.
        let ptr: jlong = leak(TextChangeCallback(on_change));
        let watcher_class = env
            .find_class("io/idealyst/runtime/RustTextWatcher")
            .unwrap();
        let watcher = env
            .new_object(&watcher_class, "(J)V", &[JValue::Long(ptr)])
            .unwrap();
        let _ = env.call_method(
            &local,
            "addTextChangedListener",
            "(Landroid/text/TextWatcher;)V",
            &[JValue::Object(&watcher)],
        );
        apply_default_layout_params(env, &local);
        env.new_global_ref(local).unwrap()
    })
}

pub(crate) fn update_value(node: &GlobalRef, value: &str) {
    with_env(|env| {
        // Only update if the text differs, to avoid cursor jumps when
        // our own listener wrote back to the signal.
        let current = env
            .call_method(node.as_obj(), "getText", "()Landroid/text/Editable;", &[])
            .ok()
            .and_then(|v| v.l().ok());
        let same = current
            .as_ref()
            .map(|cur| {
                env.call_method(cur, "toString", "()Ljava/lang/String;", &[])
                    .ok()
                    .and_then(|v| v.l().ok())
                    .and_then(|s| {
                        let jstr: jni::objects::JString = s.into();
                        env.get_string(&jstr)
                            .ok()
                            .map(|js| js.to_str().unwrap_or("").to_string())
                    })
                    .map(|s| s == value)
                    .unwrap_or(false)
            })
            .unwrap_or(false);
        if !same {
            set_text(env, &node.as_obj(), value);
        }
    });
}
