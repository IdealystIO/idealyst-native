//! iOS capture backend — ReplayKit in-app capture.
//!
//! `RPScreenRecorder.sharedRecorder.startCaptureWithHandler:completionHandler:`
//! delivers the app's own rendered screen as `CMSampleBuffer`s on an internal
//! background queue. We crack each *video* sample buffer into its
//! `CVPixelBuffer`, repack it to tightly-packed top-down RGBA8, and push it
//! into the [`FrameWriter`] — so a `ScreenRecorder` yields the same
//! [`MediaStream`](crate::MediaStream) the `camera` SDK does, and the `video`
//! SDK can display it with zero platform types in app code.
//!
//! ## Pixel format
//!
//! Unlike `AVCaptureVideoDataOutput` (where you set `videoSettings` to force
//! BGRA), ReplayKit gives you whatever it captures — in practice
//! `420YpCbCr8BiPlanar` (NV12, full- or video-range). So we read the pixel
//! buffer's actual format at runtime and convert: NV12→RGBA (both ranges) or
//! a BGRA fast-path swizzle if a future iOS ever delivers BGRA. Unknown
//! formats are skipped rather than rendered as garbage.
//!
//! ## Scope
//!
//! In-app capture only — ReplayKit's `startCapture` records the app's own
//! UIWindow content. System-wide capture (other apps) needs a Broadcast
//! Upload Extension (a separate build target) and is out of scope here.
//! [`Source::Window`] → [`RecorderError::UnsupportedSource`]; `ThisApp`,
//! `FullScreen`, and `UserChoice` all resolve to the in-app capture.
//!
//! ## Orientation
//!
//! Screen capture is naturally upright — ReplayKit captures the already-
//! composited UI, not a sideways camera sensor — so no rotation fix-up is
//! needed (contrast the `camera` SDK, which pins the capture connection to
//! Portrait).

use crate::{NativeSource, RecorderError, RecordingConfig, Source};
use block2::RcBlock;
use media_stream::FrameWriter;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2::{class, msg_send, msg_send_id};
use objc2_foundation::NSString;
use std::ffi::c_void;
use std::sync::Mutex;

// ---------------------------------------------------------------------------
// Foreign surfaces. ReplayKit classes are reached by name via `class!`, so
// the framework must be linked into the process; the empty extern block
// forces that. CoreMedia/CoreVideo expose the C functions that crack a sample
// buffer open into raw pixels — same posture as the `camera` SDK's apple.rs.
// ---------------------------------------------------------------------------

#[link(name = "ReplayKit", kind = "framework")]
extern "C" {}

#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    fn CVPixelBufferLockBaseAddress(pb: *mut c_void, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(pb: *mut c_void, flags: u64) -> i32;
    fn CVPixelBufferGetWidth(pb: *mut c_void) -> usize;
    fn CVPixelBufferGetHeight(pb: *mut c_void) -> usize;
    fn CVPixelBufferGetPixelFormatType(pb: *mut c_void) -> u32;
    // Packed (BGRA) accessors.
    fn CVPixelBufferGetBaseAddress(pb: *mut c_void) -> *mut c_void;
    fn CVPixelBufferGetBytesPerRow(pb: *mut c_void) -> usize;
    // Planar (NV12) accessors.
    fn CVPixelBufferGetBaseAddressOfPlane(pb: *mut c_void, plane: usize) -> *mut c_void;
    fn CVPixelBufferGetBytesPerRowOfPlane(pb: *mut c_void, plane: usize) -> usize;
}

#[link(name = "CoreMedia", kind = "framework")]
extern "C" {
    fn CMSampleBufferGetImageBuffer(sbuf: *mut c_void) -> *mut c_void;
}

/// `kCVPixelBufferLock_ReadOnly` — we only read the captured pixels.
const LOCK_READ_ONLY: u64 = 0x0000_0001;

/// `'BGRA'` (`kCVPixelFormatType_32BGRA`).
const PIXEL_FORMAT_32BGRA: u32 = 0x4247_5241;
/// `'420f'` (`kCVPixelFormatType_420YpCbCr8BiPlanarFullRange`) — ReplayKit's
/// usual delivery format.
const PIXEL_FORMAT_420F: u32 = 0x3432_3066;
/// `'420v'` (`kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange`).
const PIXEL_FORMAT_420V: u32 = 0x3432_3076;

