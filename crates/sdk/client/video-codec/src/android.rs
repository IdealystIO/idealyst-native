//! The Android encoder: MediaCodec, fed through its input surface.
//!
//! The stream's native source is a [`PreviewSource`] (the screen-recorder's
//! virtual display, a camera). The encoder claims it and attaches its
//! MediaCodec input surface, so the capture renders straight into the
//! encoder and no pixel reaches this process; only encoded frames come back,
//! through [`RustVideoEncoder`]'s `nativeFrame` on its codec thread.
//!
//! The codec, its callbacks and its teardown live in the Kotlin shim
//! (`runtime/kotlin/io/idealyst/videocodec/RustVideoEncoder.kt`), which needs
//! a `MediaCodec.Callback` subclass.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use jni::objects::{GlobalRef, JByteArray, JClass, JObject, JString, JValue};
use jni::sys::{jboolean, jint, jlong};
use jni::{JNIEnv, JavaVM};
use media_stream::{PreviewClaim, PreviewSource};

use crate::{Codec, EncodedFrame, EncoderConfig, EncoderEvent, EventSink, MediaStream, VideoCodecError};

const HELPER: &str = "io.idealyst.videocodec.RustVideoEncoder";

type Sink = Arc<dyn Fn(EncoderEvent) + Send + Sync>;

static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);
static SINKS: OnceLock<Mutex<HashMap<u64, Sink>>> = OnceLock::new();

fn sinks() -> &'static Mutex<HashMap<u64, Sink>> {
    SINKS.get_or_init(Default::default)
}

pub(crate) struct Encoder {
    token: u64,
    shim: Shim,
    // Dropped first, so the capture stops rendering into the surface before
    // the shim releases it.
    claim: Option<PreviewClaim>,
}

impl Encoder {
    pub(crate) fn request_keyframe(&self) {
        self.shim.call("requestKeyframe", "(J)V", &[JValue::Long(self.token as jlong)]);
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        sinks().lock().unwrap().remove(&self.token);
        drop(self.claim.take());
        self.shim.call("stop", "(J)V", &[JValue::Long(self.token as jlong)]);
    }
}

pub(crate) fn start(
    stream: &MediaStream,
    config: EncoderConfig,
    sink: EventSink,
) -> Result<(Encoder, (u32, u32)), VideoCodecError> {
    let Codec::H264 = config.codec;
    let source = stream
        .native_source()
        .and_then(|native| native.downcast::<PreviewSource>().ok())
        .ok_or(VideoCodecError::NoSurface)?;
    let geometry = source.geometry();
    if geometry.width == 0 || geometry.height == 0 || source.is_refused() {
        return Err(VideoCodecError::NoSurface);
    }
    let claim = source.try_claim().ok_or(VideoCodecError::InUse)?;

    let vm = java_vm()?;
    let mut env = vm.attach_current_thread().map_err(failed)?;
    let shim = Shim::new(&mut env)?;
    let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
    sinks().lock().unwrap().insert(token, Arc::from(sink));

    let class: &JClass = shim.class.as_obj().into();
    let surface = env
        .call_static_method(
            class,
            "start",
            "(JIIII)Landroid/view/Surface;",
            &[
                JValue::Long(token as jlong),
                JValue::Int(geometry.width as jint),
                JValue::Int(geometry.height as jint),
                JValue::Int(config.bitrate as jint),
                JValue::Int(config.fps as jint),
            ],
        )
        .and_then(|v| v.l());
    let surface = match surface {
        Ok(surface) if !surface.is_null() => surface,
        Ok(_) => {
            sinks().lock().unwrap().remove(&token);
            return Err(VideoCodecError::Platform("the encoder gave no input surface".into()));
        }
        Err(e) => {
            let _ = env.exception_clear();
            sinks().lock().unwrap().remove(&token);
            return Err(failed(e));
        }
    };
    // SAFETY: a live local reference to an `android.view.Surface`, used on
    // this thread during this call; the producer keeps its own reference.
    unsafe { claim.attach(surface.as_raw().cast()) };
    let _ = env.delete_local_ref(surface);

    Ok((Encoder { token, shim, claim: Some(claim) }, (geometry.width, geometry.height)))
}

