//! Apple capture via `AVCaptureSession` + `AVCaptureVideoDataOutput`,
//! covering both iOS and macOS (AVFoundation is the same framework on
//! both). Frames arrive on a declared sample-buffer delegate, on a private
//! serial dispatch queue, and are repacked from the device's native `BGRA`
//! into the SDK's tightly-packed top-down `RGBA8` before the callback runs.
//!
//! We drive AVFoundation through the Obj-C runtime (no typed framework
//! crate) and link `CoreMedia`/`CoreVideo` for the handful of C calls that
//! read pixels out of a `CVPixelBuffer` — the same posture the `net` SDK
//! takes with `NSURLSession`. The delegate bridges to the callback through
//! a `Send + Sync` `Mutex`, exactly like the SSE delegate's shared state.
//!
//! Verified on macOS against the built-in camera (see
//! `tests/host_capture.rs`).

use std::ffi::c_void;
use std::ptr;
use std::sync::Mutex;

use objc2::encode::{Encode, Encoding};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObjectProtocol};
use objc2::{class, declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_foundation::{NSObject, NSString};

use crate::{CameraConfig, CameraError, CameraFacing, NativeSource};
use media_stream::FrameWriter;

// ---------------------------------------------------------------------------
// Foreign surfaces. AVFoundation classes are reached by name via `class!`,
// so the framework must be linked into the process; the empty extern block
// forces that. CoreMedia/CoreVideo expose the C functions that crack a
// sample buffer open into raw pixels.
// ---------------------------------------------------------------------------

#[link(name = "AVFoundation", kind = "framework")]
extern "C" {}

#[allow(non_upper_case_globals)]
#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    /// The `CVPixelBufferAttributeKey` we set in the output's `videoSettings`
    /// to force `BGRA` frames (toll-free bridged to `NSString`).
    static kCVPixelBufferPixelFormatTypeKey: *const c_void;

    fn CVPixelBufferLockBaseAddress(pb: *mut c_void, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(pb: *mut c_void, flags: u64) -> i32;
    fn CVPixelBufferGetBaseAddress(pb: *mut c_void) -> *mut c_void;
    fn CVPixelBufferGetWidth(pb: *mut c_void) -> usize;
    fn CVPixelBufferGetHeight(pb: *mut c_void) -> usize;
    fn CVPixelBufferGetBytesPerRow(pb: *mut c_void) -> usize;
    /// The `IOSurface` backing the pixel buffer (camera frames are
    /// IOSurface-backed), or null. Borrowed — the `SurfaceWriter` retains it.
    /// Used on macOS for the zero-copy `CALayer.contents = IOSurface` display
    /// path. (iOS publishes the `CMSampleBuffer` itself instead.)
    #[cfg(target_os = "macos")]
    fn CVPixelBufferGetIOSurface(pb: *mut c_void) -> *const c_void;
}

#[link(name = "CoreMedia", kind = "framework")]
extern "C" {
    fn CMSampleBufferGetImageBuffer(sbuf: *mut c_void) -> *mut c_void;
    fn CMVideoFormatDescriptionGetDimensions(desc: *mut c_void) -> CMVideoDimensions;
    fn CMTimeMake(value: i64, timescale: i32) -> CMTime;
}

extern "C" {
    /// Serial queue for delegate callbacks. In libSystem (always linked).
    fn dispatch_queue_create(label: *const std::ffi::c_char, attr: *mut c_void) -> *mut c_void;
}

/// `CMVideoDimensions` — `{ int32 width; int32 height; }`. Returned by value
/// from `CMVideoFormatDescriptionGetDimensions`; only crosses the C ABI, so
/// it needs no `Encode`.
#[repr(C)]
#[derive(Clone, Copy)]
struct CMVideoDimensions {
    width: i32,
    height: i32,
}

/// `CMTime`. Passed by value through `setActiveVideoM{in,ax}FrameDuration:`,
/// so it must implement [`Encode`].
#[repr(C)]
#[derive(Clone, Copy)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

// SAFETY: the field layout and encoding match Apple's `CMTime` exactly
// (int64 value, int32 timescale, uint32 flags, int64 epoch), so objc2
// builds the correct method type for passing it by value.
unsafe impl Encode for CMTime {
    const ENCODING: Encoding = Encoding::Struct(
        "?",
        &[
            Encoding::LongLong,
            Encoding::Int,
            Encoding::UInt,
            Encoding::LongLong,
        ],
    );
}

/// `'BGRA'` as an `OSType` (`kCVPixelFormatType_32BGRA`). The well-supported
/// packed format on every Apple camera; we swizzle it to `RGBA` per frame.
const PIXEL_FORMAT_32BGRA: u32 = 0x4247_5241;
/// `kCVPixelBufferLock_ReadOnly` — we only read the captured pixels.
const LOCK_READ_ONLY: u64 = 0x0000_0001;

/// `AVMediaTypeVideo`'s string value. The constant's value equals this
/// literal, so we build it directly rather than linking the extern symbol
/// (same trick `microphone` uses for the audio-session category).
const AV_MEDIA_TYPE_VIDEO: &str = "vide";
/// `AVCaptureSessionPresetInputPriority` — required so the session lets us
/// pin `device.activeFormat` instead of overriding it with a preset.
const PRESET_INPUT_PRIORITY: &str = "AVCaptureSessionPresetInputPriority";
/// `AVCaptureDeviceTypeBuiltInWideAngleCamera` — the front/back camera a
/// discovery session resolves a [`CameraFacing`] to.
const DEVICE_TYPE_WIDE_ANGLE: &str = "AVCaptureDeviceTypeBuiltInWideAngleCamera";

// AVCaptureDevicePosition.
const POSITION_BACK: i64 = 1;
const POSITION_FRONT: i64 = 2;

// ---------------------------------------------------------------------------
// Delegate. Receives `captureOutput:didOutputSampleBuffer:fromConnection:`
// on the serial queue and bridges each frame to the callback through a
// `Mutex` (the delegate is the only toucher, so it's uncontended — the lock
// is there for `Send + Sync`).
// ---------------------------------------------------------------------------

/// Callback + zero-copy native handle + reusable repack scratch, shared with
/// the delegate. Holds no Obj-C handle, so it is `Send + Sync`.
struct State {
    writer: FrameWriter,
    /// Publishes the frame's native handle for the zero-copy display fast-path
    /// (an `IOSurface` on macOS, the `CMSampleBuffer` on iOS). Always published
    /// (retain + pointer swap); the CPU `writer` path only runs when a consumer
    /// taps RGBA frames via [`FrameWriter::wants_cpu_frames`].
    surf: media_stream::SurfaceWriter,
    /// Reusable tightly-packed RGBA buffer so steady-state capture doesn't
    /// allocate per frame.
    scratch: Vec<u8>,
}

pub(crate) struct DelegateIvars {
    state: Mutex<State>,
}

declare_class!(
    struct FrameDelegate;

    unsafe impl ClassType for FrameDelegate {
        type Super = NSObject;
        type Mutability = mutability::InteriorMutable;
        const NAME: &'static str = "IdealystCameraFrameDelegate";
    }

    impl DeclaredClass for FrameDelegate {
        type Ivars = DelegateIvars;
    }

    unsafe impl NSObjectProtocol for FrameDelegate {}

    unsafe impl FrameDelegate {
        // AVCaptureVideoDataOutputSampleBufferDelegate. We don't import the
        // typed protocol (no objc2-av-foundation dep); AVFoundation calls the
        // selector by `respondsToSelector:`, so implementing the method is
        // enough. The CF/obj-c args are taken as raw object pointers (same
        // bits in-register regardless of the formal encoding) and cast to the
        // CF pointer type for the C calls below.
        #[method(captureOutput:didOutputSampleBuffer:fromConnection:)]
        fn did_output(
            &self,
            _output: *mut AnyObject,
            sample_buffer: *mut AnyObject,
            _connection: *mut AnyObject,
        ) {
            let sbuf = sample_buffer as *mut c_void;
            // SAFETY: AVFoundation hands us a valid CMSampleBuffer wrapping a
            // CVPixelBuffer (we requested BGRA video output). Each accessor is
            // a documented CoreMedia/CoreVideo C call on that buffer.
            unsafe {
                let pixel_buffer = CMSampleBufferGetImageBuffer(sbuf);
                if pixel_buffer.is_null() {
                    return;
                }

                // One lock for the frame — the delegate is the sole toucher of
                // `State` and frames arrive serially (the `Mutex` is only for
                // `Send`).
                let mut state = self.ivars().state.lock().unwrap();

                // Zero-copy display fast-path: publish the platform native
                // handle (retain + pointer swap, microseconds). macOS hands the
                // `video` SDK an IOSurface for `CALayer.contents`; iOS hands it
                // the CMSampleBuffer for `AVSampleBufferDisplayLayer`. This is
                // what drops the camera preview off the CPU — no swizzle, no
                // CGImage, no per-frame upload.
                #[cfg(target_os = "macos")]
                {
                    let surface = CVPixelBufferGetIOSurface(pixel_buffer);
                    if !surface.is_null() {
                        state.surf.publish(surface);
                    }
                }
                #[cfg(target_os = "ios")]
                {
                    // The CMSampleBuffer itself (BGRA), displayed via
                    // AVSampleBufferDisplayLayer.
                    state.surf.publish(sbuf as *const c_void);
                }

                // CPU RGBA channel: only do the (expensive, full-frame) BGRA→RGBA
                // swizzle when a consumer is actually tapping CPU frames (a
                // `subscribe`r). Native-source display reads the handle above and
                // needs none of it — a preview-only session pays zero per-pixel
                // CPU cost. See `FrameWriter::wants_cpu_frames`.
                if state.writer.wants_cpu_frames() {
                    if CVPixelBufferLockBaseAddress(pixel_buffer, LOCK_READ_ONLY) != 0 {
                        return;
                    }
                    let base = CVPixelBufferGetBaseAddress(pixel_buffer) as *const u8;
                    let width = CVPixelBufferGetWidth(pixel_buffer);
                    let height = CVPixelBufferGetHeight(pixel_buffer);
                    let stride = CVPixelBufferGetBytesPerRow(pixel_buffer);

                    if !base.is_null() && width > 0 && height > 0 && stride >= width * 4 {
                        let State { writer, scratch, .. } = &mut *state;
                        repack_bgra_to_rgba(base, width, height, stride, scratch);
                        writer.write_rgba8(width as u32, height as u32, scratch);
                    }

                    CVPixelBufferUnlockBaseAddress(pixel_buffer, LOCK_READ_ONLY);
                }
            }
        }
    }
);

/// Copy a strided `BGRA` image into a tightly-packed top-down `RGBA8`
/// buffer, swizzling `B`/`R`. `scratch` is reused across frames.
///
/// # Safety
/// `base` must point at `height * stride` readable bytes with at least
/// `width * 4` valid bytes per row.
unsafe fn repack_bgra_to_rgba(
    base: *const u8,
    width: usize,
    height: usize,
    stride: usize,
    scratch: &mut Vec<u8>,
) {
    let row_bytes = width * 4;
    scratch.clear();
    scratch.resize(row_bytes * height, 0);
    for y in 0..height {
        let src_row = std::slice::from_raw_parts(base.add(y * stride), row_bytes);
        let dst_row = &mut scratch[y * row_bytes..(y + 1) * row_bytes];
        for x in 0..width {
            let s = &src_row[x * 4..x * 4 + 4]; // B G R A
            let d = &mut dst_row[x * 4..x * 4 + 4]; // R G B A
            d[0] = s[2];
            d[1] = s[1];
            d[2] = s[0];
            d[3] = s[3];
        }
    }
}

// ---------------------------------------------------------------------------
// iOS orientation. Keeps the capture connection's rotation in step with the
// INTERFACE orientation (`UIWindowScene.interfaceOrientation`), so frames are
// upright in all four orientations — the mapping itself is the pure, unit-
// tested `crate::orientation::capture_rotation`.
//
// Everything that touches UIKit runs on the main thread: `start`/`stop` hop
// there with `performSelectorOnMainThread:` (inline when already on main),
// and the notification handlers fire there because UIDevice/UIApplication
// post on main. The connection setters themselves are thread-safe and may be
// changed while the session runs.
// ---------------------------------------------------------------------------

#[cfg(target_os = "ios")]
use crate::orientation::{
    calibrated_portrait_angle, capture_rotation, CameraPosition, CaptureRotation,
    InterfaceOrientation, STANDARD_PORTRAIT_ANGLE,
};

/// Posted on `[UIDevice currentDevice]` when the device turns. The constant's
/// value equals its name, so it's built from the literal (no UIKit link).
#[cfg(target_os = "ios")]
const DEVICE_ORIENTATION_DID_CHANGE: &str = "UIDeviceOrientationDidChangeNotification";
/// Posted when the app comes back to the foreground — the interface may have
/// rotated while it was away, with no device-orientation change after.
#[cfg(target_os = "ios")]
const APP_DID_BECOME_ACTIVE: &str = "UIApplicationDidBecomeActiveNotification";
/// `UISceneActivationStateForegroundActive` / `…ForegroundInactive`.
#[cfg(target_os = "ios")]
const SCENE_FOREGROUND_ACTIVE: isize = 0;
#[cfg(target_os = "ios")]
const SCENE_FOREGROUND_INACTIVE: isize = 1;

#[cfg(target_os = "ios")]
pub(crate) struct OrientationIvars {
    /// The data output's video connection (the only one — see `open`).
    connection: Retained<AnyObject>,
    position: CameraPosition,
    /// This connection's `videoRotationAngle` for Portrait (iOS 17+).
    portrait_angle: f64,
    /// Registered for notifications (main thread only).
    observing: std::cell::Cell<bool>,
    /// Last orientation applied, to skip redundant connection writes.
    applied: std::cell::Cell<Option<InterfaceOrientation>>,
}

#[cfg(target_os = "ios")]
declare_class!(
    pub(crate) struct OrientationObserver;

    unsafe impl ClassType for OrientationObserver {
        type Super = NSObject;
        type Mutability = mutability::InteriorMutable;
        const NAME: &'static str = "IdealystCameraOrientationObserver";
    }

    impl DeclaredClass for OrientationObserver {
        type Ivars = OrientationIvars;
    }

    unsafe impl NSObjectProtocol for OrientationObserver {}

    unsafe impl OrientationObserver {
        /// Main thread. Apply the current orientation, then follow changes.
        #[method(startObserving)]
        fn start_observing(&self) {
            if self.ivars().observing.replace(true) {
                return;
            }
            unsafe {
                let center: Retained<AnyObject> =
                    msg_send_id![class!(NSNotificationCenter), defaultCenter];
                if let Some(device_class) = objc2::runtime::AnyClass::get("UIDevice") {
                    let device: Retained<AnyObject> = msg_send_id![device_class, currentDevice];
                    // Ref-counted by UIKit; balanced in `stopObserving`.
                    let _: () = msg_send![&*device, beginGeneratingDeviceOrientationNotifications];
                    let name = NSString::from_str(DEVICE_ORIENTATION_DID_CHANGE);
                    let _: () = msg_send![
                        &*center,
                        addObserver: self,
                        selector: objc2::sel!(interfaceMayHaveRotated:),
                        name: &*name,
                        object: &*device,
                    ];
                }
                let name = NSString::from_str(APP_DID_BECOME_ACTIVE);
                let _: () = msg_send![
                    &*center,
                    addObserver: self,
                    selector: objc2::sel!(interfaceMayHaveRotated:),
                    name: &*name,
                    object: ptr::null::<AnyObject>(),
                ];
            }
            self.sync();
        }

        /// Main thread. Undo everything `startObserving` registered.
        #[method(stopObserving)]
        fn stop_observing(&self) {
            if !self.ivars().observing.replace(false) {
                return;
            }
            unsafe {
                let center: Retained<AnyObject> =
                    msg_send_id![class!(NSNotificationCenter), defaultCenter];
                let _: () = msg_send![&*center, removeObserver: self];
                let _: () = msg_send![
                    class!(NSObject),
                    cancelPreviousPerformRequestsWithTarget: self
                ];
                if let Some(device_class) = objc2::runtime::AnyClass::get("UIDevice") {
                    let device: Retained<AnyObject> = msg_send_id![device_class, currentDevice];
                    let _: () = msg_send![&*device, endGeneratingDeviceOrientationNotifications];
                }
            }
        }

        /// Notification handler (main thread). The device-orientation
        /// notification can reach us BEFORE UIKit has rotated the interface
        /// for the same event (observer order is unspecified), so reading
        /// `interfaceOrientation` here could see the old value. Defer the read
        /// to the next run-loop turn, after UIKit has handled the rotation.
        #[method(interfaceMayHaveRotated:)]
        fn interface_may_have_rotated(&self, _note: *mut AnyObject) {
            unsafe {
                let _: () = msg_send![
                    self,
                    performSelector: objc2::sel!(syncOrientation),
                    withObject: ptr::null::<AnyObject>(),
                    afterDelay: 0.0f64,
                ];
            }
        }

        /// Main thread. Deferred target of `interfaceMayHaveRotated:`.
        #[method(syncOrientation)]
        fn sync_orientation(&self) {
            self.sync();
        }
    }
);

#[cfg(target_os = "ios")]
impl OrientationObserver {
    /// Configure `connection` (mirroring off, Portrait angle calibrated) and
    /// start following the interface orientation. When called on the main
    /// thread the current orientation is applied before this returns, so the
    /// session's first frames are already upright.
    unsafe fn start(connection: Retained<AnyObject>, position: CameraPosition) -> Retained<Self> {
        // Mirroring BEFORE rotation: the calibration below reads the angle of
        // the connection as it will actually run.
        disable_mirroring(&connection);
        let portrait_angle = calibrate_portrait_angle(&connection);
        let this = Self::alloc().set_ivars(OrientationIvars {
            connection,
            position,
            portrait_angle,
            observing: std::cell::Cell::new(false),
            applied: std::cell::Cell::new(None),
        });
        let this: Retained<Self> = msg_send_id![super(this), init];
        this.on_main(objc2::sel!(startObserving));
        this
    }

    /// Main thread. Read the interface orientation and apply it.
    fn sync(&self) {
        let ivars = self.ivars();
        if !ivars.observing.get() {
            return;
        }
        // Unknown (no scene yet / mid-teardown): keep the last rotation.
        let Some(orientation) = (unsafe { current_interface_orientation() }) else {
            return;
        };
        if ivars.applied.get() == Some(orientation) {
            return;
        }
        let rotation = capture_rotation(orientation, ivars.position, ivars.portrait_angle);
        unsafe { apply_rotation(&ivars.connection, rotation) };
        ivars.applied.set(Some(orientation));
    }

    /// Stop following orientation (from `StreamHandle::drop`, any thread).
    fn stop(&self) {
        // SAFETY: `stopObserving` is a declared no-argument method.
        unsafe { self.on_main(objc2::sel!(stopObserving)) };
    }

    /// Run `sel` on the main thread: inline when already there, otherwise
    /// queued without blocking (blocking could deadlock a main thread that is
    /// itself waiting on this one). The queued perform retains `self`, and
    /// main runs queued performs in order, so a stop always follows its start.
    unsafe fn on_main(&self, sel: objc2::runtime::Sel) {
        let is_main: Bool = msg_send![class!(NSThread), isMainThread];
        let _: () = msg_send![
            self,
            performSelectorOnMainThread: sel,
            withObject: ptr::null::<AnyObject>(),
            waitUntilDone: is_main,
        ];
    }
}

/// The interface orientation of the app's foreground window scene
/// (foreground-active preferred), or `None` if there is none / it's Unknown.
#[cfg(target_os = "ios")]
unsafe fn current_interface_orientation() -> Option<InterfaceOrientation> {
    // Resolved by name so the camera crate needn't link UIKit (every iOS app
    // does); absent → nothing to follow.
    let app_class = objc2::runtime::AnyClass::get("UIApplication")?;
    let scene_class = objc2::runtime::AnyClass::get("UIWindowScene")?;
    let app: Option<Retained<AnyObject>> = msg_send_id![app_class, sharedApplication];
    let app = app?;
    let scenes: Retained<AnyObject> = msg_send_id![&*app, connectedScenes];
    let scenes: Retained<AnyObject> = msg_send_id![&*scenes, allObjects];
    let count: usize = msg_send![&*scenes, count];
    let mut fallback: Option<isize> = None;
    for i in 0..count {
        let scene: Retained<AnyObject> = msg_send_id![&*scenes, objectAtIndex: i];
        let is_window_scene: Bool = msg_send![&*scene, isKindOfClass: scene_class];
        if !is_window_scene.as_bool() {
            continue;
        }
        let state: isize = msg_send![&*scene, activationState];
        let raw: isize = msg_send![&*scene, interfaceOrientation];
        if state == SCENE_FOREGROUND_ACTIVE {
            return InterfaceOrientation::from_raw(raw);
        }
        if state == SCENE_FOREGROUND_INACTIVE || fallback.is_none() {
            fallback = Some(raw);
        }
    }
    fallback.and_then(InterfaceOrientation::from_raw)
}

/// Whether `obj` implements `sel` — the iOS 17 API probe.
#[cfg(target_os = "ios")]
unsafe fn responds_to(obj: &AnyObject, sel: objc2::runtime::Sel) -> bool {
    let r: Bool = msg_send![obj, respondsToSelector: sel];
    r.as_bool()
}

/// Turn mirroring off for both cameras, matching the web backend (raw
/// `getUserMedia` frames are unmirrored). `automaticallyAdjustsVideoMirroring`
/// must be cleared FIRST: setting `videoMirrored` while it is YES raises an
/// Obj-C exception.
#[cfg(target_os = "ios")]
unsafe fn disable_mirroring(connection: &AnyObject) {
    let supported: Bool = msg_send![connection, isVideoMirroringSupported];
    if supported.as_bool() {
        let _: () = msg_send![connection, setAutomaticallyAdjustsVideoMirroring: Bool::NO];
        let _: () = msg_send![connection, setVideoMirrored: Bool::NO];
    }
}

/// This connection's `videoRotationAngle` for Portrait. Most sensors give
/// 90°, but not all (the iPhone 17 Pro front camera reports 0°), and only
/// AVFoundation knows the mounting. So ask it: set the (deprecated, still
/// honored) `videoOrientation = Portrait` and read back the angle AVFoundation
/// translated it to. iOS 16 (no `videoRotationAngle`) never uses the angle.
#[cfg(target_os = "ios")]
unsafe fn calibrate_portrait_angle(connection: &AnyObject) -> f64 {
    if !responds_to(connection, objc2::sel!(setVideoRotationAngle:)) {
        return STANDARD_PORTRAIT_ANGLE;
    }
    let supported: Bool = msg_send![connection, isVideoOrientationSupported];
    if !supported.as_bool() {
        return STANDARD_PORTRAIT_ANGLE;
    }
    // AVCaptureVideoOrientationPortrait.
    let _: () = msg_send![connection, setVideoOrientation: 1isize];
    let read_back: f64 = msg_send![connection, videoRotationAngle];
    calibrated_portrait_angle(read_back).unwrap_or(STANDARD_PORTRAIT_ANGLE)
}

/// Apply `rotation`: `videoRotationAngle` on iOS 17+ (probed with
/// `respondsToSelector:`), else `videoOrientation`. Unsupported values are
/// skipped — both setters raise an Obj-C exception on them.
#[cfg(target_os = "ios")]
unsafe fn apply_rotation(connection: &AnyObject, rotation: CaptureRotation) {
    if responds_to(connection, objc2::sel!(setVideoRotationAngle:)) {
        // CGFloat == f64 on every 64-bit iOS target.
        let angle: f64 = rotation.rotation_angle;
        let supported: Bool = msg_send![connection, isVideoRotationAngleSupported: angle];
        if supported.as_bool() {
            let _: () = msg_send![connection, setVideoRotationAngle: angle];
        }
    } else {
        let supported: Bool = msg_send![connection, isVideoOrientationSupported];
        if supported.as_bool() {
            let _: () = msg_send![connection, setVideoOrientation: rotation.video_orientation];
        }
    }
}

// ---------------------------------------------------------------------------
// Stream handle. Holds the session alive; drop stops capture and releases
// the queue. Not `Send` (Obj-C handles), matching the public docs.
// ---------------------------------------------------------------------------

pub(crate) struct StreamHandle {
    session: Retained<AnyObject>,
    _delegate: Retained<FrameDelegate>,
    _queue: Retained<AnyObject>,
    /// Keeps the capture connection following the interface orientation.
    /// `None` only if the output had no video connection.
    #[cfg(target_os = "ios")]
    orientation_observer: Option<Retained<OrientationObserver>>,
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        unsafe {
            let _: () = msg_send![&*self.session, stopRunning];
        }
        #[cfg(target_os = "ios")]
        if let Some(observer) = &self.orientation_observer {
            observer.stop();
        }
        // `_queue`/`_delegate` release here; the output drops its own retains
        // when the session deallocates.
    }
}

