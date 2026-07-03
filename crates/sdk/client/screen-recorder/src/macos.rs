//! macOS capture backend — ScreenCaptureKit (`SCStream`).
//!
//! `SCShareableContent.getShareableContentWithCompletionHandler:` enumerates
//! the displays / windows / applications the process may capture (the first
//! call triggers the Screen Recording TCC prompt). We build an
//! `SCContentFilter` (whole display, or just this app), an
//! `SCStreamConfiguration` pinned to `kCVPixelFormatType_32BGRA`, add an
//! `SCStreamOutput` delegate on a private serial dispatch queue, and
//! `startCaptureWithCompletionHandler:`. Each delivered `CMSampleBuffer`
//! wraps a BGRA `CVPixelBuffer`; we repack it (stripping `bytesPerRow`
//! padding) and push it into the [`FrameWriter`] — so a `ScreenRecorder`
//! yields the same [`MediaStream`](crate::MediaStream) the `camera` SDK does.
//!
//! We drive ScreenCaptureKit through the Obj-C runtime (no typed framework
//! crate), exactly like the `camera` SDK drives AVFoundation: classes are
//! reached by name via `class!`, the framework is force-linked with an empty
//! `#[link]` extern block, and the few CoreVideo/CoreMedia C functions that
//! crack a sample buffer open are declared `extern "C"`. The completion
//! handlers are bridged to `async` with a `futures-channel` oneshot, the same
//! callback→future pattern `camera`'s permission flow uses.
//!
//! ## Pixel format
//!
//! Unlike ReplayKit (which delivers whatever it captures, usually NV12),
//! ScreenCaptureKit lets us *request* a pixel format — we ask for `'BGRA'`,
//! so every frame arrives packed BGRA and the conversion is a single B/R
//! swizzle (`FrameWriter::write_bgra8` does the swizzle; we only repack to
//! strip row padding first).
//!
//! ## Scope
//!
//! `FullScreen` / `UserChoice` capture the primary display; `ThisApp`
//! captures only the current process's windows (via an
//! `SCContentFilter(display:including/excluding…)` keyed off the running
//! application's pid). `Source::Window` is not yet wired and returns
//! [`RecorderError::UnsupportedSource`]. Private-layer overlays (the
//! `PrivateLayer` external) are excluded from the recording when matching
//! `SCWindow`s are found (see `start`).
//!
//! Requires macOS 12.3+ (ScreenCaptureKit); the app-/window-exclusion filter
//! initializers used here exist on 13+.

use crate::{NativeSource, RecorderError, RecordingConfig, Source};
use block2::RcBlock;
use media_stream::FrameWriter;
use objc2::encode::{Encode, Encoding};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol};
use objc2::{class, declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_foundation::{NSObject, NSString};
use std::ffi::c_void;
use std::ptr;
use std::sync::Mutex;

// ---------------------------------------------------------------------------
// Foreign surfaces. ScreenCaptureKit classes are reached by name via `class!`,
// so the framework must be linked into the process; the empty extern block
// forces that. CoreMedia/CoreVideo expose the C functions that crack a sample
// buffer open into raw pixels — same posture as the `camera` SDK's apple.rs.
// ---------------------------------------------------------------------------

#[link(name = "ScreenCaptureKit", kind = "framework")]
extern "C" {}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    /// Whether this process already has Screen Recording (TCC) access. Does
    /// NOT prompt. (macOS 10.15+.)
    fn CGPreflightScreenCaptureAccess() -> bool;
    /// Request Screen Recording access: on first use shows the system dialog
    /// (which offers "Open System Settings") and registers the app in the
    /// Screen Recording privacy list. Returns the current grant state — `false`
    /// the first time, because the user must enable it in Settings and RELAUNCH
    /// the app before capture works.
    fn CGRequestScreenCaptureAccess() -> bool;
}

/// Open System Settings → Privacy & Security → Screen Recording, so a user
/// without permission is taken straight to where they grant it.
fn open_screen_recording_settings() {
    let url_str = NSString::from_str(
        "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
    );
    unsafe {
        let url: Option<Retained<NSObject>> =
            msg_send_id![class!(NSURL), URLWithString: &*url_str];
        if let Some(url) = url {
            let ws: Retained<NSObject> = msg_send_id![class!(NSWorkspace), sharedWorkspace];
            let _: bool = msg_send![&ws, openURL: &*url];
        }
    }
}

