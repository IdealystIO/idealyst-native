//! Android capture via **MediaProjection** + **VirtualDisplay**, bridged
//! through a Kotlin shim ([`RustScreenCaptureHelper`]) shipped from this crate
//! via `[package.metadata.idealyst.android].runtime_kotlin`.
//!
//! ## Where frames go
//!
//! A whole-screen stream's native source is a [`PreviewSource`]: a consumer
//! (an encoder's input surface, the `video` SDK's `TextureView`) claims it and
//! attaches its own `Surface`, and the virtual display renders straight into
//! it, so frames never pass through this process. CPU frames for subscribers
//! come from an `ImageReader` that exists only while there is
//! [CPU demand](FrameWriter::on_cpu_demand_change), and get none while a
//! consumer is attached: the display renders into one surface at a time.
//!
//! ## Why a Kotlin shim
//!
//! MediaProjection has two pieces of machinery that can't live in raw JNI:
//!
//! 1. **Consent is an Activity result.** The shim's own
//!    `ScreenCaptureConsentActivity` (see-through, in a task of its own)
//!    launches `createScreenCaptureIntent()` and hands back the answer, so an
//!    app capturing from the background does not bring itself forward to ask.
//! 2. **A foreground service is mandatory** on Android 14 (API 34, the
//!    emulator target). `getMediaProjection(resultCode, data)` throws
//!    `SecurityException` unless a service with
//!    `foregroundServiceType="mediaProjection"` has already been
//!    `startForeground(...)`. The shim starts [`MediaProjectionService`] on
//!    consent OK and only calls `getMediaProjection` once it's foregrounded.
//!
//! Both require subclassing/overriding JVM types from Kotlin, so the whole
//! consent→service→capture dance lives in the shim. Its CPU path reads
//! `planes[0]` of each `RGBA_8888` `ImageReader` image honoring `rowStride`
//! (RGBA rows are padded — `rowStride` may exceed `width*4`), packs it tight,
//! and trampolines `width*height*4`-byte frames back through the JNI exports
//! below.
//!
//! ## Async bridge
//!
//! [`start`] mints a `u64` token, parks the [`FrameWriter`] in a
//! process-global registry keyed by that token, parks a oneshot for the
//! start result, and hands the token to the shim. `nativeStarted` /
//! `nativeError` resolve the start future; `nativeFrame` looks the writer up
//! by token and delivers the frame. Dropping the [`Recording`] calls the
//! shim's `stop(token)` (tearing down the virtual display, image reader,
//! projection, and service) and unregisters the writer. This mirrors the
//! `camera` SDK's `open`/`StreamHandle` shape exactly.
//!
//! ## VERIFICATION
//!
//! Compile-checked for `aarch64-linux-android` here, but **not yet
//! device-verified** — the JNI signatures, the Kotlin MediaProjection path,
//! the consent Activity-result round-trip, and the `native*` symbol exports
//! resolve only at runtime on a device/emulator (same posture as the
//! `camera` crate's Android backend). Every failure surfaces as a typed
//! [`RecorderError`] carrying the JNI/Android message. The exports are pinned
//! with `#[used]` so the linker keeps them in the app `cdylib`'s dynsym for
//! `dlsym` resolution by the JVM.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use std::rc::Rc;

use futures_channel::oneshot;
use jni::objects::{GlobalRef, JByteBuffer, JClass, JObject, JString, JValue};
use jni::sys::{jboolean, jint, jlong};
use jni::{JNIEnv, JavaVM};

use crate::{NativeSource, RecorderError, RecordingConfig, Source};
use media_stream::{preview_channel, FrameWriter, PreviewDriver, PreviewGeometry, PreviewSource};

const HELPER: &str = "io.idealyst.screenrecorder.RustScreenCaptureHelper";