/// `RPSampleBufferType.video`. The capture handler also fires for app/mic
/// audio (2/3); we only consume video.
const RP_SAMPLE_BUFFER_TYPE_VIDEO: isize = 1;
/// `RPRecordingErrorCode.userDeclined` — the user dismissed the capture
/// consent prompt. Mapped to [`RecorderError::PermissionDenied`].
const RP_ERROR_USER_DECLINED: isize = -5803;

// ---------------------------------------------------------------------------
// Capture state shared with the ReplayKit handler block.
// ---------------------------------------------------------------------------

/// The `FrameWriter` + reusable repack scratch, owned by the capture block.
/// ReplayKit invokes the block on an internal background queue, so this is
/// behind a `Mutex` (the block is the only toucher and frames arrive serially,
/// so it's uncontended — the lock is there for `Send + Sync`), mirroring the
/// `camera` delegate's `Mutex<State>`.
struct State {
    writer: FrameWriter,
    /// Publishes the frame's `CMSampleBuffer` for the zero-copy display path
    /// (`AVSampleBufferDisplayLayer` renders NV12/BGRA on the GPU directly).
    /// Always published; the CPU NV12/BGRA→RGBA conversion only runs when a
    /// consumer taps RGBA frames. See [`FrameWriter::wants_cpu_frames`].
    surf: media_stream::SurfaceWriter,
    scratch: Vec<u8>,
}

/// No pre-prompt on iOS: ReplayKit shows its capture-consent alert when
/// [`start`] calls `startCapture`. Resolving `Ok` here just defers consent to
/// that call (same shape as the web `getDisplayMedia` picker).
pub(crate) async fn request_permission(_source: &Source) -> Result<(), RecorderError> {
    Ok(())
}

pub(crate) async fn start(
    config: RecordingConfig,
    writer: FrameWriter,
) -> Result<(Recording, Option<NativeSource>), RecorderError> {
    // A specific *other* window can't be captured in-process on iOS.
    if matches!(config.source, Source::Window(_)) {
        return Err(RecorderError::UnsupportedSource("window"));
    }

    // The shared singleton recorder.
    let recorder: Retained<AnyObject> =
        unsafe { msg_send_id![class!(RPScreenRecorder), sharedRecorder] };

    let available: Bool = unsafe { msg_send![&recorder, isAvailable] };
    if !available.as_bool() {
        return Err(RecorderError::Platform(
            "screen recording is unavailable on this device".to_string(),
        ));
    }

    // Zero-copy display channel. The capture handler publishes each video
    // frame's CMSampleBuffer through `surf_writer`; `surf_source` is returned as
    // the stream's `native_source` for the `video` SDK's AVSampleBufferDisplayLayer.
    let (surf_source, surf_writer) = media_stream::surface_channel();

    // Capture handler: fires per sample buffer on ReplayKit's internal queue.
    // It owns the `Mutex<State>`; only video buffers are repacked + written.
    let state = Mutex::new(State {
        writer,
        surf: surf_writer,
        scratch: Vec::new(),
    });
    let capture_handler = RcBlock::new(
        move |sample_buffer: *mut AnyObject, buffer_type: isize, _error: *mut AnyObject| {
            if buffer_type != RP_SAMPLE_BUFFER_TYPE_VIDEO {
                return;
            }
            // SAFETY: ReplayKit hands us a valid CMSampleBuffer wrapping a
            // CVPixelBuffer for video sample types.
            unsafe { write_video_frame(sample_buffer as *mut c_void, &state) };
        },
    );

    // Completion handler: ReplayKit calls it once when capture has started
    // (nil error) or failed / been declined. Bridge it to async via a oneshot.
    let (tx, rx) = futures_channel::oneshot::channel::<Result<(), RecorderError>>();
    let tx = std::cell::Cell::new(Some(tx));
    let completion = RcBlock::new(move |error: *mut AnyObject| {
        let result = if error.is_null() {
            Ok(())
        } else {
            Err(unsafe { nserror_to_recorder(error) })
        };
        if let Some(tx) = tx.take() {
            let _ = tx.send(result);
        }
    });

    unsafe {
        let _: () = msg_send![
            &recorder,
            startCaptureWithHandler: &*capture_handler,
            completionHandler: &*completion,
        ];
    }

    // Await the start result. A dropped sender (ReplayKit never called back)
    // surfaces as a platform error rather than hanging.
    match rx.await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(e),
        Err(_) => {
            return Err(RecorderError::Platform(
                "ReplayKit start completion was dropped".to_string(),
            ))
        }
    }

    // Hold the recorder + the capture block alive for the recording's
    // lifetime. ReplayKit copies the block, but keeping our `RcBlock` is
    // explicit and harmless. Hand back the CMSampleBuffer source as the
    // stream's zero-copy native handle for AVSampleBufferDisplayLayer display.
    Ok((
        Recording {
            recorder,
            _capture_handler: capture_handler,
        },
        Some(std::rc::Rc::new(surf_source) as NativeSource),
    ))
}

