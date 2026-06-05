//! Android capture via **Camera2** + **ImageReader**, bridged through a
//! Kotlin shim ([`RustCamera2Helper`]) shipped from this crate via
//! `[package.metadata.idealyst.android].runtime_kotlin`.
//!
//! ## Why a Kotlin shim
//!
//! Unlike `AudioRecord` (a synchronous blocking `read()` the `microphone`
//! SDK drives entirely from JNI), Camera2 is callback-driven: opening a
//! camera and receiving frames requires subclassing
//! `CameraDevice.StateCallback`, `CameraCaptureSession.StateCallback`, and
//! `ImageReader.OnImageAvailableListener`, and the open must be issued on a
//! looper thread. Subclassing abstract Java callbacks purely from JNI isn't
//! feasible, so that machinery lives in a tiny Kotlin shim. The shim opens
//! the camera, converts each `YUV_420_888` image to tightly-packed `RGBA8`,
//! and trampolines frames + lifecycle back through the JNI exports below.
//!
//! ## Async bridge
//!
//! [`open`] mints a `u64` token, parks the frame callback in a process-global
//! registry keyed by that token, parks a oneshot for the open result, and
//! hands the token to the shim. `nativeOpened`/`nativeError` resolve the
//! open future; `nativeFrame` looks the callback up by token and delivers
//! the frame. Dropping the [`StreamHandle`] calls the shim's `close(token)`
//! and unregisters the callback.
//!
//! ## VERIFICATION
//!
//! Compile-checked for `aarch64-linux-android` here, but **not yet
//! device-verified** — the JNI signatures, the Kotlin Camera2 path, and the
//! `native*` symbol exports resolve only at runtime on a device (same
//! posture as the `biometrics` crate's Android backend). Every failure is
//! surfaced as a typed [`CameraError`] carrying the JNI/Android message to
//! make that diagnosis quick. The exports are pinned with `#[used]` so the
//! linker keeps them in the app `cdylib`'s dynsym for `dlsym` resolution by
//! the JVM.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use futures_channel::oneshot;
use jni::objects::{JByteBuffer, JClass, JObject, JString, JValue};
use jni::sys::{jint, jlong};
use jni::{JNIEnv, JavaVM};

use crate::{CameraConfig, CameraError, CameraFacing, NativeSource};
use media_stream::FrameWriter;

const CAMERA_PERMISSION: &str = "android.permission.CAMERA";
const PERMISSION_GRANTED: i32 = 0; // PackageManager.PERMISSION_GRANTED

// CameraConfig::facing as the int the Kotlin shim understands.
const FACING_DEFAULT: i32 = 0;
const FACING_FRONT: i32 = 1;
const FACING_BACK: i32 = 2;

/// The sender that resolves an awaiting `open()` once the shim reports the
/// camera up (or failed).
type OpenSender = oneshot::Sender<Result<(), CameraError>>;

/// token → frame writer, for frames in flight. The JNI trampoline clones the
/// `Send` writer out from under the registry lock and pushes into it without
/// holding the global lock across the channel fan-out.
static WRITERS: OnceLock<Mutex<HashMap<u64, FrameWriter>>> = OnceLock::new();
/// token → the sender awaiting the camera-open result.
static PENDING_OPEN: OnceLock<Mutex<HashMap<u64, OpenSender>>> = OnceLock::new();
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

fn writers() -> &'static Mutex<HashMap<u64, FrameWriter>> {
    WRITERS.get_or_init(|| Mutex::new(HashMap::new()))
}
fn pending_open() -> &'static Mutex<HashMap<u64, OpenSender>> {
    PENDING_OPEN.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Stops the stream on drop: tells the shim to tear down its Camera2 session
/// and unregisters the frame callback.
pub(crate) struct StreamHandle {
    token: u64,
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        // Best-effort close via the shim, then drop the callback so no late
        // frame can reach freed state.
        if let Ok(vm) = java_vm() {
            if let Ok(mut env) = vm.attach_current_thread() {
                let _ = env.call_static_method(
                    "io/idealyst/camera/RustCamera2Helper",
                    "close",
                    "(J)V",
                    &[JValue::Long(self.token as jlong)],
                );
            }
        }
        writers().lock().unwrap().remove(&self.token);
        pending_open().lock().unwrap().remove(&self.token);
    }
}

