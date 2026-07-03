//! `Element::Image` — bare `android.widget.ImageView` stub. Actual
//! URL loading on Android needs a third-party loader (Glide, Coil,
//! etc.) — not in v1. Authors who need images today should wrap a
//! custom component over this.

use backend_android_core::helpers::apply_default_layout_params;
use crate::imp::{with_env, AndroidBackend};
use jni::objects::{GlobalRef, JObject, JValue};
use jni::JNIEnv;
use runtime_core::ObjectFit;

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
        // Default the fit to Contain (aspect-fit) so a bare Android image
        // matches the framework-wide default and the other backends. Without
        // an explicit scale type an `ImageView` defaults to `FIT_CENTER`
        // (also aspect-fit), but we set it explicitly so the value is known
        // and a later `apply_style` reset to `None` restores a consistent
        // baseline.
        apply_object_fit(env, &local, ObjectFit::Contain);
        env.new_global_ref(local).unwrap()
    })
}

/// The `ImageView.ScaleType` static-field name for an [`ObjectFit`].
fn scale_type_field(fit: ObjectFit) -> &'static str {
    match fit {
        ObjectFit::Fill => "FIT_XY",       // stretch to fill, ignore aspect
        ObjectFit::Contain => "FIT_CENTER", // aspect-fit, letterbox
        ObjectFit::Cover => "CENTER_CROP",  // aspect-fill, center-crop
    }
}

/// Return `true` if `view` is an `android.widget.ImageView` — lets the
/// generic `apply_style` path apply `object_fit` only to images.
pub(crate) fn is_image_view(env: &mut JNIEnv, view: &JObject) -> bool {
    env.find_class("android/widget/ImageView")
        .ok()
        .and_then(|c| env.is_instance_of(view, &c).ok())
        .unwrap_or(false)
}

/// Apply an [`ObjectFit`] to an `ImageView` via `setScaleType`. No-op on
/// non-image views (the caller in `apply_style` guards via [`is_image_view`],
/// but this re-checks so it's safe to call broadly). `CENTER_CROP` (Cover)
/// crops the overflow to the view's bounds for free — the Android analog of
/// CSS `object-fit: cover`.
pub(crate) fn apply_object_fit(env: &mut JNIEnv, view: &JObject, fit: ObjectFit) {
    // `setScaleType` is an `ImageView` method — calling it on any other View
    // throws (leaving a pending Java exception). Guard first.
    if !is_image_view(env, view) {
        return;
    }
    let Ok(scale_type_class) = env.find_class("android/widget/ImageView$ScaleType") else {
        return;
    };
    let st = env
        .get_static_field(
            &scale_type_class,
            scale_type_field(fit),
            "Landroid/widget/ImageView$ScaleType;",
        )
        .and_then(|v| v.l());
    if let Ok(st) = st {
        let _ = env.call_method(
            view,
            "setScaleType",
            "(Landroid/widget/ImageView$ScaleType;)V",
            &[JValue::Object(&st)],
        );
    }
}
