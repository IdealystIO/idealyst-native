//! Android capture via **Camera2**, into a display's `Surface` and an
//! **ImageReader**, bridged through a Kotlin shim ([`RustCamera2Helper`]) shipped
//! from this crate via `[package.metadata.idealyst.android].runtime_kotlin`.
//!
//! ## Why a Kotlin shim
//!
//! Unlike `AudioRecord` (a synchronous blocking `read()` the `microphone`
//! SDK drives entirely from JNI), Camera2 is callback-driven: opening a
//! camera and receiving frames requires subclassing
//! `CameraDevice.StateCallback`, `CameraCaptureSession.StateCallback`, and
//! `ImageReader.OnImageAvailableListener`, and the open must be issued on a
//! looper thread. Subclassing abstract Java callbacks purely from JNI isn't
//! feasible, so that machinery lives in a small Kotlin shim. The shim opens
//! the camera and trampolines each `YUV_420_888` image's planes + lifecycle
//! back through the JNI exports below.
//!
//! ## Preview
//!
//! The stream's `native_source` is a `PreviewSource`: a display claims it and
//! hands over a `Surface`, and Camera2 fills that surface with no copy into this
//! process. The session holds that surface and the `ImageReader`, and the repeating
//! request ([`crate::android_preview`]) moves between them on demand. The rules for
//! how a surface gets into a session whose outputs were fixed at creation are in
//! the shim's docs. A device that refuses the combination reports back through
//! [`nativePreviewRefused`], and its displays fall back to the CPU frames below.
//!
//! No frame reaches this process while nothing wants one, so a demand change is a
//! call into the shim ([`FrameWriter::on_cpu_demand_change`]) rather than a check
//! made per frame, as web and Apple can.
//!
//! [`nativePreviewRefused`]: Java_io_idealyst_camera_RustCamera2Helper_nativePreviewRefused
//!
//! ## Frames
//!
//! The shim passes the plane buffers of an open `Image` to [`nativeFrameYuv`],
//! which views them as slices for that one call and converts them to `RGBA8`
//! straight into the stream's reused frame buffer ([`crate::android_yuv`]). A
//! per-pixel loop over `ByteBuffer.get`/`put` in Kotlin held a 1080p stream to
//! about 1 FPS on a kiosk tablet, a full core on the capture thread.
//!
//! [`nativeFrameYuv`]: Java_io_idealyst_camera_RustCamera2Helper_nativeFrameYuv
//!
//! ## Async bridge
//!
//! [`open`] mints a `u64` token, parks the frame callback in a process-global
//! registry keyed by that token, parks a oneshot for the open result, and
//! hands the token to the shim. `nativeOpened`/`nativeError` resolve the
//! open future; `nativeFrameYuv` looks the writer up by token and delivers
//! the frame. Dropping the [`StreamHandle`] calls the shim's `close(token)`
//! and unregisters the callback. A `nativeError` after the open has resolved means
//! the device went away: the writer ends, releasing anyone waiting on a frame.
//!
//! ## VERIFICATION
//!
//! Compile-checked for `aarch64-linux-android` here, but **not yet
//! device-verified** — the JNI signatures, the Kotlin Camera2 path, and the
//! `native*` symbol exports resolve only at runtime on a device (same
//! posture as the `biometrics` crate's Android backend). `javap` checks each JNI
//! descriptor against the compiled shim, and every failure is
//! surfaced as a typed [`CameraError`] carrying the JNI/Android message to
//! make that diagnosis quick. The exports are pinned with `#[used]` so the
//! linker keeps them in the app `cdylib`'s dynsym for `dlsym` resolution by
//! the JVM.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use futures_channel::oneshot;
use jni::objects::{GlobalRef, JByteBuffer, JClass, JIntArray, JObject, JObjectArray, JString, JValue};
use jni::sys::{jboolean, jint, jlong, JNI_FALSE};
use jni::{JNIEnv, JavaVM};

use crate::android_yuv::{Plane, Rotation, YuvFrame};
use crate::{CameraConfig, CameraDevice, CameraError, CameraFacing, NativeSource};
use media_stream::{preview_channel, FrameWriter, PreviewDriver, PreviewGeometry, PreviewProducer};

