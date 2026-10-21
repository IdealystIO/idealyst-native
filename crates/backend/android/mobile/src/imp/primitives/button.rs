//! `ButtonPrim` — `android.widget.Button` with an
//! `OnClickListener` that trampolines back to Rust via JNI.

use crate::imp::callbacks::{leak, ClickCallback};
use backend_android_core::helpers::{apply_default_layout_params, set_text};
use crate::imp::view_rect::view_viewport_rect;
use crate::imp::{with_env, AndroidBackend};
use runtime_shared::primitives::portal::ViewportRect;
use runtime_shared::{ButtonHandle, ButtonOps};
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

        // Drop the Material button's elevation (2dp at rest) and its
        // press `StateListAnimator`, which animates `translationZ` on
        // press/release. Z is the sibling stacking channel
        // (`StyleRules::z_index` → `style::set_translation_z`): the
        // resting elevation would paint every button above later
        // siblings at equal z, and the animator would overwrite the
        // style's `translationZ` on every press.
        let null = jni::objects::JObject::null();
        let _ = env.call_method(
            &local,
            "setStateListAnimator",
            "(Landroid/animation/StateListAnimator;)V",
            &[JValue::Object(&null)],
        );
        let _ = env.call_method(&local, "setElevation", "(F)V", &[JValue::Float(0.0)]);

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

    /// Viewport-relative rect in dp (origin = the Activity root) — the
    /// `ViewportRect` contract every backend's `rect()` honors, and the
    /// space the anchored portal places in. (Was screen px for the old
    /// `PopupWindow.showAtLocation`, which nothing else on any backend
    /// used.)
    fn rect(&self, node: &dyn Any) -> ViewportRect {
        node.downcast_ref::<GlobalRef>()
            .map(view_viewport_rect)
            .unwrap_or_default()
    }
}
