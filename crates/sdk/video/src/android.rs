//! Android implementation of the Video SDK.
//!
//! Builds an `android.widget.VideoView` per mount. `setVideoURI` takes
//! an `android.net.Uri`, which we build via `Uri.parse(String)`.
//! Reactive src changes flow through `Effect::new(...)`. `start()`,
//! `pause()`, `seekTo(int ms)` drive the imperative ops.
//!
//! `controls` (MediaController) and a true `loop_playback`
//! (`OnCompletionListener.onCompletion` → `seekTo(0); start()`) require
//! Kotlin/Java shim classes — useful but pushed to a follow-up. The
//! framework's prior built-in Android Video impl also stubbed these
//! (the `_controls` / `_loop_playback` params were unused), so this
//! SDK matches that behavior.

use crate::{VideoOps, VideoProps};
// `backend-android-mobile`'s `[lib].name` is `backend_android`
// (preserved historically so `System.loadLibrary("backend_android")`
// keeps working).
use backend_android::{with_jni_env, AndroidBackend};
use jni::objects::{GlobalRef, JObject, JValue};
use runtime_core::Effect;
use std::any::Any;
use std::rc::Rc;

pub(crate) static OPS: &dyn VideoOps = &AndroidVideoOps;

pub fn register(backend: &mut AndroidBackend) {
    backend.register_external::<VideoProps, _>(|props, b| build_video(props, b));
}

fn build_video(props: &Rc<VideoProps>, b: &mut AndroidBackend) -> GlobalRef {
    let view = b.with_jni(|env, ctx| {
        let class = env
            .find_class("android/widget/VideoView")
            .expect("find_class android/widget/VideoView");
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&ctx.as_obj())],
            )
            .expect("new VideoView(Context)");
        backend_android_core::helpers::apply_default_layout_params(env, &local);
        env.new_global_ref(local).expect("new_global_ref")
    });

    // Capture autoplay so the reactive Effect knows whether to call
    // `start()` after the initial `setVideoURI`.
    let autoplay = props.autoplay;
    let view_for_src = view.clone();
    let props_clone = props.clone();
    let first_run = std::cell::Cell::new(true);
    let _src_effect = Effect::new(move || {
        let url = (props_clone.src)();
        set_video_uri(&view_for_src, &url);
        if first_run.replace(false) && autoplay {
            start(&view_for_src);
        }
    });

    view
}

fn set_video_uri(view: &GlobalRef, src: &str) {
    with_jni_env(|env| {
        let uri_class = match env.find_class("android/net/Uri") {
            Ok(c) => c,
            Err(_) => return,
        };
        let java_src = match env.new_string(src) {
            Ok(s) => s,
            Err(_) => return,
        };
        let Ok(call) = env.call_static_method(
            &uri_class,
            "parse",
            "(Ljava/lang/String;)Landroid/net/Uri;",
            &[JValue::Object(&JObject::from(java_src))],
        ) else {
            return;
        };
        let Ok(uri) = call.l() else { return };
        let _ = env.call_method(
            view.as_obj(),
            "setVideoURI",
            "(Landroid/net/Uri;)V",
            &[JValue::Object(&uri)],
        );
    });
}

fn start(view: &GlobalRef) {
    with_jni_env(|env| {
        let _ = env.call_method(view.as_obj(), "start", "()V", &[]);
    });
}

// ============================================================================
// Imperative ops
// ============================================================================

struct AndroidVideoOps;

impl VideoOps for AndroidVideoOps {
    fn play(&self, node: &dyn Any) {
        let Some(view) = node.downcast_ref::<GlobalRef>() else { return };
        start(view);
    }

    fn pause(&self, node: &dyn Any) {
        let Some(view) = node.downcast_ref::<GlobalRef>() else { return };
        with_jni_env(|env| {
            let _ = env.call_method(view.as_obj(), "pause", "()V", &[]);
        });
    }

    fn seek(&self, node: &dyn Any, seconds: f32) {
        let Some(view) = node.downcast_ref::<GlobalRef>() else { return };
        with_jni_env(|env| {
            // VideoView.seekTo(int milliseconds).
            let _ = env.call_method(
                view.as_obj(),
                "seekTo",
                "(I)V",
                &[JValue::Int((seconds * 1000.0) as i32)],
            );
        });
    }
}