// ---------------------------------------------------------------------------
// Permission. Delegated to the shared `permissions` SDK — the AVCaptureDevice
// `authorizationStatusForMediaType:` / `requestAccessForMediaType:` grant code
// that used to live here now lives in `permissions::imp` (apple.rs) under
// `Permission::Camera`. This crate keeps only the CAPTURE code below.
// ---------------------------------------------------------------------------

pub(crate) async fn request_permission() -> Result<(), CameraError> {
    map_permission(permissions::request(permissions::Permission::Camera).await)
}

/// Map the shared [`permissions::PermissionStatus`] onto camera's error.
/// `is_usable()` (Granted, or Unsupported where the platform needs no grant)
/// passes; anything else is a denial.
fn map_permission(status: permissions::PermissionStatus) -> Result<(), CameraError> {
    if status.is_usable() {
        Ok(())
    } else {
        Err(CameraError::PermissionDenied)
    }
}

// ---------------------------------------------------------------------------
// Open.
// ---------------------------------------------------------------------------

pub(crate) async fn open(
    config: CameraConfig,
    writer: FrameWriter,
) -> Result<(StreamHandle, Option<NativeSource>), CameraError> {
    request_permission().await?;

    // SAFETY: a straight transcription of the documented AVCaptureSession
    // setup (session → input → BGRA data output → delegate on a serial
    // queue → startRunning). Each `msg_send` targets a method on the class
    // it's sent to; failures are checked and mapped to `CameraError`.
    unsafe {
        let session: Retained<AnyObject> = msg_send_id![class!(AVCaptureSession), new];
        let _: () = msg_send![&*session, beginConfiguration];

        // Pin input priority up front when a resolution is requested, so the
        // session honours the device.activeFormat we set below.
        let wants_format = config.width.is_some() || config.height.is_some();
        if wants_format {
            let preset = NSString::from_str(PRESET_INPUT_PRIORITY);
            let _: () = msg_send![&*session, setSessionPreset: &*preset];
        }

        let device = device_for_facing(config.facing)?;

        let input: Option<Retained<AnyObject>> = msg_send_id![
            class!(AVCaptureDeviceInput),
            deviceInputWithDevice: &*device,
            error: ptr::null_mut::<*mut AnyObject>(),
        ];
        let input = input.ok_or_else(|| CameraError::Backend("device input creation failed".into()))?;
        let can_add_input: Bool = msg_send![&*session, canAddInput: &*input];
        if !can_add_input.as_bool() {
            return Err(CameraError::Backend("session cannot add camera input".into()));
        }
        let _: () = msg_send![&*session, addInput: &*input];

        configure_device(&device, &config)?;

        // BGRA video data output.
        let output: Retained<AnyObject> = msg_send_id![class!(AVCaptureVideoDataOutput), new];
        let settings = bgra_video_settings();
        let _: () = msg_send![&*output, setVideoSettings: &*settings];
        let _: () = msg_send![&*output, setAlwaysDiscardsLateVideoFrames: Bool::YES];

        // Zero-copy display channel. The delegate (capture queue) publishes
        // each frame's native handle through `surf_writer`; `surf_source` is
        // returned as the stream's `native_source` so the `video` SDK displays
        // it with no CPU copy.
        let (surf_source, surf_writer) = media_stream::surface_channel();

        // Delegate + private serial queue.
        let delegate: Retained<FrameDelegate> = {
            let this = FrameDelegate::alloc().set_ivars(DelegateIvars {
                state: Mutex::new(State {
                    writer,
                    surf: surf_writer,
                    scratch: Vec::new(),
                }),
            });
            msg_send_id![super(this), init]
        };
        let queue_raw = dispatch_queue_create(c"com.idealyst.camera".as_ptr(), ptr::null_mut());
        let queue: Retained<AnyObject> = Retained::from_raw(queue_raw.cast())
            .ok_or_else(|| CameraError::Backend("dispatch_queue_create failed".into()))?;
        let _: () =
            msg_send![&*output, setSampleBufferDelegate: &*delegate, queue: &*queue];

        let can_add_output: Bool = msg_send![&*session, canAddOutput: &*output];
        if !can_add_output.as_bool() {
            return Err(CameraError::Backend("session cannot add video output".into()));
        }
        let _: () = msg_send![&*session, addOutput: &*output];

        // Orient delivered frames upright by following the INTERFACE
        // orientation, kept current across rotations (see
        // `OrientationObserver`). The sensor is mounted landscape, so an
        // un-rotated connection is only upright in one landscape orientation;
        // up to 1.6.0 this pinned Portrait once at open, which rotated every
        // frame 90° on an iPad (or phone) in landscape.
        //
        // There is exactly ONE connection to rotate: the iOS preview is an
        // `AVSampleBufferDisplayLayer` fed the very `CMSampleBuffer`s this
        // data output delivers (the `video` SDK's native path — there is no
        // `AVCaptureVideoPreviewLayer` connection), and the CPU RGBA channel
        // reads the same buffers. So rotating this connection makes every
        // consumer upright at once, converging on what the web backend gets
        // from `getUserMedia`.
        //
        // iOS-only: this same file also drives macOS capture, where the
        // webcam is fixed and landscape-natural (no interface orientation to
        // follow) and its buffers are already upright. The branch reflects
        // that form-factor difference; the output (upright, unmirrored
        // frames) converges across platforms.
        #[cfg(target_os = "ios")]
        let orientation_observer = {
            let media_type = NSString::from_str(AV_MEDIA_TYPE_VIDEO);
            let connection: Option<Retained<AnyObject>> =
                msg_send_id![&*output, connectionWithMediaType: &*media_type];
            match connection {
                Some(connection) => {
                    let position: isize = msg_send![&*device, position];
                    Some(OrientationObserver::start(
                        connection,
                        CameraPosition::from_raw(position),
                    ))
                }
                None => None,
            }
        };

        let _: () = msg_send![&*session, commitConfiguration];
        let _: () = msg_send![&*session, startRunning];

        // Hand back the native handle source for the zero-copy display path
        // (IOSurface on macOS → `CALayer.contents`; CMSampleBuffer on iOS →
        // `AVSampleBufferDisplayLayer`). The CPU RGBA channel still serves
        // `subscribe`rs; it's just no longer the display path.
        Ok((
            StreamHandle {
                session,
                _delegate: delegate,
                _queue: queue,
                #[cfg(target_os = "ios")]
                orientation_observer,
            },
            Some(std::rc::Rc::new(surf_source) as NativeSource),
        ))
    }
}