pub(crate) async fn request_permission() -> Result<(), CameraError> {
    let vm = java_vm()?;
    let mut env = vm.attach_current_thread().map_err(jni_err)?;
    let activity = android_context();

    if check_self_permission(&mut env, &activity)? {
        return Ok(());
    }

    // Not yet granted: surface the runtime dialog. Its result arrives in the
    // Activity's `onRequestPermissionsResult`, which this SDK doesn't hook —
    // so we fire the request and report the current (not-granted) state. The
    // caller re-checks (or retries `open`) after the user responds. Same
    // posture as `microphone`. Documented in the README.
    let perm = env.new_string(CAMERA_PERMISSION).map_err(jni_err)?;
    let arr = env
        .new_object_array(1, "java/lang/String", &perm)
        .map_err(jni_err)?;
    let _ = env.call_method(
        &activity,
        "requestPermissions",
        "([Ljava/lang/String;I)V",
        &[JValue::Object(&JObject::from(arr)), JValue::Int(0)],
    );
    Err(CameraError::PermissionDenied)
}

pub(crate) async fn open(
    config: CameraConfig,
    writer: FrameWriter,
) -> Result<(StreamHandle, Option<NativeSource>), CameraError> {
    let vm = java_vm()?;
    let mut env = vm.attach_current_thread().map_err(jni_err)?;
    let activity = android_context();

    // Ensure the CAMERA runtime permission. On first use this fires the system
    // dialog and returns `PermissionDenied` (the dialog's result has no SDK
    // callback, so the user re-invokes `open` once granted). WITHOUT this, a
    // not-yet-granted permission just fails silently and the camera never opens
    // — and no dialog is ever shown ("nothing happens" on Android).
    request_permission().await?;

    let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
    writers().lock().unwrap().insert(token, writer);
    let (tx, rx) = oneshot::channel::<Result<(), CameraError>>();
    pending_open().lock().unwrap().insert(token, tx);

    let facing = match config.facing {
        CameraFacing::Default => FACING_DEFAULT,
        CameraFacing::Front => FACING_FRONT,
        CameraFacing::Back => FACING_BACK,
    };
    // 0 = "device default" for width/height/fps; the shim picks a supported
    // size/rate when a dimension is 0.
    let launch = env.call_static_method(
        "io/idealyst/camera/RustCamera2Helper",
        "open",
        "(Landroid/content/Context;IIIIJ)V",
        &[
            JValue::Object(&activity),
            JValue::Int(facing),
            JValue::Int(config.width.unwrap_or(0) as jint),
            JValue::Int(config.height.unwrap_or(0) as jint),
            JValue::Int(config.fps.unwrap_or(0) as jint),
            JValue::Long(token as jlong),
        ],
    );
    if let Err(e) = launch {
        // The shim never got a chance to call back — clean up the registries.
        writers().lock().unwrap().remove(&token);
        pending_open().lock().unwrap().remove(&token);
        return Err(jni_err(e));
    }

    match rx.await {
        // Android exposes no zero-copy native source yet (camera→SurfaceTexture
        // is the GPU-pipeline phase); frames flow through the CPU channel.
        Ok(Ok(())) => Ok((StreamHandle { token }, None)),
        Ok(Err(e)) => {
            writers().lock().unwrap().remove(&token);
            Err(e)
        }
        Err(_) => {
            writers().lock().unwrap().remove(&token);
            Err(CameraError::Backend("camera open channel dropped".into()))
        }
    }
}

// ---------------------------------------------------------------------------
// JNI helpers
// ---------------------------------------------------------------------------

fn java_vm() -> Result<JavaVM, CameraError> {
    let ctx = ndk_context::android_context();
    let vm_ptr = ctx.vm() as *mut jni::sys::JavaVM;
    unsafe { JavaVM::from_raw(vm_ptr) }
        .map_err(|e| CameraError::Backend(format!("invalid JavaVM pointer: {e}")))
}

fn jni_err(e: jni::errors::Error) -> CameraError {
    CameraError::Backend(format!("JNI: {e}"))
}

fn android_context<'a>() -> JObject<'a> {
    let ctx = ndk_context::android_context();
    unsafe { JObject::from_raw(ctx.context().cast()) }
}

/// `context.checkSelfPermission("android.permission.CAMERA") == GRANTED`.
fn check_self_permission(
    env: &mut JNIEnv<'_>,
    activity: &JObject<'_>,
) -> Result<bool, CameraError> {
    let perm = env.new_string(CAMERA_PERMISSION).map_err(jni_err)?;
    let result = env
        .call_method(
            activity,
            "checkSelfPermission",
            "(Ljava/lang/String;)I",
            &[JValue::Object(&JObject::from(perm))],
        )
        .map_err(jni_err)?
        .i()
        .map_err(jni_err)?;
    Ok(result == PERMISSION_GRANTED)
}

