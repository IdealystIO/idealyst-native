//! `ActivityIndicatorPrim` — indeterminate `android.widget.ProgressBar`.
//!
//! A `ProgressBar` draws its spinner scaled to whatever box it is laid out
//! in, so the size is the Taffy leaf's: the measurer reports
//! [`crate::layout_policy::activity_indicator_measure`] and the drawable follows
//! the frame. Without the measurer the leaf is 0x0 and nothing draws.

use backend_android_core::helpers::{apply_default_layout_params, parse_color};
use crate::imp::{with_env, AndroidBackend};
use runtime_shared::primitives::activity_indicator::ActivityIndicatorSize;
use jni::objects::{GlobalRef, JValue};

pub(crate) fn create(
    b: &mut AndroidBackend,
    size: ActivityIndicatorSize,
    color: Option<&runtime_shared::Color>,
) -> GlobalRef {
    let node = with_env(|env| {
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
        if let Some(argb) = color.and_then(|c| parse_color(&c.0)) {
            let tint = env
                .call_static_method(
                    "android/content/res/ColorStateList",
                    "valueOf",
                    "(I)Landroid/content/res/ColorStateList;",
                    &[JValue::Int(argb)],
                )
                .and_then(|v| v.l())
                .unwrap();
            let _ = env.call_method(
                &local,
                "setIndeterminateTintList",
                "(Landroid/content/res/ColorStateList;)V",
                &[JValue::Object(&tint)],
            );
        }
        apply_default_layout_params(env, &local);
        env.new_global_ref(local).unwrap()
    });
    let layout = b.layout_for_view(&node);
    b.layout.set_measure_fn(
        layout,
        std::rc::Rc::new(move |known, _available| {
            let (width, height) =
                crate::layout_policy::activity_indicator_measure((known.width, known.height), size);
            runtime_layout::Size { width, height }
        }),
    );
    node
}