/// The sender that resolves an awaiting `start()` once the shim reports
/// capture up (or failed), with the size frames come out at (0 × 0 for the
/// app-only source). Negative `code`s are shim-side sentinels; a
/// user-declined consent maps to [`RecorderError::PermissionDenied`].
type StartSender = oneshot::Sender<Result<(u32, u32), RecorderError>>;

/// token → frame writer, for frames in flight. The JNI trampoline clones the
/// `Send` writer out from under the registry lock and pushes into it without
/// holding the global lock across the channel fan-out (a subscriber may touch
/// the stream — e.g. drop it — re-entering this module).
static WRITERS: OnceLock<Mutex<HashMap<u64, FrameWriter>>> = OnceLock::new();
/// token → the sender awaiting the capture-start result.
static PENDING_START: OnceLock<Mutex<HashMap<u64, StartSender>>> = OnceLock::new();
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

/// Sentinel `code` the shim sends through `nativeError` when the user
/// declined the MediaProjection consent dialog (Activity result wasn't
/// `RESULT_OK`). Kept in sync with `RustScreenCaptureHelper.ERR_DENIED`.
const ERR_DENIED: jint = -10;

fn writers() -> &'static Mutex<HashMap<u64, FrameWriter>> {
    WRITERS.get_or_init(|| Mutex::new(HashMap::new()))
}
fn pending_start() -> &'static Mutex<HashMap<u64, StartSender>> {
    PENDING_START.get_or_init(|| Mutex::new(HashMap::new()))
}

/// No pre-prompt on Android: the MediaProjection consent dialog is shown by
/// [`start`] (it can't be requested ahead of time — the consent token is
/// single-use and tied to the session it's granted for). Resolving `Ok` here
/// just defers consent to that call, the same shape as iOS ReplayKit and the
/// web `getDisplayMedia` picker.
pub(crate) async fn request_permission(_source: &Source) -> Result<(), RecorderError> {
    Ok(())
}

/// Whether consent is already given, so [`start`] shows nobody a prompt.
pub(crate) fn consent_pregranted() -> bool {
    let Ok(vm) = java_vm() else { return false };
    let Ok(mut env) = vm.attach_current_thread() else { return false };
    let Ok(shim) = Shim::new(&mut env) else { return false };
    let context = android_context();
    let class: &JClass = shim.class.as_obj().into();
    let granted = env
        .call_static_method(class, "consentPregranted", "(Landroid/content/Context;)Z", &[JValue::Object(&context)])
        .and_then(|v| v.z());
    granted.unwrap_or_else(|_| {
        let _ = env.exception_clear();
        false
    })
}

/// The default display's real size in pixels.
pub(crate) fn display_size() -> Option<(u32, u32)> {
    let vm = java_vm().ok()?;
    let mut env = vm.attach_current_thread().ok()?;
    let shim = Shim::new(&mut env).ok()?;
    let context = android_context();
    let class: &JClass = shim.class.as_obj().into();
    let packed = env
        .call_static_method(class, "displaySize", "(Landroid/content/Context;)I", &[JValue::Object(&context)])
        .and_then(|v| v.i());
    match packed {
        Ok(p) => Some(((p as u32) >> 16, (p as u32) & 0xffff)),
        Err(_) => {
            let _ = env.exception_clear();
            None
        }
    }
}