// CameraConfig::facing as the int the Kotlin shim understands.
const FACING_DEFAULT: i32 = 0;
const FACING_FRONT: i32 = 1;
const FACING_BACK: i32 = 2;

const HELPER: &str = "io/idealyst/camera/RustCamera2Helper";

/// The sender that resolves an awaiting `open()` once the shim reports the
/// camera up, with the preview's geometry (or failed).
type OpenSender = oneshot::Sender<Result<PreviewGeometry, CameraError>>;

/// token → frame writer, for frames in flight. The JNI trampoline clones the
/// `Send` writer out from under the registry lock and pushes into it without
/// holding the global lock across the channel fan-out.
static WRITERS: OnceLock<Mutex<HashMap<u64, FrameWriter>>> = OnceLock::new();
/// token → the sender awaiting the camera-open result.
static PENDING_OPEN: OnceLock<Mutex<HashMap<u64, OpenSender>>> = OnceLock::new();
/// token → the preview's producer half, for the shim's refusal report.
static PREVIEWS: OnceLock<Mutex<HashMap<u64, PreviewProducer>>> = OnceLock::new();
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

fn writers() -> &'static Mutex<HashMap<u64, FrameWriter>> {
    WRITERS.get_or_init(|| Mutex::new(HashMap::new()))
}
fn pending_open() -> &'static Mutex<HashMap<u64, OpenSender>> {
    PENDING_OPEN.get_or_init(|| Mutex::new(HashMap::new()))
}
fn previews() -> &'static Mutex<HashMap<u64, PreviewProducer>> {
    PREVIEWS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A way to reach the shim from any thread. `FindClass` searches the class loader
/// of the Java caller on the stack, so from a thread Java did not call into (a
/// `Subscription` can be dropped anywhere) it finds only system classes. The class
/// is resolved once on `open`'s thread, which runs under the app's loader, and held.
#[derive(Clone)]
struct Shim {
    class: GlobalRef,
}

impl Shim {
    fn new(env: &mut JNIEnv) -> Result<Self, CameraError> {
        let class = env.find_class(HELPER).map_err(jni_err)?;
        let class = env.new_global_ref(class).map_err(jni_err)?;
        Ok(Shim { class })
    }

    /// Call a static `(J…)V` method on the shim. Best effort: each of these posts to
    /// the capture thread and cannot fail in a way a caller could act on, and a
    /// stream already closed is simply not found there.
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

/// Carries a display's attach and detach to the shim for one camera.
struct CameraPreviewDriver {
    shim: Shim,
    token: u64,
}

impl PreviewDriver for CameraPreviewDriver {
    unsafe fn attach(&self, surface: *mut std::ffi::c_void) {
        // SAFETY: per the trait, `surface` is a live `Surface` reference for this
        // call on this thread. `JObject` does not delete what it wraps, and the
        // shim keeps its own reference.
        let surface = unsafe { JObject::from_raw(surface.cast()) };
        self.shim.call(
            "attachPreview",
            "(JLandroid/view/Surface;)V",
            &[JValue::Long(self.token as jlong), JValue::Object(&surface)],
        );
    }

    fn detach(&self) {
        self.shim.call("detachPreview", "(J)V", &[JValue::Long(self.token as jlong)]);
    }
}

/// Stops the stream on drop: tells the shim to tear down its Camera2 session
/// and unregisters the frame callback.
pub(crate) struct StreamHandle {
    token: u64,
    shim: Shim,
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        // Best-effort close via the shim, then drop the callback so no late
        // frame can reach freed state.
        self.shim.call("close", "(J)V", &[JValue::Long(self.token as jlong)]);
        forget(self.token);
    }
}

/// Clear a stream's entries from every registry.
fn forget(token: u64) {
    writers().lock().unwrap().remove(&token);
    pending_open().lock().unwrap().remove(&token);
    previews().lock().unwrap().remove(&token);
}

