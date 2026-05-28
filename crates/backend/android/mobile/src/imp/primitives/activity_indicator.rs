//! `Element::ActivityIndicator` — indeterminate
//! `android.widget.ProgressBar`. Size is approximate; ProgressBar's
//! default is ~36dp which is closer to RN's "Large". Custom sizing
//! requires LayoutParams which is beyond v1 scope.

use backend_android_core::helpers::apply_default_layout_params;
use crate::imp::{with_env, AndroidBackend};
use runtime_core::primitives::activity_indicator::ActivityIndicatorSize;
use jni::objects::{GlobalRef, JValue};

pub(crate) fn create(
    b: &AndroidBackend,
    _size: ActivityIndicatorSize,
    _color: Option<&runtime_core::Color>,
) -> GlobalRef {
    with_env(|env| {
        let class = env.find_class("android/widget/ProgressBar").unwrap();
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .unwrap();
        let _ = env.call_method(
            &local,
            "setIndeterminate",
            "(Z)V",
            &[JValue::Bool(1)],
        );
        apply_default_layout_params(env, &local);
        env.new_global_ref(local).unwrap()
    })
}