pub(crate) async fn start(
    config: RecordingConfig,
    writer: FrameWriter,
) -> Result<(Recording, Option<NativeSource>), RecorderError> {
    // A specific *other* window can't be captured on Android — MediaProjection
    // mirrors a whole display, not an arbitrary app window.
    if matches!(config.source, Source::Window(_)) {
        return Err(RecorderError::UnsupportedSource("window"));
    }

    let vm = java_vm()?;
    let mut env = vm.attach_current_thread().map_err(jni_err)?;
    let activity = android_context();
    let shim = Shim::new(&mut env)?;
    // The app-only source copies its window at the window's size.
    let (width, height) = match config.source {
        Source::ThisApp => (0, 0),
        _ => config.output_size(
            display_size().ok_or_else(|| RecorderError::Platform("could not read the display's size".into()))?,
        ),
    };

    let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
    // The CPU path runs only while someone wants its frames.
    let demand_shim = shim.clone();
    writer.on_cpu_demand_change(move |wants| {
        demand_shim.call("setCpuDemand", "(JZ)V", &[JValue::Long(token as jlong), JValue::Bool(wants as jboolean)]);
    });
    writers().lock().unwrap().insert(token, writer);
    let (tx, rx) = oneshot::channel::<Result<(u32, u32), RecorderError>>();
    pending_start().lock().unwrap().insert(token, tx);

    // Source discriminant the shim branches on: `ThisApp` (0) → app-only
    // PixelCopy of our own window (no consent, no foreground service, matches
    // iOS ReplayKit); everything else → whole-screen MediaProjection.
    let source = match config.source {
        Source::ThisApp => 0,
        Source::FullScreen => 1,
        Source::UserChoice => 2,
        // Window was rejected above.
        Source::Window(_) => 2,
    };

    // Kick off capture. For `ThisApp` the shim starts the PixelCopy loop and
    // fires `nativeStarted` immediately; for the screen sources it launches the
    // consent dialog and the result arrives via nativeStarted/nativeError once
    // the user responds and the projection is up. Either way the shim returns
    // immediately.
    let class: &JClass = shim.class.as_obj().into();
    let launch = env.call_static_method(
        class,
        "start",
        "(Landroid/content/Context;JIII)V",
        &[
            JValue::Object(&activity),
            JValue::Long(token as jlong),
            JValue::Int(source),
            JValue::Int(width as jint),
            JValue::Int(height as jint),
        ],
    );
    if let Err(e) = launch {
        // The shim never got a chance to call back — clean up the registries.
        writers().lock().unwrap().remove(&token);
        pending_start().lock().unwrap().remove(&token);
        return Err(jni_err(e));
    }

    match rx.await {
        // The app-only source copies its own window and has no surface to give.
        Ok(Ok((0, _) | (_, 0))) => Ok((Recording { token, shim }, None)),
        Ok(Ok((width, height))) => {
            let geometry = PreviewGeometry { width, height, sensor_orientation: 0, front_facing: false };
            let driver = ScreenPreviewDriver { shim: shim.clone(), token };
            let (source, _producer): (PreviewSource, _) = preview_channel(geometry, driver);
            Ok((Recording { token, shim }, Some(Rc::new(source) as NativeSource)))
        }
        Ok(Err(e)) => {
            writers().lock().unwrap().remove(&token);
            Err(e)
        }
        Err(_) => {
            writers().lock().unwrap().remove(&token);
            Err(RecorderError::Platform(
                "screen-capture start channel dropped".into(),
            ))
        }
    }
}

/// A way to reach the shim from any thread. `FindClass` on a thread Java did
/// not call into (a claim can be dropped anywhere) sees only system classes,
/// so the class is resolved once through the app's class loader and held.
#[derive(Clone)]
struct Shim {
    class: GlobalRef,
}