/// A live ReplayKit recording. Dropping it stops capture and releases the
/// session — the `MediaStream` stopper owns this, so the last stream clone
/// dropping tears capture down.
pub(crate) struct Recording {
    recorder: Retained<AnyObject>,
    // Kept alive so the handler stays valid for the recording's lifetime.
    _capture_handler: RcBlock<dyn Fn(*mut AnyObject, isize, *mut AnyObject)>,
}

impl Drop for Recording {
    fn drop(&mut self) {
        // `stopCaptureWithHandler:` wants a completion block; pass a no-op
        // (a nil block would crash). Fire-and-forget — teardown completes
        // asynchronously on ReplayKit's side.
        let noop = RcBlock::new(|_error: *mut AnyObject| {});
        unsafe {
            let _: () = msg_send![&self.recorder, stopCaptureWithHandler: &*noop];
        }
    }
}

// ---------------------------------------------------------------------------
// Pixel cracking.
// ---------------------------------------------------------------------------

/// Lock the sample buffer's `CVPixelBuffer`, convert to RGBA8 by its actual
/// format, and write it. Unknown formats are skipped (not rendered as
/// garbage).
///
/// # Safety
/// `sbuf` must be a valid `CMSampleBufferRef` wrapping a `CVPixelBuffer`.
unsafe fn write_video_frame(sbuf: *mut c_void, state: &Mutex<State>) {
    let pb = CMSampleBufferGetImageBuffer(sbuf);
    if pb.is_null() {
        return;
    }

    // Zero-copy display: publish the CMSampleBuffer (retain + pointer swap) for
    // the AVSampleBufferDisplayLayer path — it renders NV12/BGRA on the GPU with
    // no CPU touch. Always published; the (expensive, full-frame) NV12/BGRA→RGBA
    // conversion below runs only when a subscriber taps CPU frames, so a
    // preview-only session pays zero per-pixel CPU cost.
    let wants_cpu = {
        let guard = state.lock().unwrap();
        guard.surf.publish(sbuf as *const c_void);
        guard.writer.wants_cpu_frames()
    };
    if !wants_cpu {
        return;
    }

    if CVPixelBufferLockBaseAddress(pb, LOCK_READ_ONLY) != 0 {
        return;
    }

    let format = CVPixelBufferGetPixelFormatType(pb);
    let width = CVPixelBufferGetWidth(pb);
    let height = CVPixelBufferGetHeight(pb);

    if width > 0 && height > 0 {
        let mut guard = state.lock().unwrap();
        let State { writer, scratch, .. } = &mut *guard;
        match format {
            PIXEL_FORMAT_32BGRA => {
                let base = CVPixelBufferGetBaseAddress(pb) as *const u8;
                let stride = CVPixelBufferGetBytesPerRow(pb);
                if !base.is_null() && stride >= width * 4 {
                    repack_bgra_to_rgba(base, width, height, stride, scratch);
                    writer.write_rgba8(width as u32, height as u32, scratch);
                }
            }
            PIXEL_FORMAT_420F | PIXEL_FORMAT_420V => {
                let y_base = CVPixelBufferGetBaseAddressOfPlane(pb, 0) as *const u8;
                let c_base = CVPixelBufferGetBaseAddressOfPlane(pb, 1) as *const u8;
                let y_stride = CVPixelBufferGetBytesPerRowOfPlane(pb, 0);
                let c_stride = CVPixelBufferGetBytesPerRowOfPlane(pb, 1);
                if !y_base.is_null() && !c_base.is_null() && y_stride >= width && c_stride >= width
                {
                    let full_range = format == PIXEL_FORMAT_420F;
                    nv12_to_rgba(
                        y_base, y_stride, c_base, c_stride, width, height, full_range, scratch,
                    );
                    writer.write_rgba8(width as u32, height as u32, scratch);
                }
            }
            // Unknown format — skip rather than emit garbage.
            _ => {}
        }
    }

    CVPixelBufferUnlockBaseAddress(pb, LOCK_READ_ONLY);
}

/// Copy a strided `BGRA` image into a tightly-packed top-down `RGBA8` buffer,
/// swizzling `B`/`R`. `scratch` is reused across frames.
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