/// Ensure the CAMERA runtime permission, delegated to the shared
/// `permissions` SDK (which owns the `checkSelfPermission` /
/// `Activity.requestPermissions` grant flow). On first use this waits on the
/// system dialog and resolves with the user's answer.
pub(crate) async fn request_permission() -> Result<(), CameraError> {
    let status = permissions::request(permissions::Permission::Camera).await;
    if status.is_usable() {
        Ok(())
    } else {
        Err(CameraError::PermissionDenied)
    }
}

/// Every Camera2 camera, from the shim's `listCameras` (`[id, lensFacing, …]`).
/// Camera2 names no cameras, so each label comes from its facing.
pub(crate) async fn devices() -> Result<Vec<CameraDevice>, CameraError> {
    let vm = java_vm()?;
    let mut env = vm.attach_current_thread().map_err(jni_err)?;
    let activity = android_context();
    let flat = env
        .call_static_method(
            HELPER,
            "listCameras",
            "(Landroid/content/Context;)[Ljava/lang/String;",
            &[JValue::Object(&activity)],
        )
        .and_then(|v| v.l())
        .map_err(jni_err)?;
    let flat = JObjectArray::from(flat);
    let len = env.get_array_length(&flat).map_err(jni_err)?;
    let mut strings = Vec::with_capacity(len as usize);
    for i in 0..len {
        let item = JString::from(env.get_object_array_element(&flat, i).map_err(jni_err)?);
        strings.push(String::from(env.get_string(&item).map_err(jni_err)?));
    }
    Ok(crate::devices::from_camera2_list(&strings))
}

pub(crate) async fn open(
    config: CameraConfig,
    writer: FrameWriter,
) -> Result<(StreamHandle, Option<NativeSource>), CameraError> {
    let vm = java_vm()?;
    let mut env = vm.attach_current_thread().map_err(jni_err)?;
    let activity = android_context();

    // Ensure the CAMERA runtime permission. On first use this shows the system
    // dialog and waits for the answer; a denial returns `PermissionDenied`.
    // WITHOUT this, a not-yet-granted permission just fails silently and the
    // camera never opens — and no dialog is ever shown.
    request_permission().await?;

    let shim = Shim::new(&mut env)?;
    let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
    // No frame arrives to check `wants_cpu_frames` on while the reader is not a
    // target, so demand reaches the shim as a call.
    let demand_shim = shim.clone();
    writer.on_cpu_demand_change(move |wants| {
        demand_shim.call(
            "setCpuDemand",
            "(JZ)V",
            &[JValue::Long(token as jlong), JValue::Bool(wants as jboolean)],
        );
    });
    writers().lock().unwrap().insert(token, writer);
    let (tx, rx) = oneshot::channel::<Result<PreviewGeometry, CameraError>>();
    pending_open().lock().unwrap().insert(token, tx);

    let facing = match config.facing {
        CameraFacing::Default => FACING_DEFAULT,
        CameraFacing::Front => FACING_FRONT,
        CameraFacing::Back => FACING_BACK,
    };
    // A pinned camera id (null when unset): the shim reports one it doesn't
    // have as "no camera".
    let camera_id = match &config.device {
        Some(id) => JObject::from(env.new_string(id).map_err(jni_err)?),
        None => JObject::null(),
    };
    // 0 = "device default" for width/height/fps; the shim picks a supported
    // size/rate when a dimension is 0.
    let launch = env.call_static_method(
        HELPER,
        "open",
        "(Landroid/content/Context;Ljava/lang/String;IIIIJ)V",
        &[
            JValue::Object(&activity),
            JValue::Object(&camera_id),
            JValue::Int(facing),
            JValue::Int(config.width.unwrap_or(0) as jint),
            JValue::Int(config.height.unwrap_or(0) as jint),
            JValue::Int(config.fps.unwrap_or(0) as jint),
            JValue::Long(token as jlong),
        ],
    );
    if let Err(e) = launch {
        // The shim never got a chance to call back — clean up the registries.
        forget(token);
        return Err(jni_err(e));
    }

    match rx.await {
        Ok(Ok(geometry)) => {
            let driver = CameraPreviewDriver { shim: shim.clone(), token };
            let (source, producer) = preview_channel(geometry, driver);
            // No size the device can show this reader's frames at is a preview refused
            // from the start. A source still goes out, not none: a display has to learn
            // it should ask for CPU frames, because this producer makes them only on
            // demand and a display polling a sourceless stream wouldn't.
            if geometry.width == 0 || geometry.height == 0 {
                producer.refuse();
            }
            previews().lock().unwrap().insert(token, producer);
            Ok((StreamHandle { token, shim }, Some(Rc::new(source) as NativeSource)))
        }
        Ok(Err(e)) => {
            forget(token);
            Err(e)
        }
        Err(_) => {
            forget(token);
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

/// `RustCamera2Helper.nativeOpened` — the session is live and streaming. The size a
/// display's surface can take (zero when this device has none for this reader's
/// frames), with the sensor mounting and facing that say how to place it.
///
/// # Safety
/// Called by the JVM with a valid `env`/`class`.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_camera_RustCamera2Helper_nativeOpened(
    _env: JNIEnv,
    _class: JClass,
    token: jlong,
    preview_width: jint,
    preview_height: jint,
    sensor_orientation: jint,
    front: jboolean,
) {
    let result = std::panic::catch_unwind(|| {
        let geometry = PreviewGeometry {
            width: u32::try_from(preview_width).unwrap_or(0),
            height: u32::try_from(preview_height).unwrap_or(0),
            sensor_orientation,
            front_facing: front != JNI_FALSE,
        };
        if let Some(tx) = pending_open().lock().unwrap().remove(&(token as u64)) {
            let _ = tx.send(Ok(geometry));
        }
    });
    if result.is_err() {
        eprintln!("camera: panic in nativeOpened trampoline; aborting");
        std::process::abort();
    }
}

/// `RustCamera2Helper.nativeError` — the camera failed to open, or went away after
/// opening. Resolves the open future with a typed error; with no open pending, ends
/// the stream so nothing waits forever on a frame.
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
        let pending = pending_open().lock().unwrap().remove(&(token as u64));
        match pending {
            Some(tx) => {
                let _ = tx.send(Err(err));
            }
            None => {
                // Cloned out so the fan-out it triggers runs without the lock.
                let writer = writers().lock().unwrap().get(&(token as u64)).cloned();
                if let Some(writer) = writer {
                    writer.end_of_stream();
                }
            }
        }
    }));
    if result.is_err() {
        eprintln!("camera: panic in nativeError trampoline; aborting");
        std::process::abort();
    }
}