/// Ensure Screen Recording permission. Preflight first (no prompt); if not
/// granted, `CGRequestScreenCaptureAccess` shows the system dialog AND registers
/// the app in the privacy list, then — since the first grant needs a relaunch —
/// we open System Settings and return `PermissionDenied` so the caller can
/// surface "enable Screen Recording and relaunch". This is the canonical macOS
/// flow; `SCShareableContent` alone doesn't reliably present the prompt.
fn ensure_screen_capture_permission() -> Result<(), RecorderError> {
    if unsafe { CGPreflightScreenCaptureAccess() } {
        return Ok(());
    }
    if unsafe { CGRequestScreenCaptureAccess() } {
        return Ok(());
    }
    open_screen_recording_settings();
    Err(RecorderError::PermissionDenied)
}

#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    fn CVPixelBufferLockBaseAddress(pb: *mut c_void, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(pb: *mut c_void, flags: u64) -> i32;
    fn CVPixelBufferGetBaseAddress(pb: *mut c_void) -> *mut c_void;
    fn CVPixelBufferGetWidth(pb: *mut c_void) -> usize;
    fn CVPixelBufferGetHeight(pb: *mut c_void) -> usize;
    fn CVPixelBufferGetBytesPerRow(pb: *mut c_void) -> usize;
    /// The `IOSurface` backing the pixel buffer (SCK frames are always
    /// IOSurface-backed), or null. Borrowed — retain it (the `SurfaceWriter`
    /// does) to keep it past the buffer's recycle. Enables the zero-copy
    /// display fast-path: `CALayer.contents = IOSurface`, no CPU swizzle.
    fn CVPixelBufferGetIOSurface(pb: *mut c_void) -> *const c_void;
}

#[link(name = "CoreMedia", kind = "framework")]
extern "C" {
    fn CMSampleBufferGetImageBuffer(sbuf: *mut c_void) -> *mut c_void;
    /// `CMSampleBufferIsValid` — SCK can deliver a status/idle sample buffer
    /// with no image; guard against cracking those.
    fn CMSampleBufferIsValid(sbuf: *mut c_void) -> objc2::runtime::Bool;
    fn CMTimeMake(value: i64, timescale: i32) -> CMTime;
}

extern "C" {
    /// Serial queue for the `SCStreamOutput` delegate callbacks. In libSystem
    /// (always linked). Same call the `camera` SDK uses.
    fn dispatch_queue_create(label: *const std::ffi::c_char, attr: *mut c_void) -> *mut c_void;
}

/// `CMTime`. Passed by value into `SCStreamConfiguration.setMinimumFrameInterval:`,
/// so it must implement [`Encode`]. Layout matches Apple's `CMTime` exactly
/// (int64 value, int32 timescale, uint32 flags, int64 epoch).
#[repr(C)]
#[derive(Clone, Copy)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

// SAFETY: the field layout + encoding match Apple's `CMTime` exactly, so objc2
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

/// `'BGRA'` as an `OSType` (`kCVPixelFormatType_32BGRA`). We request this
/// from `SCStreamConfiguration` so frames arrive packed BGRA — a single
/// B/R swizzle to RGBA (done by `FrameWriter::write_bgra8`).
const PIXEL_FORMAT_32BGRA: u32 = 0x4247_5241;
/// `kCVPixelBufferLock_ReadOnly` — we only read the captured pixels.
const LOCK_READ_ONLY: u64 = 0x0000_0001;
/// `SCStreamOutputType.screen`. The output type we register + filter on in
/// the delegate (audio is type 1, microphone 2 — not consumed here).
///
/// SCK marks idle/blank frames (no new pixels) with a non-complete
/// `SCFrameStatus` in the sample buffer's attachments; rather than read the
/// attachment dict, the delegate relies on `CMSampleBufferIsValid` +
/// `CMSampleBufferGetImageBuffer` returning null for those image-less buffers.
const SC_STREAM_OUTPUT_TYPE_SCREEN: isize = 0;

// ---------------------------------------------------------------------------
// Delegate. Receives `stream:didOutputSampleBuffer:ofType:` on the serial
// queue and bridges each frame to the writer through a `Mutex` (the delegate
// is the only toucher and frames arrive serially, so it's uncontended — the
// lock is there for `Send + Sync`). Mirrors the `camera` SDK's FrameDelegate.
// ---------------------------------------------------------------------------

