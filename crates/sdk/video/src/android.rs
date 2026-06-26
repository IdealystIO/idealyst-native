//! Android implementation of the Video SDK.
//!
//! Mounts a `FrameLayout` host. The source decides the child it hosts:
//!
//! - **URL** → an `android.widget.VideoView` child. `setVideoURI` takes an
//!   `android.net.Uri` (`Uri.parse(String)`); reactive URL changes flow
//!   through an `effect!`. `start`/`pause`/`seekTo` drive the ops.
//! - **Live `MediaStream`** → an `ImageView` child whose `Bitmap` is replaced
//!   each frame from the stream's tightly-packed RGBA8 frames. We poll the
//!   stream's latest frame on the framework's frame loop (which runs on the
//!   main/UI looper, so the view updates are UI-thread-safe) and hand the
//!   bytes to the Kotlin shim. The universal CPU path — works for ANY
//!   MediaStream (camera, screen, a compositor's output). A zero-copy native
//!   path (camera→`SurfaceTexture`) is the GPU phase.
//!
//! The `FrameLayout` + child management lives in the `RustVideoFrameSink`
//! Kotlin shim (shipped via `[package.metadata.idealyst.android].runtime_kotlin`).
//!
//! ## VERIFICATION
//!
//! Compile-checked for `aarch64-linux-android`; **not yet device-verified**
//! (the Kotlin path + bitmap upload run only on a device — same posture as
//! the camera SDK's Android backend).

use crate::{MediaContent, VideoOps, VideoProps};
// `backend-android-mobile`'s `[lib].name` is `backend_android`
// (preserved historically so `System.loadLibrary("backend_android")`
// keeps working).
use backend_android::{with_jni_env, AndroidBackend};
use jni::objects::{GlobalRef, JObject, JValue};
use runtime_core::effect;
use std::any::Any;
use std::rc::Rc;

const SINK: &str = "io/idealyst/video/RustVideoFrameSink";

pub(crate) static OPS: &dyn VideoOps = &AndroidVideoOps;

/// Register the Video handler against an `AndroidBackend`. One-line call
/// from the app's bootstrap.
pub fn register(backend: &mut AndroidBackend) {
    backend.register_external::<VideoProps, _>(|props, b| build_video(props, b));
}

// Self-register at backend construction (no app-side `register` call needed).
// See [[project_inventory_self_registration]].
inventory::submit! {
    backend_android::AndroidExternalRegistrar(register)
}

fn build_video(props: &Rc<VideoProps>, b: &mut AndroidBackend) -> GlobalRef {
    // The host is a FrameLayout; the Kotlin shim adds a VideoView (URL) or
    // ImageView (stream) child as needed.
    let host = b.with_jni(|env, ctx| {
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
    });

    // URL path — reactive Effect (efficient; fires only on change).
    let autoplay = props.autoplay;
    // object-fit: Cover → fill the box (no black letterbox); Contain → aspect-fit.
    let url_fill = matches!(props.object_fit, crate::ObjectFit::Cover);
    let host_for_url = host.clone();
    let props_for_url = props.clone();
    let first_run = std::cell::Cell::new(true);
    effect!({
        let url = match props_for_url.source.resolve() {
            MediaContent::Url(u) => u,
            MediaContent::Stream(_) | MediaContent::None => return,
        };
        set_video_uri(&host_for_url, &url, url_fill);
        if first_run.replace(false) && autoplay {
            start(&host_for_url);
        }
    });

    // Stream path — poll the latest frame each frame on the UI looper.
    let host_for_stream = host.clone();
    let props_for_stream = props.clone();
    // object-fit: Cover → CENTER_CROP (fill+crop), Contain → FIT_CENTER
    // (letterbox). Static; passed to the Kotlin sink which sets the ImageView's
    // scaleType.
    let cover = matches!(props.object_fit, crate::ObjectFit::Cover);
    let mut last_gen: u64 = u64::MAX;
    let mut scratch: Vec<u8> = Vec::new();
    runtime_core::raf_loop_scoped(move || {
        let MediaContent::Stream(stream) = props_for_stream.source.resolve() else {
            return;
        };
        let generation = stream.generation();
        if generation == last_gen {
            return;
        }
        last_gen = generation;
        if let Some((w, h)) = stream.latest(&mut scratch) {
            show_frame(&host_for_stream, &scratch, w, h, cover);
        }
    });

    host
}