/// `RustCamera2Helper.nativePreviewRefused` — this device will not run the preview
/// alongside the reader. Displays of this stream fall back to CPU frames.
///
/// # Safety
/// Called by the JVM with a valid `env`/`class`.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_camera_RustCamera2Helper_nativePreviewRefused(
    _env: JNIEnv,
    _class: JClass,
    token: jlong,
) {
    let result = std::panic::catch_unwind(|| {
        if let Some(producer) = previews().lock().unwrap().get(&(token as u64)) {
            producer.refuse();
        }
    });
    if result.is_err() {
        eprintln!("camera: panic in nativePreviewRefused trampoline; aborting");
        std::process::abort();
    }
}

/// `RustCamera2Helper.nativeRepeatingTargets` — which outputs the repeating request
/// fills, as the bits of [`crate::android_preview`]. Pure logic that cannot panic.
///
/// # Safety
/// Called by the JVM with a valid `env`/`class`.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_camera_RustCamera2Helper_nativeRepeatingTargets(
    _env: JNIEnv,
    _class: JClass,
    preview_live: jboolean,
    cpu_wanted: jboolean,
) -> jint {
    crate::android_preview::repeating_targets(preview_live != JNI_FALSE, cpu_wanted != JNI_FALSE)
}

/// `RustCamera2Helper.nativeChoosePreviewSize` — the index into `sizes` (a flat
/// `[w0, h0, w1, h1, …]` of the sizes a `SurfaceTexture` can take) for the preview, or
/// -1 for none. See [`crate::android_preview::choose_preview_size`].
///
/// # Safety
/// Called by the JVM with a valid `env`/`class`; `sizes` is a non-null `int[]`.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_camera_RustCamera2Helper_nativeChoosePreviewSize(
    env: JNIEnv,
    _class: JClass,
    reader_width: jint,
    reader_height: jint,
    sizes: JIntArray,
    display_width: jint,
    display_height: jint,
) -> jint {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let length = env.get_array_length(&sizes).ok()? as usize;
        let mut flat = vec![0; length];
        env.get_int_array_region(&sizes, 0, &mut flat).ok()?;
        let dim = |v: jint| u32::try_from(v).unwrap_or(0);
        let pairs: Vec<(u32, u32)> = flat.chunks_exact(2).map(|s| (dim(s[0]), dim(s[1]))).collect();
        crate::android_preview::choose_preview_size(
            (dim(reader_width), dim(reader_height)),
            &pairs,
            (dim(display_width), dim(display_height)),
        )
    }));
    match result {
        Ok(Some(index)) => index as jint,
        Ok(None) => -1,
        Err(_) => {
            eprintln!("camera: panic in nativeChoosePreviewSize trampoline; aborting");
            std::process::abort();
        }
    }
}