/// The `FrameWriter` + zero-copy `SurfaceWriter` + reusable repack scratch,
/// shared with the delegate. Holds no Obj-C handle, so it is `Send + Sync`.
struct State {
    writer: FrameWriter,
    /// Publishes the frame's `IOSurface` for the zero-copy display fast-path.
    /// Always published (cheap: retain + pointer swap); the CPU `writer` path
    /// below only runs when a consumer actually taps RGBA frames.
    surf: media_stream::SurfaceWriter,
    /// Reusable tightly-packed BGRA buffer (row padding stripped) so
    /// steady-state capture doesn't allocate per frame.
    scratch: Vec<u8>,
}

pub(crate) struct DelegateIvars {
    state: Mutex<State>,
}

declare_class!(
    struct StreamOutput;

    unsafe impl ClassType for StreamOutput {
        type Super = NSObject;
        type Mutability = mutability::InteriorMutable;
        const NAME: &'static str = "IdealystScreenRecorderStreamOutput";
    }

    impl DeclaredClass for StreamOutput {
        type Ivars = DelegateIvars;
    }

    unsafe impl NSObjectProtocol for StreamOutput {}

    unsafe impl StreamOutput {
        // `SCStreamOutput`. We don't import the typed protocol (no
        // objc2-screen-capture-kit dep); SCK calls the selector by
        // `respondsToSelector:`, so implementing the method is enough. The
        // CF/obj-c args are taken as raw pointers (same bits in-register
        // regardless of the formal encoding) and cast for the C calls below.
        #[method(stream:didOutputSampleBuffer:ofType:)]
        fn did_output(
            &self,
            _stream: *mut AnyObject,
            sample_buffer: *mut AnyObject,
            output_type: isize,
        ) {
            // Only screen frames; ignore audio/mic types if ever delivered.
            if output_type != SC_STREAM_OUTPUT_TYPE_SCREEN {
                return;
            }
            let sbuf = sample_buffer as *mut c_void;
            if sbuf.is_null() {
                return;
            }
            // SAFETY: SCK hands us a valid CMSampleBuffer for screen output;
            // each accessor is a documented CoreMedia/CoreVideo C call on it.
            unsafe {
                if !CMSampleBufferIsValid(sbuf).as_bool() {
                    return;
                }
                let pixel_buffer = CMSampleBufferGetImageBuffer(sbuf);
                if pixel_buffer.is_null() {
                    // Status-only sample buffer (idle/blank frame) — no image.
                    return;
                }

                // One lock for the whole frame — the delegate is the sole
                // toucher of `State` and frames arrive serially, so it's
                // uncontended (the `Mutex` is only there for `Send`).
                let mut state = self.ivars().state.lock().unwrap();

                // Zero-copy display fast-path: publish the frame's IOSurface
                // (retain + pointer swap, microseconds) so the `video` SDK can
                // set it straight as `CALayer.contents` — no CPU swizzle, no
                // CGImage, no per-frame upload. This is what keeps the live
                // preview low-latency: the heavy CPU path below previously
                // backed up SCK's bounded frame queue (≈ queueDepth × per-frame
                // swizzle time of latency); the surface publish never stalls.
                let surface = CVPixelBufferGetIOSurface(pixel_buffer);
                if !surface.is_null() {
                    state.surf.publish(surface);
                }

                // CPU RGBA channel: only do the (expensive, full-frame) repack +
                // BGRA→RGBA swizzle when a consumer is actually tapping CPU
                // frames (a `subscribe`r — e.g. a file encoder). Pure
                // native-source display reads the IOSurface above and needs
                // none of this, so a preview-only session pays zero per-pixel
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
                        // Repack to tightly-packed BGRA (strip `bytesPerRow`
                        // padding); `write_bgra8` swizzles B/R → RGBA. It rejects
                        // a frame shorter than width*height*4, so the packed
                        // scratch must be exactly that size.
                        repack_strided_bgra(base, width, height, stride, scratch);
                        writer.write_bgra8(width as u32, height as u32, scratch);
                    }

                    CVPixelBufferUnlockBaseAddress(pixel_buffer, LOCK_READ_ONLY);
                }
            }
        }
    }
);

