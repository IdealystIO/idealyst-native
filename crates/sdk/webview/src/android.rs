//! Android implementation of the WebView SDK.
//!
//! Builds an `android.webkit.WebView`, navigates via `loadUrl(String)`,
//! subscribes to reactive URL changes through an `effect!`.
//!
//! Callback wiring (`on_message`/`on_load`/`on_error`) is a no-op in
//! v1 — needs a `WebViewClient` subclass (`onPageFinished`/
//! `onReceivedError`) and a `@JavascriptInterface`-annotated bridge
//! class to be useful. Those Kotlin shims would ship from this same
//! crate via `[package.metadata.idealyst.android].runtime_kotlin`
//! once added.

use crate::{WebViewOps, WebViewProps};
// The Android backend's package is `backend-android-mobile` but its
// `[lib].name` is `backend_android` (preserved historically so the
// JNI `System.loadLibrary("backend_android")` call keeps working).
use backend_android::{with_jni_env, AndroidBackend};
use jni::objects::{GlobalRef, JObject, JValue};
use std::any::Any;
use std::rc::Rc;

pub(crate) static OPS: &dyn WebViewOps = &AndroidWebViewOps;

/// Register the WebView handler against an `AndroidBackend`. One-line call from
/// app bootstrap.
pub fn register(backend: &mut AndroidBackend) {
    backend.register_external::<WebViewProps, _>(|props, b| build_web_view(props, b));
}

// Self-register at backend construction (no app-side `register` call needed).
// See [[project_inventory_self_registration]].
inventory::submit! {
    backend_android::AndroidExternalRegistrar(register)
}

fn build_web_view(props: &Rc<WebViewProps>, b: &mut AndroidBackend) -> GlobalRef {
    let view = b.with_jni(|env, ctx| {
        let class = env
            .find_class("android/webkit/WebView")
            .expect("find_class android/webkit/WebView");
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&ctx.as_obj())],
            )
            .expect("new WebView(Context)");
        backend_android_core::helpers::apply_default_layout_params(env, &local);
        env.new_global_ref(local).expect("new_global_ref")
    });

    // Reactive URL — Effect runs initially (driving the first
    // `loadUrl`) and then re-runs whenever signals read inside `url()`
    // change. The walker's active scope owns this Effect, so it
    // outlives the handler return.
    let view_for_url = view.clone();
    let props_clone = props.clone();
    runtime_core::effect!({
        let url = (props_clone.url)();
        load_url(&view_for_url, &url);
    });

    view
}

fn load_url(view: &GlobalRef, url: &str) {
    with_jni_env(|env| {
        let java_url = match env.new_string(url) {
            Ok(s) => s,
            Err(_) => return,
        };
        let _ = env.call_method(
            view.as_obj(),
            "loadUrl",
            "(Ljava/lang/String;)V",
            &[JValue::Object(&JObject::from(java_url))],
        );
    });
}

// ============================================================================
// Imperative ops
// ============================================================================

struct AndroidWebViewOps;

impl WebViewOps for AndroidWebViewOps {
    fn reload(&self, node: &dyn Any) {
        let Some(view) = node.downcast_ref::<GlobalRef>() else {
            return;
        };
        with_jni_env(|env| {
            let _ = env.call_method(view.as_obj(), "reload", "()V", &[]);
        });
    }

    // `post_message` + `execute_js` need a Kotlin/Java bridge to be
    // useful (`WebView.evaluateJavascript` delivers results via an
    // async callback, so the synchronous trait signature can't be
    // honored without a thread-blocking shim). Leaving them as the
    // trait defaults until the bridge lands.
}
