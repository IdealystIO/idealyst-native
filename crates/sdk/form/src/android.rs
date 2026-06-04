//! Android implementation of the Form SDK.
//!
//! Android has no "form" construct, and its form affordances (autofill
//! hints via `setAutofillHints`, IME-action submit via `imeOptions`)
//! live per-field on the inputs, not on a container. So the Android
//! `Form` is a plain passthrough `FrameLayout`: the framework parents
//! the author's children into it and Taffy lays them out. The
//! `on_submit` closure is NOT auto-triggered here — submission is fired
//! by the author's submit `Button` calling `on_submit` directly.

use crate::{FormOps, FormProps};
// The Android backend's package is `backend-android-mobile` but its
// `[lib].name` is `backend_android`.
use backend_android::AndroidBackend;
use jni::objects::{GlobalRef, JValue};

pub(crate) static OPS: &dyn FormOps = &AndroidFormOps;

/// Register the Form handler against an `AndroidBackend`. One-line call from
/// app bootstrap.
pub fn register(backend: &mut AndroidBackend) {
    backend.register_external::<FormProps, _>(|_props, b| build_form(b));
}

// Self-register at backend construction (no app-side `register` call needed).
// See [[project_inventory_self_registration]].
inventory::submit! {
    backend_android::AndroidExternalRegistrar(register)
}

fn build_form(b: &mut AndroidBackend) -> GlobalRef {
    b.with_jni(|env, ctx| {
        // FrameLayout is the framework's container view (children are
        // positioned absolutely via Taffy frame margins — matching how
        // the backend builds every other container).
        let class = env
            .find_class("android/widget/FrameLayout")
            .expect("find_class android/widget/FrameLayout");
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&ctx.as_obj())],
            )
            .expect("new FrameLayout(Context)");
        backend_android_core::helpers::apply_default_layout_params(env, &local);
        env.new_global_ref(local).expect("new_global_ref")
    })
}

struct AndroidFormOps;

// `submit` stays the trait default no-op: Android has no form-submit
// event to drive. Author code triggers submission by invoking its
// `on_submit` closure from the submit Button's `on_press`.
impl FormOps for AndroidFormOps {}