/// View a plane's direct `ByteBuffer` as a [`Plane`], or `None` to refuse the
/// frame: not a direct buffer, a null address, or a negative stride. The slice
/// spans the buffer's whole capacity; [`YuvFrame::new`] checks it against the
/// frame.
///
/// # Safety
/// `buffer` must be a direct buffer whose memory stays valid and unwritten for
/// `'a`. A plane of an open `Image` qualifies; closing the `Image` frees it.
unsafe fn plane<'a>(
    env: &JNIEnv,
    buffer: &'a JByteBuffer,
    row_stride: jint,
    pixel_stride: jint,
) -> Option<Plane<'a>> {
    let row_stride = usize::try_from(row_stride).ok()?;
    let pixel_stride = usize::try_from(pixel_stride).ok()?;
    let address = env.get_direct_buffer_address(buffer).ok()?;
    let capacity = env.get_direct_buffer_capacity(buffer).ok()?;
    if address.is_null() {
        return None;
    }
    // SAFETY: a direct buffer's address and capacity describe one live
    // allocation, and the caller keeps it alive and unwritten for `'a`.
    let data = unsafe { std::slice::from_raw_parts(address as *const u8, capacity) };
    Some(Plane { data, row_stride, pixel_stride })
}

/// `RustCamera2Helper.nativeFrameYuv` — one `YUV_420_888` image, as its three
/// plane buffers and their strides, plus the clockwise `rotation` to bake in.
/// Converted to tightly-packed top-down `RGBA8` in the stream's reused frame
/// buffer: no copy across JNI and no per-frame allocation. A frame that cannot
/// be converted is dropped. Delivered on the shim's ImageReader handler thread.
///
/// The U and V buffers may overlap (semi-planar layouts); they are only read.
///
/// # Safety
/// Called by the JVM with a valid `env`/`class`. The buffers are the direct
/// plane buffers of an `Image` the shim closes only after this call returns, so
/// the slices never outlive their memory, and no slice survives this call.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_camera_RustCamera2Helper_nativeFrameYuv(
    env: JNIEnv,
    _class: JClass,
    token: jlong,
    y: JByteBuffer,
    y_row_stride: jint,
    y_pixel_stride: jint,
    u: JByteBuffer,
    u_row_stride: jint,
    u_pixel_stride: jint,
    v: JByteBuffer,
    v_row_stride: jint,
    v_pixel_stride: jint,
    width: jint,
    height: jint,
    rotation: jint,
) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // Clone the frame writer out from under the registry lock, then push
        // into it without holding the global lock across the channel fan-out
        // (a subscriber may itself touch the stream, e.g. drop it).
        let writer = writers().lock().unwrap().get(&(token as u64)).cloned();
        let Some(writer) = writer else {
            return;
        };
        let (Ok(width), Ok(height)) = (usize::try_from(width), usize::try_from(height)) else {
            return;
        };
        let Some(rotation) = Rotation::from_degrees(rotation) else {
            return;
        };
        // SAFETY: the shim keeps the `Image` open until this function returns
        // (see above), and everything here is done by then.
        let planes = unsafe {
            (
                plane(&env, &y, y_row_stride, y_pixel_stride),
                plane(&env, &u, u_row_stride, u_pixel_stride),
                plane(&env, &v, v_row_stride, v_pixel_stride),
            )
        };
        let (Some(y), Some(u), Some(v)) = planes else {
            return;
        };
        // Refused here, before the writer is touched, so a bad frame publishes
        // nothing rather than a blank one.
        let Some(frame) = YuvFrame::new(width, height, y, u, v) else {
            return;
        };
        let (out_width, out_height) = frame.output_size(rotation);
        // `write_rgba8_with` sizes `out` from these same dimensions, so the
        // conversion's own size check cannot fail.
        writer.write_rgba8_with(out_width as u32, out_height as u32, |out| {
            frame.convert(rotation, out);
        });
    }));
    if result.is_err() {
        eprintln!("camera: panic in nativeFrameYuv trampoline; aborting");
        std::process::abort();
    }
}

