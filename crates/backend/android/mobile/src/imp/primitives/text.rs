//! `Primitive::Text` — `android.widget.TextView`.

use backend_android_core::helpers::{apply_default_layout_params, set_text};
use crate::imp::{with_env, AndroidBackend};
use jni::objects::{GlobalRef, JValue};

pub(crate) fn create(b: &AndroidBackend, content: &str) -> GlobalRef {
    with_env(|env| {
        let class = env.find_class("android/widget/TextView").unwrap();
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .unwrap();
        set_text(env, &local, content);
        apply_default_layout_params(env, &local);
        env.new_global_ref(local).unwrap()
    })
}

pub(crate) fn update_text(node: &GlobalRef, content: &str) {
    with_env(|env| set_text(env, &node.as_obj(), content));
}
