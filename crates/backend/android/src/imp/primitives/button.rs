//! `Primitive::Button` — `android.widget.Button` with an
//! `OnClickListener` that trampolines back to Rust via JNI.

use crate::imp::callbacks::{leak, ClickCallback};
use crate::imp::helpers::{apply_default_layout_params, set_text, view_screen_rect};
use crate::imp::{with_env, AndroidBackend};
use framework_core::primitives::overlay::ViewportRect;
use framework_core::{ButtonHandle, ButtonOps};
use jni::objects::{GlobalRef, JValue};
use jni::sys::jlong;
use std::any::Any;
use std::rc::Rc;

pub(crate) fn create(b: &AndroidBackend, label: &str, on_click: Rc<dyn Fn()>) -> GlobalRef {
    with_env(|env| {
        let class = env.find_class("android/widget/Button").unwrap();
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .unwrap();
        set_text(env, &local, label);

        // Box the callback, then leak it to get a stable pointer the
        // JVM can hold. The Kotlin listener stores this as a `Long`
        // and passes it back via `nativeInvoke`.
        let ptr: jlong = leak(ClickCallback(on_click));

        let listener_class = env
            .find_class("io/idealyst/runtime/RustClickListener")
            .unwrap();
        let listener = env
            .new_object(&listener_class, "(J)V", &[JValue::Long(ptr)])
            .unwrap();
        env.call_method(
            &local,
            "setOnClickListener",
            "(Landroid/view/View$OnClickListener;)V",
            &[JValue::Object(&listener)],
        )
        .unwrap();

        apply_default_layout_params(env, &local);
        env.new_global_ref(local).unwrap()
    })
}

pub(crate) fn make_handle(node: &GlobalRef) -> ButtonHandle {
    ButtonHandle::new(Rc::new(node.clone()), &AndroidButtonOps)
}

struct AndroidButtonOps;
impl ButtonOps for AndroidButtonOps {
    fn click(&self, node: &dyn Any) {
        let Some(gref) = node.downcast_ref::<GlobalRef>() else {
            return;
        };
        with_env(|env| {
            let _ = env.call_method(gref.as_obj(), "performClick", "()Z", &[]);
        });
    }

    /// Screen-relative rect in physical pixels — origin top-left of
    /// the device screen, including the status bar. We use screen
    /// coords (not viewport / window coords) because the only
    /// consumer on Android is `Overlay`'s `PopupWindow` anchoring,
    /// which itself takes screen coords via `showAtLocation`. The
    /// framework's `ViewportRect` is otherwise coord-system-agnostic
    /// — backends just need internal consistency between the
    /// producer (this method) and the consumer (overlay positioning).
    fn rect(&self, node: &dyn Any) -> ViewportRect {
        node.downcast_ref::<GlobalRef>()
            .map(|gref| view_screen_rect(gref))
            .unwrap_or_default()
    }
}
