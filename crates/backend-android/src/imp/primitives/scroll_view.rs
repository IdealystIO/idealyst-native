//! `Primitive::ScrollView` — a `ScrollView` (or `HorizontalScrollView`)
//! wrapping a `LinearLayout`. We return the inner LinearLayout as the
//! framework's node so child insertions go into the scrollable area.

use crate::imp::helpers::apply_default_layout_params;
use crate::imp::{with_env, AndroidBackend};
use jni::objects::{GlobalRef, JValue};

pub(crate) fn create(b: &AndroidBackend, horizontal: bool) -> GlobalRef {
    // ScrollView is a single-child ViewGroup. To accept multiple
    // children we wrap a LinearLayout inside the ScrollView and call
    // `addView` on the inner layout. We expose the inner layout as
    // the node so the framework's `insert` calls hit it directly.
    with_env(|env| {
        let outer_class = if horizontal {
            env.find_class("android/widget/HorizontalScrollView").unwrap()
        } else {
            env.find_class("android/widget/ScrollView").unwrap()
        };
        let outer = env
            .new_object(
                &outer_class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .unwrap();
        let inner_class = env.find_class("android/widget/LinearLayout").unwrap();
        let inner = env
            .new_object(
                &inner_class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .unwrap();
        let orient = if horizontal { 0 } else { 1 };
        let _ = env.call_method(&inner, "setOrientation", "(I)V", &[JValue::Int(orient)]);
        let _ = env.call_method(
            &outer,
            "addView",
            "(Landroid/view/View;)V",
            &[JValue::Object(&inner)],
        );
        // Style application targets the inner too — outer is just a
        // chrome wrapper. This is a v1 compromise; if padding/margin
        // behaves oddly on the outer ScrollView, we can revisit.
        apply_default_layout_params(env, &outer);
        apply_default_layout_params(env, &inner);
        env.new_global_ref(inner).unwrap()
    })
}
