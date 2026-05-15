//! `Primitive::View` — `android.widget.LinearLayout` in vertical
//! orientation (matches the framework's default flex-column).

use crate::imp::helpers::apply_default_layout_params;
use crate::imp::{with_env, AndroidBackend};
use jni::objects::{GlobalRef, JValue};

pub(crate) fn create(b: &AndroidBackend) -> GlobalRef {
    with_env(|env| {
        let class = env.find_class("android/widget/LinearLayout").unwrap();
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .unwrap();
        // Vertical orientation (1) so children stack top-to-bottom,
        // matching the framework's default flex-column layout.
        env.call_method(&local, "setOrientation", "(I)V", &[JValue::Int(1)])
            .unwrap();
        apply_default_layout_params(env, &local);
        env.new_global_ref(local).unwrap()
    })
}

pub(crate) fn insert(parent: &mut GlobalRef, child: GlobalRef) {
    with_env(|env| {
        env.call_method(
            parent.as_obj(),
            "addView",
            "(Landroid/view/View;)V",
            &[JValue::Object(&child.as_obj())],
        )
        .unwrap();
    });
}

pub(crate) fn clear_children(node: &GlobalRef) {
    with_env(|env| {
        env.call_method(node.as_obj(), "removeAllViews", "()V", &[])
            .unwrap();
    });
}