/// Copy a strided BGRA image into a tightly-packed (row-padding-stripped)
/// BGRA buffer. `scratch` is reused across frames. The downstream
/// `write_bgra8` does the B/R swizzle to RGBA; this only removes the
/// `bytesPerRow` padding so each row is exactly `width * 4` bytes.
///
/// # Safety
/// `base` must point at `height * stride` readable bytes with at least
/// `width * 4` valid bytes per row.
unsafe fn repack_strided_bgra(
    base: *const u8,
    width: usize,
    height: usize,
    stride: usize,
    scratch: &mut Vec<u8>,
) {
    let row_bytes = width * 4;
    scratch.clear();
    scratch.resize(row_bytes * height, 0);
    if stride == row_bytes {
        // No padding — one contiguous copy.
        let src = std::slice::from_raw_parts(base, row_bytes * height);
        scratch.copy_from_slice(src);
        return;
    }
    for y in 0..height {
        let src_row = std::slice::from_raw_parts(base.add(y * stride), row_bytes);
        let dst_row = &mut scratch[y * row_bytes..(y + 1) * row_bytes];
        dst_row.copy_from_slice(src_row);
    }
}

// ---------------------------------------------------------------------------
// Recording handle. Holds the stream alive; drop stops capture and releases
// the queue + delegate. Not `Send` (Obj-C handles), matching the public docs.
// ---------------------------------------------------------------------------

pub(crate) struct Recording {
    stream: Retained<AnyObject>,
    _delegate: Retained<StreamOutput>,
    _queue: Retained<AnyObject>,
}

impl Drop for Recording {
    fn drop(&mut self) {
        // `stopCaptureWithCompletionHandler:` wants a completion block; pass a
        // no-op (a nil block would crash). Fire-and-forget — teardown
        // completes asynchronously on SCK's side, after which the stream's
        // retains on the queue + delegate release with this handle.
        let noop = RcBlock::new(|_error: *mut AnyObject| {});
        unsafe {
            let _: () = msg_send![&self.stream, stopCaptureWithCompletionHandler: &*noop];
        }
    }
}

// ---------------------------------------------------------------------------
// Permission. The first `getShareableContentWithCompletionHandler:` triggers
// the Screen Recording TCC prompt; resolving content confirms consent.
// ---------------------------------------------------------------------------

pub(crate) async fn request_permission(_source: &Source) -> Result<(), RecorderError> {
    // Canonical Screen Recording permission flow: preflight, prompt via
    // `CGRequestScreenCaptureAccess` (which also registers the app + offers a
    // Settings shortcut), and deep-link to Settings if still not granted.
    ensure_screen_capture_permission()
}

// ---------------------------------------------------------------------------
// Start.
// ---------------------------------------------------------------------------

