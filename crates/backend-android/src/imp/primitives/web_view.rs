//! `Primitive::WebView` — `android.webkit.WebView` with reactive URL.

use crate::imp::helpers::apply_default_layout_params;
use crate::imp::{with_env, AndroidBackend};
use jni::objects::{GlobalRef, JObject, JValue};

pub(crate) fn create(b: &AndroidBackend, url: &str) -> GlobalRef {
    with_env(|env| {
        let class = env.find_class("android/webkit/WebView").unwrap();
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .unwrap();
        let java_url = env.new_string(url).unwrap();
        let _ = env.call_method(
            &local,
            "loadUrl",
            "(Ljava/lang/String;)V",
            &[JValue::Object(&JObject::from(java_url))],
        );
        apply_default_layout_params(env, &local);
        env.new_global_ref(local).unwrap()
    })
}

pub(crate) fn update_url(node: &GlobalRef, url: &str) {
    with_env(|env| {
        let java_url = env.new_string(url).unwrap();
        let _ = env.call_method(
            node.as_obj(),
            "loadUrl",
            "(Ljava/lang/String;)V",
            &[JValue::Object(&JObject::from(java_url))],
        );
    });
}
