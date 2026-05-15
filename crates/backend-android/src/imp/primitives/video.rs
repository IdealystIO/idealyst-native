//! `Primitive::Video` — `android.widget.VideoView`. Production apps
//! typically use ExoPlayer; we ship the simple path. `_controls` /
//! `_loop_playback` would require a `MediaController` and an
//! `OnCompletionListener` — both straightforward to add but skipped
//! for v1.

use crate::imp::helpers::apply_default_layout_params;
use crate::imp::{with_env, AndroidBackend};
use framework_core::primitives::video::{VideoHandle, VideoOps};
use jni::objects::{GlobalRef, JObject, JValue};
use std::any::Any;
use std::rc::Rc;

pub(crate) fn create(
    b: &AndroidBackend,
    src: &str,
    autoplay: bool,
    _controls: bool,
    _loop_playback: bool,
) -> GlobalRef {
    with_env(|env| {
        let class = env.find_class("android/widget/VideoView").unwrap();
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .unwrap();
        // setVideoPath(String) for local files; setVideoURI for URIs.
        // Use setVideoURI by parsing the string.
        let uri_class = env.find_class("android/net/Uri").unwrap();
        let java_src = env.new_string(src).unwrap();
        let uri = env
            .call_static_method(
                &uri_class,
                "parse",
                "(Ljava/lang/String;)Landroid/net/Uri;",
                &[JValue::Object(&JObject::from(java_src))],
            )
            .unwrap()
            .l()
            .unwrap();
        let _ = env.call_method(
            &local,
            "setVideoURI",
            "(Landroid/net/Uri;)V",
            &[JValue::Object(&uri)],
        );
        if autoplay {
            let _ = env.call_method(&local, "start", "()V", &[]);
        }
        apply_default_layout_params(env, &local);
        env.new_global_ref(local).unwrap()
    })
}

pub(crate) fn update_src(node: &GlobalRef, src: &str) {
    with_env(|env| {
        let uri_class = env.find_class("android/net/Uri").unwrap();
        let java_src = env.new_string(src).unwrap();
        let uri = env
            .call_static_method(
                &uri_class,
                "parse",
                "(Ljava/lang/String;)Landroid/net/Uri;",
                &[JValue::Object(&JObject::from(java_src))],
            )
            .unwrap()
            .l()
            .unwrap();
        let _ = env.call_method(
            node.as_obj(),
            "setVideoURI",
            "(Landroid/net/Uri;)V",
            &[JValue::Object(&uri)],
        );
    });
}

pub(crate) fn make_handle(node: &GlobalRef) -> VideoHandle {
    VideoHandle::new(Rc::new(node.clone()), &AndroidVideoOps)
}

struct AndroidVideoOps;
impl VideoOps for AndroidVideoOps {
    fn play(&self, node: &dyn Any) {
        let Some(gref) = node.downcast_ref::<GlobalRef>() else {
            return;
        };
        with_env(|env| {
            let _ = env.call_method(gref.as_obj(), "start", "()V", &[]);
        });
    }
    fn pause(&self, node: &dyn Any) {
        let Some(gref) = node.downcast_ref::<GlobalRef>() else {
            return;
        };
        with_env(|env| {
            let _ = env.call_method(gref.as_obj(), "pause", "()V", &[]);
        });
    }
    fn seek(&self, node: &dyn Any, seconds: f32) {
        let Some(gref) = node.downcast_ref::<GlobalRef>() else {
            return;
        };
        with_env(|env| {
            // seekTo(int milliseconds).
            let _ = env.call_method(
                gref.as_obj(),
                "seekTo",
                "(I)V",
                &[JValue::Int((seconds * 1000.0) as i32)],
            );
        });
    }
}