/// Push one RGBA8 frame to the host's stream ImageView (Kotlin makes the
/// Bitmap + setImageBitmap). Called on the UI thread from the frame loop.
fn show_frame(host: &GlobalRef, rgba: &[u8], width: u32, height: u32, cover: bool) {
    with_jni_env(|env| {
        let needed = (width as usize) * (height as usize) * 4;
        if rgba.len() < needed {
            return;
        }
        // Hand Kotlin a direct ByteBuffer that VIEWS our RGBA slice — no
        // `byte[]` allocation and no Rust→JVM copy (the old
        // `byte_array_from_slice` did both, ~8 MB/frame at 1080p). Kotlin
        // copies straight into the Bitmap (`copyPixelsFromBuffer`)
        // synchronously, before this call returns.
        //
        // SAFETY: `rgba` outlives this synchronous `showFrame` call; the
        // direct buffer only aliases the slice for the call's duration, and
        // Kotlin reads (never retains) it.
        let buf = match unsafe { env.new_direct_byte_buffer(rgba.as_ptr() as *mut u8, needed) } {
            Ok(b) => b,
            Err(_) => return,
        };
        let _ = env.call_static_method(
            SINK,
            "showFrame",
            "(Landroid/widget/FrameLayout;Ljava/nio/ByteBuffer;IIZ)V",
            &[
                JValue::Object(host.as_obj()),
                JValue::Object(&buf),
                JValue::Int(width as i32),
                JValue::Int(height as i32),
                JValue::Bool(cover as u8),
            ],
        );
    });
}

/// Ensure the host has a VideoView child and point it at `src`. `fill` selects
/// object-fit cover (stretch to fill) vs the stock aspect-fit (Contain).
fn set_video_uri(host: &GlobalRef, src: &str, fill: bool) {
    with_jni_env(|env| {
        let Some(video_view) = ensure_video_view(env, host, fill) else {
            return;
        };
        let Ok(uri_class) = env.find_class("android/net/Uri") else {
            return;
        };
        let Ok(java_src) = env.new_string(src) else {
            return;
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
            &video_view,
            "setVideoURI",
            "(Landroid/net/Uri;)V",
            &[JValue::Object(&uri)],
        );
    });
}

/// `RustVideoFrameSink.ensureVideoView(host)` → the (created-if-needed)
/// VideoView child.
fn ensure_video_view<'a>(
    env: &mut jni::JNIEnv<'a>,
    host: &GlobalRef,
    fill: bool,
) -> Option<JObject<'a>> {
    env.call_static_method(
        SINK,
        "ensureVideoView",
        "(Landroid/widget/FrameLayout;Z)Landroid/widget/VideoView;",
        &[JValue::Object(host.as_obj()), JValue::Bool(fill as u8)],
    )
    .ok()?
    .l()
    .ok()
}

/// `RustVideoFrameSink.videoView(host)` → the existing VideoView child (or
/// `None`), for imperative ops that shouldn't create one.
fn existing_video_view<'a>(env: &mut jni::JNIEnv<'a>, host: &GlobalRef) -> Option<JObject<'a>> {
    let view = env
        .call_static_method(
            SINK,
            "videoView",
            "(Landroid/widget/FrameLayout;)Landroid/widget/VideoView;",
            &[JValue::Object(host.as_obj())],
        )
        .ok()?
        .l()
        .ok()?;
    (!view.is_null()).then_some(view)
}

fn start(host: &GlobalRef) {
    with_jni_env(|env| {
        if let Some(vv) = existing_video_view(env, host) {
            let _ = env.call_method(&vv, "start", "()V", &[]);
        }
    });
}

// ============================================================================
// Imperative ops — route to the host's VideoView child.
// ============================================================================

struct AndroidVideoOps;

impl VideoOps for AndroidVideoOps {
    fn play(&self, node: &dyn Any) {
        let Some(host) = node.downcast_ref::<GlobalRef>() else { return };
        start(host);
    }

    fn pause(&self, node: &dyn Any) {
        let Some(host) = node.downcast_ref::<GlobalRef>() else { return };
        with_jni_env(|env| {
            if let Some(vv) = existing_video_view(env, host) {
                let _ = env.call_method(&vv, "pause", "()V", &[]);
            }
        });
    }

    fn seek(&self, node: &dyn Any, seconds: f32) {
        let Some(host) = node.downcast_ref::<GlobalRef>() else { return };
        with_jni_env(|env| {
            if let Some(vv) = existing_video_view(env, host) {
                // VideoView.seekTo(int milliseconds).
                let _ = env.call_method(
                    &vv,
                    "seekTo",
                    "(I)V",
                    &[JValue::Int((seconds * 1000.0) as i32)],
                );
            }
        });
    }
}