/// Build `@{ kCVPixelBufferPixelFormatTypeKey : @(kCVPixelFormatType_32BGRA) }`.
unsafe fn bgra_video_settings() -> Retained<AnyObject> {
    let number: Retained<AnyObject> =
        msg_send_id![class!(NSNumber), numberWithUnsignedInt: PIXEL_FORMAT_32BGRA];
    // The key is a CFStringRef constant, toll-free bridged to NSString.
    let key: &AnyObject = &*(kCVPixelBufferPixelFormatTypeKey as *const AnyObject);
    msg_send_id![
        class!(NSDictionary),
        dictionaryWithObject: &*number,
        forKey: key,
    ]
}

/// Resolve a [`CameraFacing`] to an `AVCaptureDevice`. `Default` takes the
/// system default video device; `Front`/`Back` run a discovery session for a
/// wide-angle camera at that position.
unsafe fn device_for_facing(facing: CameraFacing) -> Result<Retained<AnyObject>, CameraError> {
    let media_type = NSString::from_str(AV_MEDIA_TYPE_VIDEO);
    match facing {
        CameraFacing::Default => {
            let device: Option<Retained<AnyObject>> =
                msg_send_id![class!(AVCaptureDevice), defaultDeviceWithMediaType: &*media_type];
            device.ok_or(CameraError::NoCamera)
        }
        CameraFacing::Front | CameraFacing::Back => {
            let position = if matches!(facing, CameraFacing::Front) {
                POSITION_FRONT
            } else {
                POSITION_BACK
            };
            let type_str = NSString::from_str(DEVICE_TYPE_WIDE_ANGLE);
            let types: Retained<AnyObject> =
                msg_send_id![class!(NSArray), arrayWithObject: &*type_str];
            let discovery: Retained<AnyObject> = msg_send_id![
                class!(AVCaptureDeviceDiscoverySession),
                discoverySessionWithDeviceTypes: &*types,
                mediaType: &*media_type,
                position: position,
            ];
            let devices: Retained<AnyObject> = msg_send_id![&*discovery, devices];
            let count: usize = msg_send![&*devices, count];
            if count == 0 {
                return Err(CameraError::NoCamera);
            }
            let device: Retained<AnyObject> = msg_send_id![&*devices, objectAtIndex: 0usize];
            Ok(device)
        }
    }
}