/// `RustCamera2Helper.nativeFrameRotation` — the clockwise angle each frame is
/// rotated by, from the sensor mounting, the display's
/// `Surface.ROTATION_*` and the lens facing. The math is the host-tested
/// [`crate::android_rotation::frame_rotation`]; it lives in Rust so it can be
/// unit-tested (there is no JVM test harness). Called on open and again on
/// every display rotation, never per frame.
///
/// # Safety
/// Called by the JVM with a valid `env`/`class`.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_camera_RustCamera2Helper_nativeFrameRotation(
    _env: JNIEnv,
    _class: JClass,
    sensor_orientation: jint,
    display_rotation: jint,
    front: jboolean,
) -> jint {
    // Pure integer arithmetic on `rem_euclid` — cannot panic, so no
    // catch_unwind is needed at this FFI boundary.
    crate::android_rotation::frame_rotation(sensor_orientation, display_rotation, front != JNI_FALSE)
}

// Pin the exports so the linker keeps them in the app `cdylib`'s dynamic
// symbol table (the JVM resolves them by `dlsym`).
#[used]
static KEEP_NATIVE_OPENED: extern "system" fn(JNIEnv, JClass, jlong, jint, jint, jint, jboolean) =
    Java_io_idealyst_camera_RustCamera2Helper_nativeOpened;
#[used]
static KEEP_NATIVE_PREVIEW_REFUSED: extern "system" fn(JNIEnv, JClass, jlong) =
    Java_io_idealyst_camera_RustCamera2Helper_nativePreviewRefused;
#[used]
static KEEP_NATIVE_REPEATING_TARGETS: extern "system" fn(JNIEnv, JClass, jboolean, jboolean) -> jint =
    Java_io_idealyst_camera_RustCamera2Helper_nativeRepeatingTargets;
#[used]
static KEEP_NATIVE_CHOOSE_PREVIEW_SIZE: extern "system" fn(
    JNIEnv,
    JClass,
    jint,
    jint,
    JIntArray,
    jint,
    jint,
) -> jint = Java_io_idealyst_camera_RustCamera2Helper_nativeChoosePreviewSize;
#[used]
static KEEP_NATIVE_ERROR: extern "system" fn(JNIEnv, JClass, jlong, jint, JString) =
    Java_io_idealyst_camera_RustCamera2Helper_nativeError;
#[used]
static KEEP_NATIVE_FRAME: extern "system" fn(
    JNIEnv,
    JClass,
    jlong,
    JByteBuffer,
    jint,
    jint,
    JByteBuffer,
    jint,
    jint,
    JByteBuffer,
    jint,
    jint,
    jint,
    jint,
    jint,
) = Java_io_idealyst_camera_RustCamera2Helper_nativeFrameYuv;
#[used]
static KEEP_NATIVE_FRAME_ROTATION: extern "system" fn(JNIEnv, JClass, jint, jint, jboolean) -> jint =
    Java_io_idealyst_camera_RustCamera2Helper_nativeFrameRotation;