/// Map the shim's error `(code, message)` to a typed [`CameraError`]. `code`
/// is a Camera2 `CameraDevice.StateCallback.ERROR_*` value, or a negative
/// sentinel for a shim-side exception / "no matching camera".
fn map_open_error(code: jint, message: Option<String>) -> CameraError {
    const SENTINEL_NO_CAMERA: jint = -2;
    match code {
        SENTINEL_NO_CAMERA => CameraError::NoCamera,
        // CameraDevice.StateCallback.ERROR_CAMERA_IN_USE / MAX_IN_USE / DISABLED.
        1..=3 => CameraError::Backend(format!(
            "camera unavailable (code {code}): {}",
            message.unwrap_or_default()
        )),
        _ => CameraError::Backend(format!(
            "camera open failed (code {code}): {}",
            message.unwrap_or_default()
        )),
    }
}

// ---------------------------------------------------------------------------
// JNI exports — the Kotlin shim's trampolines.
// ---------------------------------------------------------------------------

/// `RustCamera2Helper.nativeOpened` — the session is live and streaming.
///
/// # Safety
/// Called by the JVM with a valid `env`/`class`.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_camera_RustCamera2Helper_nativeOpened(
    _env: JNIEnv,
    _class: JClass,
    token: jlong,
) {
    let result = std::panic::catch_unwind(|| {
        if let Some(tx) = pending_open().lock().unwrap().remove(&(token as u64)) {
            let _ = tx.send(Ok(()));
        }
    });
    if result.is_err() {
        eprintln!("camera: panic in nativeOpened trampoline; aborting");
        std::process::abort();
    }
}

/// `RustCamera2Helper.nativeError` — the camera failed to open (or died
/// before configuring). Resolves the open future with a typed error.
///
/// # Safety
/// Called by the JVM with a valid `env`/`class`; `message` is a `String?`.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_camera_RustCamera2Helper_nativeError(
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
        let err = map_open_error(code, message);
        if let Some(tx) = pending_open().lock().unwrap().remove(&(token as u64)) {
            let _ = tx.send(Err(err));
        }
    }));
    if result.is_err() {
        eprintln!("camera: panic in nativeError trampoline; aborting");
        std::process::abort();
    }
}

/// `RustCamera2Helper.nativeFrameDirect` — one converted frame in a
/// REUSED direct `ByteBuffer`. `buffer` holds tightly-packed top-down
/// `RGBA8` of `width * height * 4` bytes in off-heap memory the Kotlin
/// side reuses across frames; we read it ZERO-COPY via
/// `GetDirectBufferAddress` — no `byte[]` marshal across JNI and no
/// per-frame Rust `Vec` allocation (the old `convert_byte_array` path
/// did both, ~8 MB/frame each at 1080p). Delivered on the shim's
/// ImageReader handler thread.
///
/// # Safety
/// Called by the JVM with a valid `env`/`class`; `buffer` is a direct
/// `java.nio.ByteBuffer` that stays alive and unmodified for the
/// duration of this synchronous call (the Kotlin side only reuses it
/// after `nativeFrameDirect` returns).
#[no_mangle]
pub extern "system" fn Java_io_idealyst_camera_RustCamera2Helper_nativeFrameDirect(
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
        // Guard the read against a short buffer (a malformed frame); the
        // capacity query is cheap and keeps the `from_raw_parts` in bounds.
        if env.get_direct_buffer_capacity(&buffer).unwrap_or(0) < needed {
            return;
        }
        // SAFETY: `addr` points at the off-heap region of a live direct
        // ByteBuffer whose capacity we just checked is >= `needed`; we read
        // exactly `needed` bytes during this synchronous call, before the
        // Kotlin side reuses the buffer for the next frame.
        let bytes = unsafe { std::slice::from_raw_parts(addr, needed) };
        // The shim's YUV→RGBA converter is the authority on packing; a
        // mismatch means a malformed frame `write_rgba8` will reject anyway.
        writer.write_rgba8(width as u32, height as u32, bytes);
    }));
    if result.is_err() {
        eprintln!("camera: panic in nativeFrameDirect trampoline; aborting");
        std::process::abort();
    }
}

// Pin the exports so the linker keeps them in the app `cdylib`'s dynamic
// symbol table (the JVM resolves them by `dlsym`).
#[used]
static KEEP_NATIVE_OPENED: extern "system" fn(JNIEnv, JClass, jlong) =
    Java_io_idealyst_camera_RustCamera2Helper_nativeOpened;
#[used]
static KEEP_NATIVE_ERROR: extern "system" fn(JNIEnv, JClass, jlong, jint, JString) =
    Java_io_idealyst_camera_RustCamera2Helper_nativeError;
#[used]
static KEEP_NATIVE_FRAME: extern "system" fn(JNIEnv, JClass, jlong, JByteBuffer, jint, jint) =
    Java_io_idealyst_camera_RustCamera2Helper_nativeFrameDirect;