/// Apply an explicit resolution (via `device.activeFormat`) and/or frame
/// rate when requested. A no-op when the config pins nothing.
unsafe fn configure_device(device: &AnyObject, config: &CameraConfig) -> Result<(), CameraError> {
    if config.width.is_none() && config.height.is_none() && config.fps.is_none() {
        return Ok(());
    }

    let locked: Bool = msg_send![device, lockForConfiguration: ptr::null_mut::<*mut AnyObject>()];
    if !locked.as_bool() {
        return Err(CameraError::Backend("lockForConfiguration failed".into()));
    }

    let result = configure_device_locked(device, config);

    let _: () = msg_send![device, unlockForConfiguration];
    result
}

unsafe fn configure_device_locked(
    device: &AnyObject,
    config: &CameraConfig,
) -> Result<(), CameraError> {
    match (config.width, config.height) {
        (Some(w), Some(h)) => {
            let formats: Retained<AnyObject> = msg_send_id![device, formats];
            let count: usize = msg_send![&*formats, count];
            let mut chosen: Option<Retained<AnyObject>> = None;
            for i in 0..count {
                let format: Retained<AnyObject> = msg_send_id![&*formats, objectAtIndex: i];
                let desc: *mut c_void = msg_send![&*format, formatDescription];
                let dims = CMVideoFormatDescriptionGetDimensions(desc);
                if dims.width == w as i32 && dims.height == h as i32 {
                    chosen = Some(format);
                    break;
                }
            }
            match chosen {
                Some(format) => {
                    let _: () = msg_send![device, setActiveFormat: &*format];
                }
                None => {
                    return Err(CameraError::UnsupportedConfig(format!(
                        "no {w}x{h} capture format on this camera"
                    )))
                }
            }
        }
        (None, None) => {}
        _ => {
            return Err(CameraError::UnsupportedConfig(
                "width and height must both be set".into(),
            ))
        }
    }

    if let Some(fps) = config.fps {
        if fps == 0 {
            return Err(CameraError::UnsupportedConfig("fps must be non-zero".into()));
        }
        let duration = CMTimeMake(1, fps as i32);
        let _: () = msg_send![device, setActiveVideoMinFrameDuration: duration];
        let _: () = msg_send![device, setActiveVideoMaxFrameDuration: duration];
    }

    Ok(())
}