impl Shim {
    fn new(env: &mut JNIEnv) -> Result<Self, RecorderError> {
        let context = android_context();
        let loader = env
            .call_method(&context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
            .and_then(|v| v.l())
            .map_err(jni_err)?;
        let name = env.new_string(HELPER).map_err(jni_err)?;
        let class = env
            .call_method(&loader, "loadClass", "(Ljava/lang/String;)Ljava/lang/Class;", &[JValue::Object(&name)])
            .and_then(|v| v.l());
        let class = match class {
            Ok(c) => c,
            Err(e) => {
                let _ = env.exception_clear();
                return Err(jni_err(e));
            }
        };
        Ok(Shim { class: env.new_global_ref(class).map_err(jni_err)? })
    }

    /// Call a static method on the shim. Best effort: each of these posts to
    /// the main thread and cannot fail in a way a caller could act on, and a
    /// session already gone is simply not found there.
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

/// Carries a consumer's attach and detach to the shim for one recording.
struct ScreenPreviewDriver {
    shim: Shim,
    token: u64,
}

impl PreviewDriver for ScreenPreviewDriver {
    unsafe fn attach(&self, surface: *mut std::ffi::c_void) {
        // SAFETY: per the trait, `surface` is a live `Surface` reference for this
        // call on this thread. `JObject` does not delete what it wraps, and the
        // shim keeps its own reference.
        let surface = unsafe { JObject::from_raw(surface.cast()) };
        self.shim.call(
            "attachSurface",
            "(JLandroid/view/Surface;)V",
            &[JValue::Long(self.token as jlong), JValue::Object(&surface)],
        );
    }

    fn detach(&self) {
        self.shim.call("detachSurface", "(J)V", &[JValue::Long(self.token as jlong)]);
    }
}

/// A live MediaProjection recording. Dropping it tells the shim to tear down
/// the virtual display + image reader + projection + foreground service, and
/// unregisters the frame writer so no late frame reaches freed state — the
/// `MediaStream` stopper owns this, so the last stream clone dropping tears
/// capture down (same lifecycle as `camera`).
pub(crate) struct Recording {
    token: u64,
    shim: Shim,
}

impl Drop for Recording {
    fn drop(&mut self) {
        self.shim.call("stop", "(J)V", &[JValue::Long(self.token as jlong)]);
        writers().lock().unwrap().remove(&self.token);
        pending_start().lock().unwrap().remove(&self.token);
    }
}

// ---------------------------------------------------------------------------
// JNI helpers — identical posture to the `camera` SDK's android.rs.
// ---------------------------------------------------------------------------

fn java_vm() -> Result<JavaVM, RecorderError> {
    let ctx = ndk_context::android_context();
    let vm_ptr = ctx.vm() as *mut jni::sys::JavaVM;
    unsafe { JavaVM::from_raw(vm_ptr) }
        .map_err(|e| RecorderError::Platform(format!("invalid JavaVM pointer: {e}")))
}

fn jni_err(e: jni::errors::Error) -> RecorderError {
    RecorderError::Platform(format!("JNI: {e}"))
}

fn android_context<'a>() -> JObject<'a> {
    let ctx = ndk_context::android_context();
    unsafe { JObject::from_raw(ctx.context().cast()) }
}

/// Map the shim's error `(code, message)` to a typed [`RecorderError`].
/// `ERR_DENIED` is the consent-declined sentinel; everything else is a
/// platform diagnostic carrying the shim's message.
fn map_start_error(code: jint, message: Option<String>) -> RecorderError {
    match code {
        ERR_DENIED => RecorderError::PermissionDenied,
        _ => RecorderError::Platform(format!(
            "screen capture failed (code {code}): {}",
            message.unwrap_or_default()
        )),
    }
}

// ---------------------------------------------------------------------------
// JNI exports — the Kotlin shim's trampolines. Each is wrapped in
// `catch_unwind` + log + `abort()` so a panic can never unwind across the FFI
// boundary (the crash-loud-on-JNI-panic policy; copied from `camera`).
// ---------------------------------------------------------------------------

/// `RustScreenCaptureHelper.nativeStarted` — the projection and its virtual
/// display are up at `width` × `height` (0 × 0: the app-only source).
///
/// # Safety
/// Called by the JVM with a valid `env`/`class`.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_screenrecorder_RustScreenCaptureHelper_nativeStarted(
    _env: JNIEnv,
    _class: JClass,
    token: jlong,
    width: jint,
    height: jint,
) {
    let result = std::panic::catch_unwind(|| {
        if let Some(tx) = pending_start().lock().unwrap().remove(&(token as u64)) {
            let _ = tx.send(Ok((width.max(0) as u32, height.max(0) as u32)));
        }
    });
    if result.is_err() {
        eprintln!("screen-recorder: panic in nativeStarted trampoline; aborting");
        std::process::abort();
    }
}

/// `RustScreenCaptureHelper.nativeError` — consent was declined or the
/// projection failed to start. Resolves the start future with a typed error.
///
/// # Safety
/// Called by the JVM with a valid `env`/`class`; `message` is a `String?`.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_screenrecorder_RustScreenCaptureHelper_nativeError(
    mut env: JNIEnv,
    _class: JClass,
    token: jlong,
    code: jint,
    message: JString,
) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let message = if message.is_null() {
            None
        } else {
            env.get_string(&message).ok().map(|s| s.into())
        };
        let err = map_start_error(code, message);
        if let Some(tx) = pending_start().lock().unwrap().remove(&(token as u64)) {
            let _ = tx.send(Err(err));
        }
    }));
    if result.is_err() {
        eprintln!("screen-recorder: panic in nativeError trampoline; aborting");
        std::process::abort();
    }
}

