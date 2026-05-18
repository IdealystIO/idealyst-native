//! `Primitive::Link` — clickable container that fires `on_activate`
//! when tapped (the framework wraps push/replace/reset dispatch in
//! there, so the backend doesn't have to care about the nav shape).
//!
//! Implementation: a `FrameLayout` with `setClickable(true)` and a
//! `setOnClickListener` wired to a `RustClickListener` whose
//! `nativePtr` is a leaked `ClickCallback`. The framework's
//! `Backend::insert` then attaches the link's child views via the
//! normal view-tree path — no special-casing needed since `Link` is
//! just a container as far as the walker is concerned.

use crate::imp::callbacks::{leak, ClickCallback};
use crate::imp::{with_env, AndroidBackend};
use backend_android_core::helpers::apply_default_layout_params;
use jni::objects::{GlobalRef, JValue};
use jni::sys::jlong;

pub(crate) fn create(b: &AndroidBackend, on_activate: std::rc::Rc<dyn Fn()>) -> GlobalRef {
    with_env(|env| {
        let class = env.find_class("android/widget/FrameLayout").unwrap();
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .unwrap();

        // Without `setClickable(true)`, FrameLayout swallows the touch
        // event before any OnClickListener fires.
        let _ = env.call_method(&local, "setClickable", "(Z)V", &[JValue::Bool(1)]);

        let ptr: jlong = leak(ClickCallback(on_activate));
        let listener_class = env
            .find_class("io/idealyst/runtime/RustClickListener")
            .unwrap();
        let listener = env
            .new_object(&listener_class, "(J)V", &[JValue::Long(ptr)])
            .unwrap();
        let _ = env.call_method(
            &local,
            "setOnClickListener",
            "(Landroid/view/View$OnClickListener;)V",
            &[JValue::Object(&listener)],
        );

        apply_default_layout_params(env, &local);
        env.new_global_ref(local).unwrap()
    })
}