pub(crate) async fn start(
    config: RecordingConfig,
    writer: FrameWriter,
) -> Result<(Recording, Option<NativeSource>), RecorderError> {
    // A specific *other* window isn't wired yet.
    if matches!(config.source, Source::Window(_)) {
        return Err(RecorderError::UnsupportedSource("window"));
    }

    // Gate on Screen Recording permission via the canonical CoreGraphics APIs
    // (prompt + Settings deep-link). Without this, `SCShareableContent` can
    // silently fail to present the system prompt.
    ensure_screen_capture_permission()?;

    // Enumerate shareable content (now permitted).
    let content = unsafe { shareable_content().await }?;

    // SAFETY: a straight transcription of the documented ScreenCaptureKit
    // setup (content → display → filter → configuration → stream → output on a
    // serial queue → startCapture). Each `msg_send` targets a method on the
    // class it's sent to; failures are checked and mapped to `RecorderError`.
    unsafe {
        // The primary display. `SCShareableContent.displays` is an
        // `NSArray<SCDisplay *>`; the first is the main display.
        let displays: Retained<AnyObject> = msg_send_id![&content, displays];
        let display_count: usize = msg_send![&displays, count];
        if display_count == 0 {
            return Err(RecorderError::Platform("no shareable display".into()));
        }
        let display: Retained<AnyObject> = msg_send_id![&displays, objectAtIndex: 0usize];

        // Native pixel size for the stream config when the caller didn't pin
        // one. `-[SCDisplay width]`/`height` return `NSInteger` (signed `'q'`,
        // i.e. `isize`) — reading them as `usize` (`'Q'`) trips objc2's encoding
        // verifier and SIGABRTs inside the Record handler. Display dimensions
        // are positive, so widen back to `usize` for the config after.
        let disp_w: isize = msg_send![&display, width];
        let disp_h: isize = msg_send![&display, height];
        let (width, height) = match config.size {
            Some((w, h)) => (w as usize, h as usize),
            None => (disp_w.max(1) as usize, disp_h.max(1) as usize),
        };

        // Build the content filter. For `ThisApp` include only the current
        // running application; otherwise capture the whole display. In both
        // cases exclude the registered private-layer overlay windows.
        let filter = build_content_filter(&content, &display, &config.source)?;

        // Stream configuration: BGRA, requested size + frame rate.
        let configuration: Retained<AnyObject> =
            msg_send_id![class!(SCStreamConfiguration), new];
        let _: () = msg_send![&configuration, setWidth: width];
        let _: () = msg_send![&configuration, setHeight: height];
        let _: () = msg_send![&configuration, setPixelFormat: PIXEL_FORMAT_32BGRA];
        let fps = config.fps.max(1);
        let interval = CMTimeMake(1, fps as i32);
        let _: () = msg_send![&configuration, setMinimumFrameInterval: interval];
        // A small queue depth bounds memory if a consumer stalls; 5 is
        // Apple's documented sample value.
        let _: () = msg_send![&configuration, setQueueDepth: 5isize];

        // The stream. `delegate:nil` — we register an `SCStreamOutput` for
        // frames separately; the stream-level delegate is only for
        // `stream:didStopWithError:`, which we don't need (Drop stops it).
        let stream: Retained<AnyObject> = {
            let alloc: *mut AnyObject = msg_send![class!(SCStream), alloc];
            let inited: *mut AnyObject = msg_send![
                alloc,
                initWithFilter: &*filter,
                configuration: &*configuration,
                delegate: ptr::null::<AnyObject>(),
            ];
            Retained::from_raw(inited)
                .ok_or_else(|| RecorderError::Platform("SCStream init returned nil".into()))?
        };

        // Zero-copy display channel. The delegate (capture queue) publishes
        // each frame's IOSurface through `surf_writer`; `surf_source` is handed
        // back as the stream's `native_source` so the `video` SDK displays it
        // with no CPU copy. Created before the delegate so its writer moves in.
        let (surf_source, surf_writer) = media_stream::surface_channel();

        // Output delegate on a private serial dispatch queue.
        let delegate: Retained<StreamOutput> = {
            let this = StreamOutput::alloc().set_ivars(DelegateIvars {
                state: Mutex::new(State {
                    writer,
                    surf: surf_writer,
                    scratch: Vec::new(),
                }),
            });
            msg_send_id![super(this), init]
        };
        let queue_raw =
            dispatch_queue_create(c"com.idealyst.screenrecorder".as_ptr(), ptr::null_mut());
        let queue: Retained<AnyObject> = Retained::from_raw(queue_raw.cast())
            .ok_or_else(|| RecorderError::Platform("dispatch_queue_create failed".into()))?;

        // `addStreamOutput:type:sampleHandlerQueue:error:` returns BOOL +
        // writes an NSError* on failure.
        let mut err: *mut AnyObject = ptr::null_mut();
        let added: objc2::runtime::Bool = msg_send![
            &stream,
            addStreamOutput: &*delegate,
            type: SC_STREAM_OUTPUT_TYPE_SCREEN,
            sampleHandlerQueue: &*queue,
            error: &mut err as *mut *mut AnyObject,
        ];
        if !added.as_bool() {
            return Err(nserror_to_recorder(err, "addStreamOutput failed"));
        }

        // Start capture; bridge the completion handler to async.
        let (tx, rx) = futures_channel::oneshot::channel::<Result<(), RecorderError>>();
        let tx = std::cell::Cell::new(Some(tx));
        let completion = RcBlock::new(move |error: *mut AnyObject| {
            let result = if error.is_null() {
                Ok(())
            } else {
                Err(nserror_to_recorder(error, "startCapture failed"))
            };
            if let Some(tx) = tx.take() {
                let _ = tx.send(result);
            }
        });
        let _: () = msg_send![&stream, startCaptureWithCompletionHandler: &*completion];

        match rx.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                return Err(RecorderError::Platform(
                    "ScreenCaptureKit start completion was dropped".into(),
                ))
            }
        }

        // Hand back the IOSurface source as the stream's zero-copy native
        // handle. The `video` SDK downcasts it and sets each frame's surface
        // straight as `CALayer.contents` — no CPU swizzle, no CGImage. The CPU
        // RGBA channel still serves `subscribe`rs (file encoders); it's just no
        // longer the display path.
        Ok((
            Recording {
                stream,
                _delegate: delegate,
                _queue: queue,
            },
            Some(std::rc::Rc::new(surf_source) as NativeSource),
        ))
    }
}