/// The shim's class, resolved through the app's class loader: `FindClass` on
/// a thread Java did not call into (an encoder can be dropped anywhere) sees
/// only system classes.
#[derive(Clone)]
struct Shim {
    class: GlobalRef,
}

impl Shim {
    fn new(env: &mut JNIEnv) -> Result<Self, VideoCodecError> {
        let context = android_context()?;
        let loader = env
            .call_method(&context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
            .and_then(|v| v.l())
            .map_err(failed)?;
        let name = env.new_string(HELPER).map_err(failed)?;
        let class = env
            .call_method(&loader, "loadClass", "(Ljava/lang/String;)Ljava/lang/Class;", &[JValue::Object(&name)])
            .and_then(|v| v.l());
        let class = match class {
            Ok(c) => c,
            Err(e) => {
                let _ = env.exception_clear();
                return Err(VideoCodecError::Platform(format!("{HELPER} is not in the APK: {e}")));
            }
        };
        Ok(Shim { class: env.new_global_ref(class).map_err(failed)? })
    }

    /// A best-effort `(J)V` call: the shim posts the work and cannot fail in a
    /// way the caller could act on.
    fn call(&self, method: &str, sig: &str, args: &[JValue]) {
        let Ok(vm) = java_vm() else { return };
        let Ok(mut env) = vm.attach_current_thread() else { return };
        let class: &JClass = self.class.as_obj().into();
        if env.call_static_method(class, method, sig, args).is_err() {
            // A pending exception would abort the next JNI call on this thread.
            let _ = env.exception_clear();
        }
    }
}

fn java_vm() -> Result<JavaVM, VideoCodecError> {
    let ctx = ndk_context::android_context();
    unsafe { JavaVM::from_raw(ctx.vm().cast()) }.map_err(failed)
}

fn android_context<'a>() -> Result<JObject<'a>, VideoCodecError> {
    let ptr = ndk_context::android_context().context();
    if ptr.is_null() {
        return Err(VideoCodecError::Platform("no Android context".into()));
    }
    // SAFETY: `ndk_context` holds a process-lifetime global reference; this
    // wrapper never deletes it.
    Ok(unsafe { JObject::from_raw(ptr.cast()) })
}

fn failed(e: impl std::fmt::Display) -> VideoCodecError {
    VideoCodecError::Platform(e.to_string())
}

/// A panic must not unwind into the JVM.
fn guarded(what: &str, f: impl FnOnce()) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err() {
        eprintln!("video-codec: panic in {what}; aborting");
        std::process::abort();
    }
}

/// `RustVideoEncoder.nativeFrame`: one encoded frame, on the codec's thread.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_videocodec_RustVideoEncoder_nativeFrame(
    env: JNIEnv,
    _class: JClass,
    token: jlong,
    data: JByteArray,
    pts_us: jlong,
    key: jboolean,
) {
    guarded("nativeFrame", || {
        let sink = sinks().lock().unwrap().get(&(token as u64)).cloned();
        let Some(sink) = sink else { return };
        let Ok(data) = env.convert_byte_array(&data) else { return };
        sink(EncoderEvent::Frame(EncodedFrame { data, pts_us: pts_us.max(0) as u64, key: key != 0 }));
    });
}

/// `RustVideoEncoder.nativeEnded`: the codec failed; no more frames.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_videocodec_RustVideoEncoder_nativeEnded(
    mut env: JNIEnv,
    _class: JClass,
    token: jlong,
    message: JString,
) {
    guarded("nativeEnded", || {
        let message: String =
            if message.is_null() { String::new() } else { env.get_string(&message).map(Into::into).unwrap_or_default() };
        let sink = sinks().lock().unwrap().remove(&(token as u64));
        if let Some(sink) = sink {
            sink(EncoderEvent::Ended(message));
        }
    });
}

// Pin the exports in the app `cdylib`'s dynamic symbol table, where the JVM
// resolves them.
#[used]
static KEEP_NATIVE_FRAME: extern "system" fn(JNIEnv, JClass, jlong, JByteArray, jlong, jboolean) =
    Java_io_idealyst_videocodec_RustVideoEncoder_nativeFrame;
#[used]
static KEEP_NATIVE_ENDED: extern "system" fn(JNIEnv, JClass, jlong, JString) =
    Java_io_idealyst_videocodec_RustVideoEncoder_nativeEnded;