/// `RustScreenCaptureHelper.nativeFrameDirect` — one captured frame in a
/// REUSED direct `ByteBuffer`. `buffer` is tightly-packed top-down `RGBA8`
/// of `width * height * 4` bytes (the shim strips the ImageReader plane's
/// `rowStride` padding) in off-heap memory; we read it ZERO-COPY via
/// `GetDirectBufferAddress` — no `byte[]` marshal and no Rust `Vec` alloc
/// per frame. Delivered on the shim's capture handler thread.
///
/// # Safety
/// Called by the JVM with a valid `env`/`class`; `buffer` is a direct
/// `java.nio.ByteBuffer` that stays alive and unmodified for the duration of
/// this synchronous call.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_screenrecorder_RustScreenCaptureHelper_nativeFrameDirect(
    env: JNIEnv,
    _class: JClass,
    token: jlong,
    buffer: JByteBuffer,
    width: jint,
    height: jint,
) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if width <= 0 || height <= 0 {
            return;
        }
        // Clone the frame writer out from under the registry lock, then push
        // into it without holding the global lock across the channel fan-out
        // (a subscriber may itself touch the stream, e.g. drop it).
        let writer = writers().lock().unwrap().get(&(token as u64)).cloned();
        let Some(writer) = writer else {
            return;
        };

        let needed = (width as usize) * (height as usize) * 4;
        let addr = match env.get_direct_buffer_address(&buffer) {
            Ok(p) if !p.is_null() => p,
            _ => return,
        };
        if env.get_direct_buffer_capacity(&buffer).unwrap_or(0) < needed {
            return;
        }
        // SAFETY: `addr` points at the off-heap region of a live direct
        // ByteBuffer whose capacity we just checked is >= `needed`; we read
        // exactly `needed` bytes during this synchronous call.
        let bytes = unsafe { std::slice::from_raw_parts(addr, needed) };
        // The shim is the authority on packing (tight RGBA8); a mismatched
        // length `write_rgba8` will reject anyway.
        writer.write_rgba8(width as u32, height as u32, bytes);
    }));
    if result.is_err() {
        eprintln!("screen-recorder: panic in nativeFrameDirect trampoline; aborting");
        std::process::abort();
    }
}

// Pin the exports so the linker keeps them in the app `cdylib`'s dynamic
// symbol table (the JVM resolves them by `dlsym`).
#[used]
static KEEP_NATIVE_STARTED: extern "system" fn(JNIEnv, JClass, jlong, jint, jint) =
    Java_io_idealyst_screenrecorder_RustScreenCaptureHelper_nativeStarted;
#[used]
static KEEP_NATIVE_ERROR: extern "system" fn(JNIEnv, JClass, jlong, jint, JString) =
    Java_io_idealyst_screenrecorder_RustScreenCaptureHelper_nativeError;
#[used]
static KEEP_NATIVE_FRAME: extern "system" fn(JNIEnv, JClass, jlong, JByteBuffer, jint, jint) =
    Java_io_idealyst_screenrecorder_RustScreenCaptureHelper_nativeFrameDirect;