/// Build an `SCContentFilter` for `source` against `display`, excluding the
/// registered private-layer overlay windows so the recording doesn't show
/// the toolbar/preview (a feedback mirror).
///
/// # Safety
/// `content` is a valid `SCShareableContent`, `display` a valid `SCDisplay`.
unsafe fn build_content_filter(
    content: &AnyObject,
    display: &AnyObject,
    source: &Source,
) -> Result<Retained<AnyObject>, RecorderError> {
    // Resolve the overlay `SCWindow`s to exclude. The backend exposes their
    // `windowNumber`s; match each against `SCWindow.windowID`.
    let exclude = private_layer_scwindows(content);

    let inited: *mut AnyObject = match source {
        // Include only the current running application. Find this process's
        // `SCRunningApplication` (by pid) among `content.applications`.
        Source::ThisApp if current_sc_application(content).is_some() => {
            let app = current_sc_application(content).unwrap();
            let apps: Retained<AnyObject> =
                msg_send_id![class!(NSArray), arrayWithObject: &*app];
            // `initWithDisplay:includingApplications:exceptingWindows:`
            // captures the display but only the listed apps' windows, minus
            // the excepted windows (our overlays).
            let alloc: *mut AnyObject = msg_send![class!(SCContentFilter), alloc];
            msg_send![
                alloc,
                initWithDisplay: display,
                includingApplications: &*apps,
                exceptingWindows: &*exclude,
            ]
        }
        // FullScreen / UserChoice (no picker yet), or `ThisApp` when the
        // running app couldn't be resolved: whole display minus overlays.
        _ => {
            let alloc: *mut AnyObject = msg_send![class!(SCContentFilter), alloc];
            msg_send![
                alloc,
                initWithDisplay: display,
                excludingWindows: &*exclude,
            ]
        }
    };
    Retained::from_raw(inited)
        .ok_or_else(|| RecorderError::Platform("SCContentFilter init returned nil".into()))
}

/// The `SCRunningApplication` matching this process, or `None` if absent.
///
/// # Safety
/// `content` is a valid `SCShareableContent`.
unsafe fn current_sc_application(content: &AnyObject) -> Option<Retained<AnyObject>> {
    let pid = std::process::id() as i32;
    let apps: Retained<AnyObject> = msg_send_id![content, applications];
    let count: usize = msg_send![&apps, count];
    for i in 0..count {
        let app: Retained<AnyObject> = msg_send_id![&apps, objectAtIndex: i];
        // `SCRunningApplication.processID` is `pid_t` (int32).
        let app_pid: i32 = msg_send![&app, processID];
        if app_pid == pid {
            return Some(app);
        }
    }
    None
}

/// Resolve the registered private-layer overlay windows to an
/// `NSArray<SCWindow *>` for the filter's exclusion list. Matches the
/// backend's `windowNumber`s against `SCWindow.windowID`. Returns an empty
/// array when no overlay is mounted or none matches.
///
/// # Safety
/// `content` is a valid `SCShareableContent`.
unsafe fn private_layer_scwindows(content: &AnyObject) -> Retained<AnyObject> {
    let ids = backend_macos::private_layer_window_ids();
    if ids.is_empty() {
        return msg_send_id![class!(NSArray), array];
    }
    let windows: Retained<AnyObject> = msg_send_id![content, windows];
    let count: usize = msg_send![&windows, count];
    // `NSMutableArray` to collect matches.
    let matches: Retained<AnyObject> = msg_send_id![class!(NSMutableArray), array];
    for i in 0..count {
        let win: Retained<AnyObject> = msg_send_id![&windows, objectAtIndex: i];
        // `SCWindow.windowID` is a `CGWindowID` (uint32). The backend stores
        // `windowNumber` (NSInteger) which equals the CGWindowID for a real
        // on-screen NSWindow.
        let window_id: u32 = msg_send![&win, windowID];
        if ids.iter().any(|&id| id == window_id as i64) {
            let _: () = msg_send![&matches, addObject: &*win];
        }
    }
    matches
}

