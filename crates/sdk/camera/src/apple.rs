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

use block2::RcBlock;
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
// AVAuthorizationStatus.
const AUTH_RESTRICTED: i64 = 1;
const AUTH_DENIED: i64 = 2;
const AUTH_AUTHORIZED: i64 = 3;

// ---------------------------------------------------------------------------
// Delegate. Receives `captureOutput:didOutputSampleBuffer:fromConnection:`
// on the serial queue and bridges each frame to the callback through a
// `Mutex` (the delegate is the only toucher, so it's uncontended — the lock
// is there for `Send + Sync`).
// ---------------------------------------------------------------------------

/// Callback + reusable repack scratch, shared with the delegate. Holds no
/// Obj-C handle, so it is `Send + Sync`.
struct State {
    writer: FrameWriter,
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
                if CVPixelBufferLockBaseAddress(pixel_buffer, LOCK_READ_ONLY) != 0 {
                    return;
                }
                let base = CVPixelBufferGetBaseAddress(pixel_buffer) as *const u8;
                let width = CVPixelBufferGetWidth(pixel_buffer);
                let height = CVPixelBufferGetHeight(pixel_buffer);
                let stride = CVPixelBufferGetBytesPerRow(pixel_buffer);

                if !base.is_null() && width > 0 && height > 0 && stride >= width * 4 {
                    let mut state = self.ivars().state.lock().unwrap();
                    let State { writer, scratch } = &mut *state;
                    repack_bgra_to_rgba(base, width, height, stride, scratch);
                    writer.write_rgba8(width as u32, height as u32, scratch);
                }

                CVPixelBufferUnlockBaseAddress(pixel_buffer, LOCK_READ_ONLY);
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
// Stream handle. Holds the session alive; drop stops capture and releases
// the queue. Not `Send` (Obj-C handles), matching the public docs.
// ---------------------------------------------------------------------------

pub(crate) struct StreamHandle {
    session: Retained<AnyObject>,
    _delegate: Retained<FrameDelegate>,
    _queue: Retained<AnyObject>,
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        unsafe {
            let _: () = msg_send![&*self.session, stopRunning];
        }
        // `_queue`/`_delegate` release here; the output drops its own retains
        // when the session deallocates.
    }
}

// ---------------------------------------------------------------------------
// Permission.
// ---------------------------------------------------------------------------

pub(crate) async fn request_permission() -> Result<(), CameraError> {
    let media_type = NSString::from_str(AV_MEDIA_TYPE_VIDEO);
    let status: i64 =
        unsafe { msg_send![class!(AVCaptureDevice), authorizationStatusForMediaType: &*media_type] };
    match status {
        AUTH_AUTHORIZED => Ok(()),
        AUTH_DENIED | AUTH_RESTRICTED => Err(CameraError::PermissionDenied),
        // NotDetermined (0): surface the OS prompt and await the result.
        _ => {
            let (tx, rx) = futures_channel::oneshot::channel::<bool>();
            let tx = std::cell::Cell::new(Some(tx));
            let block = RcBlock::new(move |granted: Bool| {
                if let Some(tx) = tx.take() {
                    let _ = tx.send(granted.as_bool());
                }
            });
            unsafe {
                let _: () = msg_send![
                    class!(AVCaptureDevice),
                    requestAccessForMediaType: &*media_type,
                    completionHandler: &*block,
                ];
            }
            match rx.await {
                Ok(true) => Ok(()),
                _ => Err(CameraError::PermissionDenied),
            }
        }
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

        // Delegate + private serial queue.
        let delegate: Retained<FrameDelegate> = {
            let this = FrameDelegate::alloc().set_ivars(DelegateIvars {
                state: Mutex::new(State {
                    writer,
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

        // Orient delivered frames upright. A phone's camera sensor is mounted
        // landscape, so without pinning the output connection's orientation the
        // CVPixelBuffers arrive rotated 90° relative to a portrait-held device
        // — the "rotation is wrong" symptom. Pinning Portrait makes EVERY
        // consumer (the iOS CALayer display, the CPU RGBA channel, a future
        // GPU compositor) receive upright frames, converging on the same
        // behavior the web backend gets for free from `getUserMedia`.
        //
        // iOS-only: this same file also drives macOS capture, where the
        // webcam is already landscape-natural and forcing Portrait would
        // rotate it WRONG. The branch reflects a real form-factor difference
        // (phone held portrait vs. a fixed landscape webcam), not a backend
        // hack — the output (upright frames) still converges across platforms.
        //
        // `videoOrientation` is deprecated on iOS 17 in favor of
        // `videoRotationAngle`, but remains the correct, honored API at the
        // framework's iOS-16 deployment floor (verified on iPhone X / 16.7).
        #[cfg(target_os = "ios")]
        {
            // AVCaptureVideoOrientationPortrait. The setter takes an
            // `AVCaptureVideoOrientation` (NSInteger == isize).
            const AV_VIDEO_ORIENTATION_PORTRAIT: isize = 1;
            let media_type = NSString::from_str(AV_MEDIA_TYPE_VIDEO);
            let connection: Option<Retained<AnyObject>> =
                msg_send_id![&*output, connectionWithMediaType: &*media_type];
            if let Some(connection) = connection {
                let supported: Bool = msg_send![&*connection, isVideoOrientationSupported];
                if supported.as_bool() {
                    let _: () = msg_send![
                        &*connection,
                        setVideoOrientation: AV_VIDEO_ORIENTATION_PORTRAIT
                    ];
                }
            }
        }

        let _: () = msg_send![&*session, commitConfiguration];
        let _: () = msg_send![&*session, startRunning];

        // Apple exposes no zero-copy native source yet (CVPixelBuffer→Metal
        // via CVMetalTextureCache is the GPU-pipeline phase); frames flow
        // through the CPU channel.
        Ok((
            StreamHandle {
                session,
                _delegate: delegate,
                _queue: queue,
            },
            None,
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
