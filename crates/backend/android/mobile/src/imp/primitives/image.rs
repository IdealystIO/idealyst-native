//! `Element::Image` — bare `android.widget.ImageView` stub. Actual
//! URL loading on Android needs a third-party loader (Glide, Coil,
//! etc.) — not in v1. Authors who need images today should wrap a
//! custom component over this.

use backend_android_core::helpers::apply_default_layout_params;
use crate::imp::{with_env, AndroidBackend};
use jni::objects::{GlobalRef, JValue};

pub(crate) fn create(b: &AndroidBackend, _src: &str, _alt: Option<&str>) -> GlobalRef {
    with_env(|env| {
        let class = env.find_class("android/widget/ImageView").unwrap();
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .unwrap();
        apply_default_layout_params(env, &local);
        env.new_global_ref(local).unwrap()
    })
}