/// Await `SCShareableContent.getShareableContentWithCompletionHandler:`,
/// returning the content or mapping the error / nil to a [`RecorderError`].
/// The first call drives the Screen Recording TCC prompt.
///
/// # Safety
/// Issues the documented SCK class method with a matching completion block.
async unsafe fn shareable_content() -> Result<Retained<AnyObject>, RecorderError> {
    let (tx, rx) =
        futures_channel::oneshot::channel::<Result<Retained<AnyObject>, RecorderError>>();
    let tx = std::cell::Cell::new(Some(tx));
    let block = RcBlock::new(move |content: *mut AnyObject, error: *mut AnyObject| {
        let result = if !error.is_null() {
            Err(nserror_to_recorder(error, "getShareableContent failed"))
        } else if content.is_null() {
            Err(RecorderError::PermissionDenied)
        } else {
            // Retain the content for use after the block returns.
            match Retained::retain(content) {
                Some(c) => Ok(c),
                None => Err(RecorderError::Platform("nil shareable content".into())),
            }
        };
        if let Some(tx) = tx.take() {
            let _ = tx.send(result);
        }
    });
    let _: () = msg_send![
        class!(SCShareableContent),
        getShareableContentWithCompletionHandler: &*block,
    ];
    match rx.await {
        Ok(r) => r,
        Err(_) => Err(RecorderError::Platform(
            "getShareableContent completion was dropped".into(),
        )),
    }
}

/// Read an `NSError` into a [`RecorderError`], mapping the
/// ScreenCaptureKit user-declined code to [`RecorderError::PermissionDenied`].
/// `fallback` describes the call site when the error is null/unreadable.
///
/// Safe to call with a null `error` (returns a `Platform(fallback)` error).
fn nserror_to_recorder(error: *mut AnyObject, fallback: &str) -> RecorderError {
    if error.is_null() {
        return RecorderError::Platform(fallback.to_string());
    }
    // SAFETY: caller passes a valid `NSError *` (or null, handled above).
    unsafe {
        let err = &*error;
        // `SCStreamErrorCode.userDeclined` == -3801 in the `SCStreamError`
        // domain — the user dismissed the Screen Recording consent.
        const SC_ERROR_USER_DECLINED: isize = -3801;
        let code: isize = msg_send![err, code];
        if code == SC_ERROR_USER_DECLINED {
            return RecorderError::PermissionDenied;
        }
        let desc: *mut NSString = msg_send![err, localizedDescription];
        if desc.is_null() {
            return RecorderError::Platform(fallback.to_string());
        }
        RecorderError::Platform((*desc).to_string())
    }
}

// ===========================================================================
// Private layer — borderless overlay window above the app window.
// ===========================================================================

use backend_macos::MacosBackend;

/// Install the `PrivateLayer` external handler against a `MacosBackend`.
///
/// The handler asks the backend to build a separate, borderless `NSWindow`
/// (see `MacosBackend::create_private_layer_window`) and returns its content
/// view. The framework's External walker then parents the layer's children
/// (toolbar, recording preview) into that content view; the backend's
/// `insert` / `clear_children` skip reparenting it into the main tree because
/// the content view is registered as a detached window root.
///
/// The overlay is added as a CHILD window above the app window so it tracks
/// the app's moves + Spaces and composites on top, and its passthrough
/// `hitTest:` lets clicks fall through to the app everywhere except over a
/// real control — so the toolbar is interactive while the canvas beneath stays
/// drawable. Mirrors `ios::register`.
///
/// Capture EXCLUSION is wired: the backend records each overlay's
/// `windowNumber` (and pins `NSWindowSharingNone`); [`start`] reads those ids
/// via `backend_macos::private_layer_window_ids`, matches them against
/// `SCShareableContent.windows`, and passes the matching `SCWindow`s to the
/// `SCContentFilter` exclusion list — so the recording omits the overlay.
pub fn register(backend: &mut MacosBackend) {
    backend.register_external::<crate::PrivateLayerProps, _>(|_props, b| {
        b.create_private_layer_window()
    });
}

// Self-register at backend construction (no app-side `register` call needed).
// See [[project_inventory_self_registration]].
inventory::submit! {
    backend_macos::MacosExternalRegistrar(register)
}