/// Convert an `NV12` (`420YpCbCr8BiPlanar`) image to tightly-packed top-down
/// `RGBA8`. Plane 0 is full-res luma (`Y`); plane 1 is half-res interleaved
/// chroma (`Cb`,`Cr`). Uses BT.601 fixed-point math (`<<8` scale) so the hot
/// loop stays integer-only — the CPU path is the universal fallback; a
/// zero-copy `CVPixelBuffer`→Metal import is the GPU phase.
///
/// `full_range` selects the `420f` (full-range) vs `420v` (video-range)
/// coefficients. `scratch` is reused across frames.
///
/// # Safety
/// `y_base` must point at `height * y_stride` readable bytes (≥ `width`/row);
/// `c_base` at `(height/2) * c_stride` readable bytes (≥ `width`/row).
#[allow(clippy::too_many_arguments)]
unsafe fn nv12_to_rgba(
    y_base: *const u8,
    y_stride: usize,
    c_base: *const u8,
    c_stride: usize,
    width: usize,
    height: usize,
    full_range: bool,
    scratch: &mut Vec<u8>,
) {
    let row_bytes = width * 4;
    scratch.clear();
    scratch.resize(row_bytes * height, 0);
    for y in 0..height {
        let y_row = std::slice::from_raw_parts(y_base.add(y * y_stride), width);
        // Chroma is subsampled 2× vertically: row y uses chroma row y/2.
        let c_row = std::slice::from_raw_parts(c_base.add((y / 2) * c_stride), width);
        let dst_row = &mut scratch[y * row_bytes..(y + 1) * row_bytes];
        for x in 0..width {
            let yv = y_row[x] as i32;
            // Chroma is subsampled 2× horizontally; Cb,Cr interleaved.
            let cb = c_row[(x / 2) * 2] as i32 - 128;
            let cr = c_row[(x / 2) * 2 + 1] as i32 - 128;
            let (r, g, b) = if full_range {
                // 420f: R=Y+1.402Cr, G=Y-0.344Cb-0.714Cr, B=Y+1.772Cb.
                (
                    yv + ((359 * cr) >> 8),
                    yv - ((88 * cb + 183 * cr) >> 8),
                    yv + ((454 * cb) >> 8),
                )
            } else {
                // 420v: Y'=1.164(Y-16); R=Y'+1.596Cr, G=Y'-0.391Cb-0.813Cr,
                // B=Y'+2.018Cb.
                let yy = (298 * (yv - 16)) >> 8;
                (
                    yy + ((409 * cr) >> 8),
                    yy - ((100 * cb + 208 * cr) >> 8),
                    yy + ((516 * cb) >> 8),
                )
            };
            let d = &mut dst_row[x * 4..x * 4 + 4];
            d[0] = r.clamp(0, 255) as u8;
            d[1] = g.clamp(0, 255) as u8;
            d[2] = b.clamp(0, 255) as u8;
            d[3] = 255;
        }
    }
}

/// Read an `NSError` into a [`RecorderError`], mapping ReplayKit's
/// user-declined code to [`RecorderError::PermissionDenied`].
///
/// # Safety
/// `error` must be a non-null valid `NSError *`.
unsafe fn nserror_to_recorder(error: *mut AnyObject) -> RecorderError {
    let err = &*error;
    let code: isize = msg_send![err, code];
    if code == RP_ERROR_USER_DECLINED {
        return RecorderError::PermissionDenied;
    }
    let desc: Retained<NSString> = msg_send_id![err, localizedDescription];
    RecorderError::Platform(desc.to_string())
}

// ===========================================================================
// Private layer — ReplayKit-excluded overlay window.
// ===========================================================================

use backend_ios::IosBackend;

/// Install the `PrivateLayer` external handler against an `IosBackend`.
///
/// The handler asks the backend to build a separate, ReplayKit-excluded
/// `UIWindow` (see `IosBackend::create_private_layer_window`) and returns
/// its content view. The framework's External walker then parents the
/// layer's children into that content view; the backend's `insert`
/// skips reparenting it into the main (recorded) tree because the
/// content view is registered as a detached window root.
///
/// ReplayKit records the app's key window only; the overlay lives on a
/// separate non-key window at a high `windowLevel`, so it's visible to
/// the user but absent from the recording. The orchestrator verifies
/// this on a real device.
pub fn register(backend: &mut IosBackend) {
    backend.register_external::<crate::PrivateLayerProps, _>(|_props, b| {
        b.create_private_layer_window()
    });
}
