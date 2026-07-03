//! iOS / macOS recording via `AVAssetWriter` (H.264 video + AAC audio),
//! driven through the Obj-C runtime — the same posture `camera`'s
//! `AVCaptureSession` backend takes. We link CoreMedia / CoreVideo /
//! AudioToolbox for the handful of C calls that build a `CVPixelBuffer` from
//! RGBA bytes and a `CMSampleBuffer` from interleaved-`f32` PCM.
//!
//! # Why a dedicated encoder thread
//!
//! AVFoundation objects (`AVAssetWriter`, its inputs, the pixel-buffer
//! adaptor) are not `Send` in objc2's type system — and our two capture taps
//! arrive on *different* threads (the camera's dispatch queue, cpal's audio
//! thread). Rather than `unsafe impl Send` on Obj-C handles (a claim we'd have
//! to defend across two concurrent producers), we own every Obj-C object on a
//! single owned `std::thread`. The capture callbacks copy each frame/chunk
//! into a plain `Vec` (which IS `Send`) and push it down an `mpsc` channel;
//! the encoder thread is the *only* toucher of the writer. The safety story is
//! then "Obj-C objects never cross a thread boundary," which needs no unsafe
//! assertion.
//!
//! # Why the writer starts lazily
//!
//! `AVAssetWriter` needs every input's format *before* `startWriting`: video
//! width/height, audio sample-rate/channels. We don't know those until the
//! first sample of each expected kind arrives. So the encoder buffers the
//! earliest samples, and once it has seen the first of every expected kind it
//! builds the writer, `startSession`s at the earliest capture timestamp, and
//! flushes the buffer in timestamp order. Steady-state samples append直接.
//!
//! Verified on macOS by `tests/host_record.rs`, which feeds synthetic
//! `MediaStream` + `AudioStream` producers (no real devices) and asserts a
//! non-trivial, `AVAsset`-readable `.mp4` lands on disk.

use std::ffi::c_void;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;

/// Max video frames allowed in-flight (queued but not yet dequeued by the
/// encoder) before the producer drops new ones. A live render thread
/// (`write_rgba8` on every raf frame) outruns the H.264 encoder, so an
/// unbounded channel backs up GBs of full-res RGBA — and `stop()` then spends
/// SECONDS draining that backlog before it can finalize (measured ~1.9s for a
/// 5s 2048×1536 recording: the whiteboard "stop is super delayed" bug). Real-
/// time recording policy: drop at the source when the encoder is behind so the
/// queue stays tiny and finalize is prompt. Small (a few frames) bounds memory
/// AND latency; the encoder's own `isReadyForMoreMediaData` does the final drop.
const MAX_INFLIGHT_VIDEO_FRAMES: usize = 3;

use block2::RcBlock;
use objc2::encode::{Encode, Encoding, RefEncode};
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, Bool};
use objc2::{class, msg_send, msg_send_id};
use objc2_foundation::NSString;

use crate::{MediaInputs, MediaWriterError, RecordConfig};
use media_stream::{AudioSubscription, Subscription};

// ---------------------------------------------------------------------------
// Foreign surfaces.
// ---------------------------------------------------------------------------

#[link(name = "AVFoundation", kind = "framework")]
extern "C" {
    // AVFoundation dictionary keys / enum-value constants are `NSString * const`
    // with non-obvious string values, so we link the real symbols rather than
    // guess. Each is a pointer-sized slot holding the NSString pointer.
    static AVVideoCodecKey: *const AnyObject;
    static AVVideoWidthKey: *const AnyObject;
    static AVVideoHeightKey: *const AnyObject;
    static AVVideoCodecTypeH264: *const AnyObject;
    static AVVideoCompressionPropertiesKey: *const AnyObject;
    static AVVideoAverageBitRateKey: *const AnyObject;
    static AVVideoExpectedSourceFrameRateKey: *const AnyObject;
    static AVFormatIDKey: *const AnyObject;
    static AVSampleRateKey: *const AnyObject;
    static AVNumberOfChannelsKey: *const AnyObject;
    static AVEncoderBitRateKey: *const AnyObject;
    static AVFileTypeMPEG4: *const AnyObject;
}

#[allow(non_upper_case_globals)]
#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    fn CVPixelBufferCreate(
        allocator: *const c_void,
        width: usize,
        height: usize,
        pixel_format: u32,
        attrs: *const c_void,
        out: *mut *mut c_void,
    ) -> i32;
    fn CVPixelBufferLockBaseAddress(pb: *mut c_void, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(pb: *mut c_void, flags: u64) -> i32;
    fn CVPixelBufferGetBaseAddress(pb: *mut c_void) -> *mut c_void;
    fn CVPixelBufferGetBytesPerRow(pb: *mut c_void) -> usize;
    /// Zero-copy: wrap an existing `IOSurface` in a `CVPixelBuffer` that the
    /// encoder appends directly — no allocation, no pixel copy. The pixel format
    /// + dimensions come from the surface itself.
    fn CVPixelBufferCreateWithIOSurface(
        allocator: *const c_void,
        surface: *const c_void,
        attrs: *const c_void,
        out: *mut *mut c_void,
    ) -> i32;
}

#[link(name = "IOSurface", kind = "framework")]
extern "C" {
    fn IOSurfaceGetWidth(surface: *const c_void) -> usize;
    fn IOSurfaceGetHeight(surface: *const c_void) -> usize;
}

#[link(name = "CoreMedia", kind = "framework")]
extern "C" {
    fn CMTimeMake(value: i64, timescale: i32) -> CMTime;
    fn CMAudioFormatDescriptionCreate(
        allocator: *const c_void,
        asbd: *const Asbd,
        layout_size: usize,
        layout: *const c_void,
        magic_cookie_size: usize,
        magic_cookie: *const c_void,
        extensions: *const c_void,
        out: *mut *mut c_void,
    ) -> i32;
    fn CMBlockBufferCreateWithMemoryBlock(
        allocator: *const c_void,
        memory_block: *mut c_void,
        block_length: usize,
        block_allocator: *const c_void,
        custom_source: *const c_void,
        offset_to_data: usize,
        data_length: usize,
        flags: u32,
        out: *mut *mut c_void,
    ) -> i32;
    fn CMBlockBufferReplaceDataBytes(
        source_bytes: *const c_void,
        dest: *mut c_void,
        offset_into_dest: usize,
        data_length: usize,
    ) -> i32;
    #[allow(clippy::too_many_arguments)]
    fn CMSampleBufferCreate(
        allocator: *const c_void,
        data_buffer: *mut c_void,
        data_ready: u8,
        make_data_ready_callback: *const c_void,
        make_data_ready_refcon: *mut c_void,
        format_description: *mut c_void,
        num_samples: isize,
        num_timing_entries: isize,
        timing_array: *const CMSampleTimingInfo,
        num_size_entries: isize,
        size_array: *const usize,
        out: *mut *mut c_void,
    ) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(cf: *const c_void);
}

/// `CMTime` — by-value across `startSessionAtSourceTime:` and the append
/// selectors, so it needs [`Encode`]. Field layout matches Apple's exactly.
#[repr(C)]
#[derive(Clone, Copy)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

// SAFETY: matches Apple's `CMTime` (int64 value, int32 timescale, uint32
// flags, int64 epoch) so objc2 builds the correct by-value method type.
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

/// Opaque `CVPixelBufferRef` pointee. Exists so a `*mut CVBuffer` argument to
/// `appendPixelBuffer:` encodes as `^{__CVBuffer=}` — objc2's debug encoding
/// check rejects a bare `*mut c_void` (`^v`).
struct CVBuffer {
    _priv: [u8; 0],
}
// SAFETY: the encoding matches the `CVPixelBufferRef` argument type AVFoundation
// declares for `appendPixelBuffer:withPresentationTime:`.
unsafe impl RefEncode for CVBuffer {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("__CVBuffer", &[]));
}

/// Opaque `CMSampleBufferRef` pointee — encodes a `*mut CMSampleBuffer`
/// argument as `^{opaqueCMSampleBuffer=}` for `appendSampleBuffer:`.
struct CMSampleBuffer {
    _priv: [u8; 0],
}
// SAFETY: matches the `CMSampleBufferRef` argument type AVFoundation declares
// for `appendSampleBuffer:`.
unsafe impl RefEncode for CMSampleBuffer {
    const ENCODING_REF: Encoding =
        Encoding::Pointer(&Encoding::Struct("opaqueCMSampleBuffer", &[]));
}

/// `CMSampleTimingInfo` — only crosses the C ABI by pointer, so no `Encode`.
#[repr(C)]
#[derive(Clone, Copy)]
struct CMSampleTimingInfo {
    duration: CMTime,
    presentation: CMTime,
    decode: CMTime,
}

/// `AudioStreamBasicDescription`. Crosses the ABI by pointer only.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Asbd {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels_per_frame: u32,
    bits_per_channel: u32,
    reserved: u32,
}

// Pixel format / flag constants.
const PIXEL_FORMAT_32BGRA: u32 = 0x4247_5241; // 'BGRA'
const LOCK_FLAGS: u64 = 0;
const AUDIO_FORMAT_LPCM: u32 = 0x6c70_636d; // 'lpcm'
const AUDIO_FORMAT_AAC: u32 = 0x6161_6320; // 'aac '
// kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked.
const LPCM_FLAGS_FLOAT_PACKED: u32 = 1 | (1 << 3);
// kCMBlockBufferAssureMemoryNowFlag.
const BLOCK_BUFFER_ASSURE_MEMORY: u32 = 1 << 0;
// AVAssetWriterStatus.
const STATUS_COMPLETED: isize = 2;
const STATUS_FAILED: isize = 3;
// Microsecond timebase — matches `media_stream::clock` and `pts_micros`.
const TIMESCALE_US: i32 = 1_000_000;

const AV_MEDIA_TYPE_VIDEO: &str = "vide";
const AV_MEDIA_TYPE_AUDIO: &str = "soun";

// ---------------------------------------------------------------------------
// Cross-thread messages: plain `Send` data, never an Obj-C handle.
// ---------------------------------------------------------------------------

enum Msg {
    Video {
        width: u32,
        height: u32,
        pts_us: u64,
        rgba: Vec<u8>,
    },
    Audio {
        sample_rate: u32,
        channels: u16,
        pts_us: u64,
        samples: Vec<f32>,
    },
    /// Finalize the file and exit; the encoder replies through a thread-safe,
    /// awaitable oneshot (NOT a blocking channel — see `stop` below).
    Stop(crate::oneshot::SyncTx<Result<(), MediaWriterError>>),
}

struct WorkerParams {
    path: PathBuf,
    has_video: bool,
    has_audio: bool,
    fps: u32,
    video_bitrate: Option<u32>,
    audio_bitrate: Option<u32>,
}

// ---------------------------------------------------------------------------
// Handle returned to the public API. Owns the capture subscriptions (dropping
// them stops the taps) and the channel to the encoder thread.
// ---------------------------------------------------------------------------

pub(crate) struct RecordingHandle {
    tx: Sender<Msg>,
    join: Option<JoinHandle<()>>,
    _video_sub: Option<Subscription>,
    _audio_sub: Option<AudioSubscription>,
    /// Held while recording from a zero-copy native source (the encoder thread
    /// polls the surface). Dropping it lets the GPU producer stop publishing.
    #[cfg(target_os = "macos")]
    _native_tap: Option<media_stream::NativeTap>,
}

impl RecordingHandle {
    pub(crate) async fn stop(mut self) -> Result<(), MediaWriterError> {
        // Stop the taps first so no further samples enqueue, then ask the
        // encoder to finalize and report.
        self._video_sub = None;
        self._audio_sub = None;
        // Release the native tap too, so a GPU producer stops the per-frame
        // IOSurface blit + publish immediately (the encoder polls the surface).
        #[cfg(target_os = "macos")]
        {
            self._native_tap = None;
        }

        // AWAIT the finalize result — never block on it. `stop` runs on the
        // single-threaded main executor; a synchronous `recv()` here freezes the
        // run loop, which starves AVFoundation's `finishWritingWithCompletion-
        // Handler:` completion (it needs the run loop pumping) → the encoder
        // hangs on its 15s timeout while the whole UI is frozen. Awaiting the
        // parking `SyncRx` yields to the run loop so the completion fires and the
        // encoder finalizes promptly. See `oneshot::sync_oneshot` +
        // `sync_cross_thread_send_delivers_payload` for the regression test of
        // this park-don't-block contract.
        let (done_tx, done_rx) = crate::oneshot::sync_oneshot();
        self.tx
            .send(Msg::Stop(done_tx))
            .map_err(|_| MediaWriterError::Backend("encoder thread gone".into()))?;
        let result = done_rx
            .await
            .ok_or_else(|| MediaWriterError::Backend(
                "encoder thread dropped before finishing".into(),
            ))?;
        // The encoder `return`s immediately after replying, so this join is
        // effectively instantaneous (it does NOT block the run loop on finalize).
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        result
    }
}

impl Drop for RecordingHandle {
    fn drop(&mut self) {
        // Dropped without `stop()`: closing the channel makes the encoder
        // thread abort (cancelWriting, discard the partial file).
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

// ---------------------------------------------------------------------------
// start(): resolve the output path, spawn the encoder, wire the taps.
// ---------------------------------------------------------------------------

pub(crate) async fn start(
    inputs: MediaInputs<'_>,
    config: &RecordConfig,
) -> Result<RecordingHandle, MediaWriterError> {
    let path = config
        .store
        .local_path(&config.path)
        .ok_or(MediaWriterError::NoLocalPath)?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // AVAssetWriter refuses to initialize if the file already exists.
    let _ = std::fs::remove_file(&path);

    let params = WorkerParams {
        path,
        has_video: inputs.video.is_some(),
        has_audio: inputs.audio.is_some(),
        fps: config.fps.max(1),
        video_bitrate: config.video_bitrate,
        audio_bitrate: config.audio_bitrate,
    };

    // Zero-copy native video source: a GPU canvas publishes rendered IOSurfaces
    // as the stream's `native_source`. When present we record those directly
    // (the encoder thread polls the surface) and SKIP the CPU video tap, so the
    // recording never touches a CPU frame. macOS only (IOSurface); other apple
    // targets fall through to the CPU path.
    #[cfg(target_os = "macos")]
    let (native_source, native_tap): (Option<media_stream::SurfaceSource>, _) = match inputs
        .video
        .and_then(|s| s.native_source())
        .and_then(|ns| ns.downcast::<media_stream::SurfaceSource>().ok())
    {
        Some(src) => {
            let tap = src.register_tap();
            (Some((*src).clone()), Some(tap))
        }
        None => (None, None),
    };
    #[cfg(not(target_os = "macos"))]
    let native_source: Option<media_stream::SurfaceSource> = None;
    let is_native = native_source.is_some();

    let (tx, rx) = std::sync::mpsc::channel::<Msg>();
    // Bounds the video backlog (see `MAX_INFLIGHT_VIDEO_FRAMES`). Incremented by
    // the producer on enqueue, decremented by the encoder on dequeue.
    let inflight = Arc::new(AtomicUsize::new(0));
    let join = {
        let inflight = inflight.clone();
        std::thread::Builder::new()
            .name("media-writer".into())
            .spawn(move || encoder_thread(rx, params, inflight, native_source))
            .map_err(|e| MediaWriterError::Backend(format!("spawn encoder thread: {e}")))?
    };

    let video_sub = if is_native {
        None
    } else {
        inputs.video.map(|stream| {
        let tx = tx.clone();
        let inflight = inflight.clone();
        stream.subscribe(move |f| {
            // Real-time drop: if the encoder is already `MAX_INFLIGHT_VIDEO_FRAMES`
            // behind, skip this frame rather than clone+queue it (which would
            // grow the backlog `stop()` must later drain). Lowers effective fps
            // under load instead of unbounded memory + finalize latency.
            if inflight.load(Ordering::Acquire) >= MAX_INFLIGHT_VIDEO_FRAMES {
                return;
            }
            inflight.fetch_add(1, Ordering::AcqRel);
            if tx
                .send(Msg::Video {
                    width: f.width,
                    height: f.height,
                    pts_us: f.pts_micros,
                    rgba: f.data.to_vec(),
                })
                .is_err()
            {
                // Encoder gone: undo the reservation we won't see dequeued.
                inflight.fetch_sub(1, Ordering::AcqRel);
            }
        })
        })
    };
    let audio_sub = inputs.audio.map(|stream| {
        let tx = tx.clone();
        stream.subscribe(move |f| {
            let _ = tx.send(Msg::Audio {
                sample_rate: f.sample_rate,
                channels: f.channels,
                pts_us: f.pts_micros,
                samples: f.samples.to_vec(),
            });
        })
    });

    Ok(RecordingHandle {
        tx,
        join: Some(join),
        _video_sub: video_sub,
        _audio_sub: audio_sub,
        #[cfg(target_os = "macos")]
        _native_tap: native_tap,
    })
}

// ---------------------------------------------------------------------------
// Encoder thread — the sole owner of every Obj-C handle.
// ---------------------------------------------------------------------------

fn encoder_thread(
    rx: Receiver<Msg>,
    params: WorkerParams,
    inflight: Arc<AtomicUsize>,
    native: Option<media_stream::SurfaceSource>,
) {
    use std::sync::mpsc::RecvTimeoutError;
    let mut enc = Encoder::new(params);
    // Last native generation appended — only macOS polls native surfaces.
    #[cfg_attr(not(target_os = "macos"), allow(unused_variables, unused_mut))]
    let mut last_native_gen = u64::MAX;
    loop {
        // With a native source, poll it on a short timeout between messages
        // (the producer publishes the latest surface; we pull it). Without one,
        // block on `recv` so an idle CPU-only recording wastes nothing.
        let msg = if native.is_some() {
            rx.recv_timeout(std::time::Duration::from_millis(8))
        } else {
            rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
        };
        match msg {
            Ok(Msg::Video {
                width,
                height,
                pts_us,
                rgba,
            }) => {
                // Release the in-flight reservation as soon as we own the frame,
                // so the producer can enqueue the next one while we encode this.
                inflight.fetch_sub(1, Ordering::AcqRel);
                guard(&mut enc, |e| e.on_video(width, height, pts_us, rgba));
            }
            Ok(Msg::Audio {
                sample_rate,
                channels,
                pts_us,
                samples,
            }) => guard(&mut enc, |e| e.on_audio(sample_rate, channels, pts_us, samples)),
            Ok(Msg::Stop(reply)) => {
                let result = match run_guarded(|| enc.finish()) {
                    Ok(r) => r,
                    Err(msg) => Err(MediaWriterError::Backend(msg)),
                };
                let _ = reply.send(result);
                return;
            }
            // Timeout: pull the latest native surface, if a new one was published.
            Err(RecvTimeoutError::Timeout) => {
                #[cfg(target_os = "macos")]
                if let Some(src) = &native {
                    let gen = src.generation();
                    if gen != last_native_gen {
                        last_native_gen = gen;
                        let ptr = src.acquire();
                        if !ptr.is_null() {
                            let pts = media_stream::clock::now_micros();
                            guard(&mut enc, |e| e.on_native(ptr, pts));
                            // Balance the `acquire` retain (the pixel buffer took
                            // its own during the append).
                            unsafe { src.release(ptr) };
                        }
                    }
                }
            }
            // Channel closed without a Stop (handle dropped): abort.
            Err(RecvTimeoutError::Disconnected) => {
                let _ = run_guarded(|| enc.abort());
                return;
            }
        }
    }
}

/// Run an encoder step, recording any failure. The closure runs the
/// AVFoundation calls; this traps BOTH a Rust panic (e.g. a nil-return unwrap)
/// and an Obj-C `NSException`, turning either into a recorded failure. Without
/// this either one would cross the `objc2::exception::catch` C-ABI boundary and
/// `abort()` the process (a panic that "cannot unwind").
fn guard(enc: &mut Encoder, f: impl FnOnce(&mut Encoder)) {
    if let Err(msg) = run_guarded(|| f(enc)) {
        enc.failed.get_or_insert(msg);
    }
}

/// Run `f`, returning `Err(message)` for either a Rust panic or an Obj-C
/// exception. The Rust `catch_unwind` is nested INSIDE the Obj-C `catch` so a
/// Rust panic is absorbed before it can reach the non-unwinding C trampoline.
fn run_guarded<R>(f: impl FnOnce() -> R) -> Result<R, String> {
    // SAFETY: the Obj-C `catch` only traps an exception raised by AVFoundation;
    // the inner `catch_unwind` ensures no Rust panic propagates into its C frame.
    let outcome = unsafe {
        objc2::exception::catch(AssertUnwindSafe(|| {
            std::panic::catch_unwind(AssertUnwindSafe(f))
        }))
    };
    match outcome {
        Ok(Ok(r)) => Ok(r),
        Ok(Err(p)) => Err(panic_message(p)),
        Err(exc) => Err(exception_message(exc)),
    }
}

/// Best-effort message from a caught Rust panic payload.
fn panic_message(p: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        format!("internal panic: {s}")
    } else if let Some(s) = p.downcast_ref::<String>() {
        format!("internal panic: {s}")
    } else {
        "internal panic (non-string payload)".into()
    }
}

/// Best-effort human message from a caught `NSException`.
fn exception_message(exc: Option<Retained<objc2::exception::Exception>>) -> String {
    match exc {
        Some(e) => format!("Obj-C exception: {e:?}"),
        None => "Obj-C exception (no info)".into(),
    }
}

/// A buffered pre-start sample.
enum Pending {
    Video {
        width: u32,
        height: u32,
        pts_us: u64,
        rgba: Vec<u8>,
    },
    Audio {
        sample_rate: u32,
        channels: u16,
        pts_us: u64,
        samples: Vec<f32>,
    },
}

struct Encoder {
    params: WorkerParams,
    started: bool,
    failed: Option<String>,
    writer: Option<Retained<AnyObject>>,
    video_input: Option<Retained<AnyObject>>,
    video_adaptor: Option<Retained<AnyObject>>,
    audio_input: Option<Retained<AnyObject>>,
    audio_format_desc: Option<*mut c_void>,
    first_video: Option<(u32, u32)>,
    first_audio: Option<(u32, u16)>,
    pending: Vec<Pending>,
}

impl Encoder {
    fn new(params: WorkerParams) -> Self {
        Self {
            params,
            started: false,
            failed: None,
            writer: None,
            video_input: None,
            video_adaptor: None,
            audio_input: None,
            audio_format_desc: None,
            first_video: None,
            first_audio: None,
            pending: Vec::new(),
        }
    }

    fn on_video(&mut self, width: u32, height: u32, pts_us: u64, rgba: Vec<u8>) {
        if self.failed.is_some() {
            return;
        }
        if self.started {
            self.append_video(width, height, pts_us, &rgba);
        } else {
            self.first_video.get_or_insert((width, height));
            self.pending.push(Pending::Video {
                width,
                height,
                pts_us,
                rgba,
            });
            self.maybe_start();
        }
    }

    fn on_audio(&mut self, sample_rate: u32, channels: u16, pts_us: u64, samples: Vec<f32>) {
        if self.failed.is_some() {
            return;
        }
        if self.started {
            self.append_audio(sample_rate, channels, pts_us, &samples);
        } else {
            self.first_audio.get_or_insert((sample_rate, channels));
            self.pending.push(Pending::Audio {
                sample_rate,
                channels,
                pts_us,
                samples,
            });
            self.maybe_start();
        }
    }

    /// Start the writer once every expected input's format is known, then
    /// flush buffered samples in timestamp order.
    fn maybe_start(&mut self) {
        let ready = (!self.params.has_video || self.first_video.is_some())
            && (!self.params.has_audio || self.first_audio.is_some());
        if !ready {
            // Bound pre-start buffering so a never-arriving second source
            // can't grow `pending` without limit (≈ a few seconds at 30fps).
            const MAX_PENDING: usize = 256;
            if self.pending.len() > MAX_PENDING {
                self.failed = Some("one input never produced a sample".into());
                self.pending.clear();
            }
            return;
        }
        let session_start_us = self
            .pending
            .iter()
            .map(|p| match p {
                Pending::Video { pts_us, .. } | Pending::Audio { pts_us, .. } => *pts_us,
            })
            .min()
            .unwrap_or(0);
        if let Err(e) = self.build_writer(session_start_us) {
            self.failed = Some(e);
            self.pending.clear();
            return;
        }
        self.started = true;

        let mut pending = std::mem::take(&mut self.pending);
        pending.sort_by_key(|p| match p {
            Pending::Video { pts_us, .. } | Pending::Audio { pts_us, .. } => *pts_us,
        });
        for p in pending {
            match p {
                Pending::Video {
                    width,
                    height,
                    pts_us,
                    rgba,
                } => self.append_video(width, height, pts_us, &rgba),
                Pending::Audio {
                    sample_rate,
                    channels,
                    pts_us,
                    samples,
                } => self.append_audio(sample_rate, channels, pts_us, &samples),
            }
        }
    }

    /// Build the `AVAssetWriter`, its inputs, `startWriting`, and start the
    /// session at the earliest buffered timestamp.
    fn build_writer(&mut self, session_start_us: u64) -> Result<(), String> {
        unsafe {
            let path_str = NSString::from_str(&self.params.path.to_string_lossy());
            let url: Retained<AnyObject> =
                msg_send_id![class!(NSURL), fileURLWithPath: &*path_str];
            let file_type: &AnyObject = &*AVFileTypeMPEG4;

            let alloc: Allocated<AnyObject> = msg_send_id![class!(AVAssetWriter), alloc];
            let mut err: *mut AnyObject = ptr::null_mut();
            let writer: Option<Retained<AnyObject>> = msg_send_id![
                alloc,
                initWithURL: &*url,
                fileType: file_type,
                error: &mut err,
            ];
            let writer = writer.ok_or_else(|| "AVAssetWriter initWithURL failed".to_string())?;

            if self.params.has_video {
                let (w, h) = self.first_video.unwrap();
                let settings = video_settings(w, h, self.params.fps, self.params.video_bitrate);
                let media_type = NSString::from_str(AV_MEDIA_TYPE_VIDEO);
                let input: Retained<AnyObject> = msg_send_id![
                    class!(AVAssetWriterInput),
                    assetWriterInputWithMediaType: &*media_type,
                    outputSettings: &*settings,
                ];
                let _: () = msg_send![&*input, setExpectsMediaDataInRealTime: Bool::YES];
                let can: Bool = msg_send![&*writer, canAddInput: &*input];
                if !can.as_bool() {
                    return Err("writer cannot add video input".into());
                }
                let _: () = msg_send![&*writer, addInput: &*input];

                // The pixel-buffer adaptor MUST be created before startWriting —
                // creating it afterward raises NSInternalInconsistencyException
                // on first append. (Invariant: adaptor-before-startWriting.)
                let adaptor: Retained<AnyObject> = msg_send_id![
                    class!(AVAssetWriterInputPixelBufferAdaptor),
                    assetWriterInputPixelBufferAdaptorWithAssetWriterInput: &*input,
                    sourcePixelBufferAttributes: ptr::null::<AnyObject>(),
                ];
                self.video_adaptor = Some(adaptor);
                self.video_input = Some(input);
            }

            if self.params.has_audio {
                let (rate, channels) = self.first_audio.unwrap();
                let settings = audio_settings(rate, channels, self.params.audio_bitrate);
                let media_type = NSString::from_str(AV_MEDIA_TYPE_AUDIO);
                let input: Retained<AnyObject> = msg_send_id![
                    class!(AVAssetWriterInput),
                    assetWriterInputWithMediaType: &*media_type,
                    outputSettings: &*settings,
                ];
                let _: () = msg_send![&*input, setExpectsMediaDataInRealTime: Bool::YES];
                let can: Bool = msg_send![&*writer, canAddInput: &*input];
                if !can.as_bool() {
                    return Err("writer cannot add audio input".into());
                }
                let _: () = msg_send![&*writer, addInput: &*input];
                self.audio_input = Some(input);
            }

            let ok: Bool = msg_send![&*writer, startWriting];
            if !ok.as_bool() {
                let status: isize = msg_send![&*writer, status];
                return Err(format!("startWriting failed (status {status})"));
            }
            let start_time = CMTimeMake(session_start_us as i64, TIMESCALE_US);
            let _: () = msg_send![&*writer, startSessionAtSourceTime: start_time];

            self.writer = Some(writer);
        }
        Ok(())
    }

    fn append_video(&mut self, width: u32, height: u32, pts_us: u64, rgba: &[u8]) {
        let Some(input) = self.video_input.as_ref() else {
            return;
        };
        let need = width as usize * height as usize * 4;
        if rgba.len() < need {
            return;
        }
        unsafe {
            let ready: Bool = msg_send![&**input, isReadyForMoreMediaData];
            if !ready.as_bool() {
                // Real-time policy: drop a frame rather than stall capture.
                return;
            }

            let mut pb: *mut c_void = ptr::null_mut();
            let rc = CVPixelBufferCreate(
                ptr::null(),
                width as usize,
                height as usize,
                PIXEL_FORMAT_32BGRA,
                ptr::null(),
                &mut pb,
            );
            if rc != 0 || pb.is_null() {
                self.failed = Some(format!("CVPixelBufferCreate failed ({rc})"));
                return;
            }
            CVPixelBufferLockBaseAddress(pb, LOCK_FLAGS);
            let base = CVPixelBufferGetBaseAddress(pb) as *mut u8;
            let stride = CVPixelBufferGetBytesPerRow(pb);
            if !base.is_null() && stride >= width as usize * 4 {
                fill_bgra(base, stride, width as usize, height as usize, rgba);
            }
            CVPixelBufferUnlockBaseAddress(pb, LOCK_FLAGS);

            let time = CMTimeMake(pts_us as i64, TIMESCALE_US);
            let appended = self.append_pixel_buffer(pb, time);
            CFRelease(pb);
            if let Err(e) = appended {
                self.failed = Some(e);
            }
        }
    }

    /// Append a `CVPixelBuffer` at `time` through the input's pixel-buffer
    /// adaptor (created in `build_writer` before `startWriting`), which wraps it
    /// in a `CMSampleBuffer` for us — no manual video sample-buffer construction.
    unsafe fn append_pixel_buffer(&mut self, pb: *mut c_void, time: CMTime) -> Result<(), String> {
        let Some(adaptor) = self.video_adaptor.clone() else {
            return Err("no pixel-buffer adaptor".into());
        };
        let pb = pb as *mut CVBuffer;
        let ok: Bool = msg_send![
            &*adaptor,
            appendPixelBuffer: pb,
            withPresentationTime: time,
        ];
        if ok.as_bool() {
            Ok(())
        } else {
            Err(self.writer_error_string("appendPixelBuffer"))
        }
    }

    /// Zero-copy native video frame: the producer (a GPU canvas) rendered into
    /// an `IOSurface` and published it; we wrap that surface in a `CVPixelBuffer`
    /// and append it — no allocation, no `fill_bgra` swizzle, no CPU touch.
    ///
    /// Native frames can't be buffered in `pending` like CPU frames (the
    /// producer's IOSurface ring reuses them), so we record the video format
    /// from the surface and route through `maybe_start` — the SAME gate the CPU
    /// path uses. When audio is also expected, the writer is NOT built until the
    /// first audio chunk has set its format (an `AVAssetWriter` input can't be
    /// added after `startWriting`); until then this frame is simply dropped and a
    /// later poll starts the writer. Building eagerly here was the
    /// "recording stop failed: unwrap() on a None value" panic — `build_writer`
    /// unwraps `first_audio`, which is absent on the first video frame (only
    /// reachable once audio recording was added; video-only never hit it).
    #[cfg(target_os = "macos")]
    fn on_native(&mut self, surface: *const c_void, pts_us: u64) {
        if self.failed.is_some() {
            return;
        }
        if !self.started {
            let (w, h) = unsafe {
                (IOSurfaceGetWidth(surface) as u32, IOSurfaceGetHeight(surface) as u32)
            };
            if w == 0 || h == 0 {
                return;
            }
            self.first_video = Some((w, h));
            if self.params.has_audio {
                // AV recording: hold off until the audio format is known, then
                // let `maybe_start` build the writer and flush the buffered audio
                // (its session start derives from those audio chunks; the native
                // video isn't in `pending`). This frame is appended below only if
                // we actually started — otherwise it's dropped while we wait.
                self.maybe_start();
                if !self.started {
                    return;
                }
            } else {
                // Video-only native: no audio to wait for, and `pending` is
                // always empty on this path, so `maybe_start` would mis-derive
                // the session start as 0. Start at THIS frame's timestamp.
                if let Err(e) = self.build_writer(pts_us) {
                    self.failed = Some(e);
                    return;
                }
                self.started = true;
            }
        }
        self.append_native(surface, pts_us);
    }

    #[cfg(target_os = "macos")]
    fn append_native(&mut self, surface: *const c_void, pts_us: u64) {
        let Some(input) = self.video_input.as_ref() else {
            return;
        };
        unsafe {
            let ready: Bool = msg_send![&**input, isReadyForMoreMediaData];
            if !ready.as_bool() {
                // Real-time policy: drop a frame rather than stall.
                return;
            }
            let mut pb: *mut c_void = ptr::null_mut();
            let rc = CVPixelBufferCreateWithIOSurface(ptr::null(), surface, ptr::null(), &mut pb);
            if rc != 0 || pb.is_null() {
                self.failed = Some(format!("CVPixelBufferCreateWithIOSurface failed ({rc})"));
                return;
            }
            let time = CMTimeMake(pts_us as i64, TIMESCALE_US);
            let appended = self.append_pixel_buffer(pb, time);
            CFRelease(pb);
            if let Err(e) = appended {
                self.failed = Some(e);
            }
        }
    }

    fn append_audio(&mut self, sample_rate: u32, channels: u16, pts_us: u64, samples: &[f32]) {
        // Clone (retain) the input so we don't hold an immutable borrow of
        // `self` across `self.audio_format_desc(..)` below.
        let Some(input) = self.audio_input.clone() else {
            return;
        };
        if channels == 0 || sample_rate == 0 || samples.is_empty() {
            return;
        }
        unsafe {
            let ready: Bool = msg_send![&*input, isReadyForMoreMediaData];
            if !ready.as_bool() {
                return;
            }
            let format_desc = match self.audio_format_desc(sample_rate, channels) {
                Ok(d) => d,
                Err(e) => {
                    self.failed = Some(e);
                    return;
                }
            };
            match build_audio_sample_buffer(format_desc, sample_rate, channels, pts_us, samples) {
                Ok(sbuf) => {
                    let sbuf_ref = sbuf as *mut CMSampleBuffer;
                    let ok: Bool = msg_send![&*input, appendSampleBuffer: sbuf_ref];
                    CFRelease(sbuf);
                    if !ok.as_bool() {
                        self.failed = Some(self.writer_error_string("appendSampleBuffer(audio)"));
                    }
                }
                Err(e) => self.failed = Some(e),
            }
        }
    }

    /// The cached LPCM `CMAudioFormatDescription`, built on first audio chunk.
    unsafe fn audio_format_desc(
        &mut self,
        sample_rate: u32,
        channels: u16,
    ) -> Result<*mut c_void, String> {
        if let Some(d) = self.audio_format_desc {
            return Ok(d);
        }
        let bytes_per_frame = 4 * channels as u32;
        let asbd = Asbd {
            sample_rate: sample_rate as f64,
            format_id: AUDIO_FORMAT_LPCM,
            format_flags: LPCM_FLAGS_FLOAT_PACKED,
            bytes_per_packet: bytes_per_frame,
            frames_per_packet: 1,
            bytes_per_frame,
            channels_per_frame: channels as u32,
            bits_per_channel: 32,
            reserved: 0,
        };
        let mut desc: *mut c_void = ptr::null_mut();
        let rc = CMAudioFormatDescriptionCreate(
            ptr::null(),
            &asbd,
            0,
            ptr::null(),
            0,
            ptr::null(),
            ptr::null(),
            &mut desc,
        );
        if rc != 0 || desc.is_null() {
            return Err(format!("CMAudioFormatDescriptionCreate failed ({rc})"));
        }
        self.audio_format_desc = Some(desc);
        Ok(desc)
    }

    /// Read `AVAssetWriter.error.localizedDescription` for a richer message.
    unsafe fn writer_error_string(&self, ctx: &str) -> String {
        if let Some(writer) = self.writer.as_ref() {
            let err: *mut AnyObject = msg_send![&**writer, error];
            if !err.is_null() {
                let desc: Retained<NSString> = msg_send_id![err, localizedDescription];
                return format!("{ctx}: {desc}");
            }
        }
        format!("{ctx} failed")
    }

    fn finish(&mut self) -> Result<(), MediaWriterError> {
        if let Some(e) = self.failed.take() {
            self.cleanup_handles();
            return Err(MediaWriterError::Backend(e));
        }
        let Some(writer) = self.writer.take() else {
            self.cleanup_handles();
            return Err(MediaWriterError::Backend(
                "no media captured before stop".into(),
            ));
        };
        unsafe {
            if let Some(v) = self.video_input.take() {
                let _: () = msg_send![&*v, markAsFinished];
            }
            if let Some(a) = self.audio_input.take() {
                let _: () = msg_send![&*a, markAsFinished];
            }

            // finishWritingWithCompletionHandler: is async; block this thread
            // on a channel the completion block signals.
            let (tx, rx) = std::sync::mpsc::channel::<()>();
            let tx = std::cell::Cell::new(Some(tx));
            let block = RcBlock::new(move || {
                if let Some(tx) = tx.take() {
                    let _ = tx.send(());
                }
            });
            let _: () = msg_send![&*writer, finishWritingWithCompletionHandler: &*block];
            // The completion block fires on an AVFoundation queue; bound the
            // wait so a stuck finalize can never hang the caller's `stop()`.
            if rx
                .recv_timeout(std::time::Duration::from_secs(15))
                .is_err()
            {
                self.release_format_desc();
                return Err(MediaWriterError::Backend(
                    "finishWriting timed out".into(),
                ));
            }

            let status: isize = msg_send![&*writer, status];
            let result = if status == STATUS_COMPLETED {
                Ok(())
            } else if status == STATUS_FAILED {
                Err(MediaWriterError::Backend(
                    self.writer_error_string_for(&writer, "finishWriting"),
                ))
            } else {
                Err(MediaWriterError::Backend(format!(
                    "writer ended in status {status}"
                )))
            };
            self.release_format_desc();
            result
        }
    }

    fn abort(&mut self) {
        unsafe {
            if let Some(writer) = self.writer.take() {
                let _: () = msg_send![&*writer, cancelWriting];
            }
        }
        self.cleanup_handles();
        let _ = std::fs::remove_file(&self.params.path);
    }

    fn cleanup_handles(&mut self) {
        self.video_input = None;
        self.audio_input = None;
        self.video_adaptor = None;
        self.writer = None;
        self.release_format_desc();
    }

    fn release_format_desc(&mut self) {
        if let Some(d) = self.audio_format_desc.take() {
            unsafe { CFRelease(d) };
        }
    }

    unsafe fn writer_error_string_for(&self, writer: &Retained<AnyObject>, ctx: &str) -> String {
        let err: *mut AnyObject = msg_send![&**writer, error];
        if !err.is_null() {
            let desc: Retained<NSString> = msg_send_id![err, localizedDescription];
            return format!("{ctx}: {desc}");
        }
        format!("{ctx} failed")
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // If finish()/abort() already ran these are no-ops; otherwise release.
        self.release_format_desc();
    }
}

// ---------------------------------------------------------------------------
// Free helpers.
// ---------------------------------------------------------------------------

/// Copy a tightly-packed top-down RGBA image into a (possibly strided) BGRA
/// `CVPixelBuffer`, swizzling R/B.
///
/// # Safety
/// `dest` must point at `height * stride` writable bytes, `stride >= width*4`.
unsafe fn fill_bgra(dest: *mut u8, stride: usize, width: usize, height: usize, rgba: &[u8]) {
    let row_bytes = width * 4;
    for y in 0..height {
        let src = &rgba[y * row_bytes..(y + 1) * row_bytes];
        let dst = std::slice::from_raw_parts_mut(dest.add(y * stride), row_bytes);
        for x in 0..width {
            let s = &src[x * 4..x * 4 + 4]; // R G B A
            let d = &mut dst[x * 4..x * 4 + 4]; // B G R A
            d[0] = s[2];
            d[1] = s[1];
            d[2] = s[0];
            d[3] = s[3];
        }
    }
}

/// Build a `CMSampleBuffer` of interleaved-`f32` LPCM. Returns a +1 retained
/// buffer the caller must `CFRelease`.
///
/// # Safety
/// `format_desc` must be a valid LPCM `CMAudioFormatDescription` matching
/// `sample_rate`/`channels`.
unsafe fn build_audio_sample_buffer(
    format_desc: *mut c_void,
    sample_rate: u32,
    channels: u16,
    pts_us: u64,
    samples: &[f32],
) -> Result<*mut c_void, String> {
    let bytes_per_frame = 4 * channels as usize;
    let frame_count = samples.len() / channels.max(1) as usize;
    if frame_count == 0 {
        return Err("empty audio chunk".into());
    }
    let data_len = frame_count * bytes_per_frame;

    // Allocate a CMBlockBuffer with backing memory, then copy the PCM in.
    let mut block: *mut c_void = ptr::null_mut();
    let rc = CMBlockBufferCreateWithMemoryBlock(
        ptr::null(),
        ptr::null_mut(),
        data_len,
        ptr::null(),
        ptr::null(),
        0,
        data_len,
        BLOCK_BUFFER_ASSURE_MEMORY,
        &mut block,
    );
    if rc != 0 || block.is_null() {
        return Err(format!("CMBlockBufferCreateWithMemoryBlock failed ({rc})"));
    }
    let rc = CMBlockBufferReplaceDataBytes(
        samples.as_ptr() as *const c_void,
        block,
        0,
        data_len,
    );
    if rc != 0 {
        CFRelease(block);
        return Err(format!("CMBlockBufferReplaceDataBytes failed ({rc})"));
    }

    let timing = CMSampleTimingInfo {
        duration: CMTimeMake(1, sample_rate as i32),
        presentation: CMTimeMake(pts_us as i64, TIMESCALE_US),
        decode: CMTime {
            value: 0,
            timescale: 0,
            flags: 0,
            epoch: 0,
        },
    };
    let size = bytes_per_frame;

    let mut sbuf: *mut c_void = ptr::null_mut();
    let rc = CMSampleBufferCreate(
        ptr::null(),
        block,
        1, // dataReady
        ptr::null(),
        ptr::null_mut(),
        format_desc,
        frame_count as isize,
        1,
        &timing,
        1,
        &size,
        &mut sbuf,
    );
    // CMSampleBufferCreate retains the block buffer; release our reference.
    CFRelease(block);
    if rc != 0 || sbuf.is_null() {
        return Err(format!("CMSampleBufferCreate failed ({rc})"));
    }
    Ok(sbuf)
}

/// Build the H.264 video `outputSettings` dictionary.
unsafe fn video_settings(
    width: u32,
    height: u32,
    fps: u32,
    bitrate: Option<u32>,
) -> Retained<AnyObject> {
    let dict: Retained<AnyObject> = msg_send_id![class!(NSMutableDictionary), dictionary];
    let codec: &AnyObject = &*AVVideoCodecTypeH264;
    set_obj(&dict, &*AVVideoCodecKey, codec);
    let w: Retained<AnyObject> = msg_send_id![class!(NSNumber), numberWithInt: width as i32];
    let h: Retained<AnyObject> = msg_send_id![class!(NSNumber), numberWithInt: height as i32];
    set_obj(&dict, &*AVVideoWidthKey, &w);
    set_obj(&dict, &*AVVideoHeightKey, &h);

    // Compression properties: hint the source frame rate (keyframe tuning) and
    // an optional average bitrate. The real cadence still follows each frame's
    // capture timestamp.
    let props: Retained<AnyObject> = msg_send_id![class!(NSMutableDictionary), dictionary];
    let rate: Retained<AnyObject> = msg_send_id![class!(NSNumber), numberWithInt: fps as i32];
    set_obj(&props, &*AVVideoExpectedSourceFrameRateKey, &rate);
    if let Some(bps) = bitrate {
        let n: Retained<AnyObject> = msg_send_id![class!(NSNumber), numberWithInt: bps as i32];
        set_obj(&props, &*AVVideoAverageBitRateKey, &n);
    }
    set_obj(&dict, &*AVVideoCompressionPropertiesKey, &props);
    dict
}

/// Build the AAC audio `outputSettings` dictionary.
unsafe fn audio_settings(sample_rate: u32, channels: u16, bitrate: Option<u32>) -> Retained<AnyObject> {
    let dict: Retained<AnyObject> = msg_send_id![class!(NSMutableDictionary), dictionary];
    let fmt: Retained<AnyObject> =
        msg_send_id![class!(NSNumber), numberWithUnsignedInt: AUDIO_FORMAT_AAC];
    set_obj(&dict, &*AVFormatIDKey, &fmt);
    let rate: Retained<AnyObject> =
        msg_send_id![class!(NSNumber), numberWithDouble: sample_rate as f64];
    set_obj(&dict, &*AVSampleRateKey, &rate);
    let ch: Retained<AnyObject> =
        msg_send_id![class!(NSNumber), numberWithUnsignedInt: channels as u32];
    set_obj(&dict, &*AVNumberOfChannelsKey, &ch);
    if let Some(bps) = bitrate {
        let n: Retained<AnyObject> = msg_send_id![class!(NSNumber), numberWithInt: bps as i32];
        set_obj(&dict, &*AVEncoderBitRateKey, &n);
    }
    dict
}

/// `dict[key] = value` for an `NSMutableDictionary`.
unsafe fn set_obj(dict: &Retained<AnyObject>, key: *const AnyObject, value: &AnyObject) {
    let key: &AnyObject = &*key;
    let _: () = msg_send![&**dict, setObject: value, forKey: key];
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

// macOS-only: the native (zero-copy IOSurface) video path only compiles on
// macOS, and the test creates a real `IOSurface` to drive it. iOS uses the CPU
// video tap, so this path doesn't exist there.
#[cfg(all(test, target_os = "macos"))]
mod native_av_tests {
    use super::*;

    #[link(name = "IOSurface", kind = "framework")]
    extern "C" {
        fn IOSurfaceCreate(properties: *const c_void) -> *mut c_void;
    }

    /// A small BGRA `IOSurface` for the test. Uses the documented string values
    /// of the `kIOSurface*` property keys so we needn't link the constants.
    /// Returns a +1 reference the caller `CFRelease`s.
    fn make_test_surface(w: i64, h: i64) -> *const c_void {
        unsafe {
            let dict: Retained<AnyObject> = msg_send_id![class!(NSMutableDictionary), dictionary];
            let put = |key: &str, val: i64| {
                let k = NSString::from_str(key);
                let n: Retained<AnyObject> =
                    msg_send_id![class!(NSNumber), numberWithLongLong: val];
                let _: () = msg_send![&*dict, setObject: &*n, forKey: &*k];
            };
            put("IOSurfaceWidth", w);
            put("IOSurfaceHeight", h);
            put("IOSurfaceBytesPerElement", 4);
            put("IOSurfacePixelFormat", 0x4247_5241); // 'BGRA'
            let props = (&*dict as *const AnyObject) as *const c_void;
            let surf = IOSurfaceCreate(props);
            assert!(!surf.is_null(), "IOSurfaceCreate returned null");
            surf
        }
    }

    // Regression: the zero-copy native (IOSurface) video path must NOT build the
    // `AVAssetWriter` on the first video frame when audio is ALSO expected —
    // `build_writer` unwraps `first_audio`, which is still `None` until the first
    // audio chunk arrives, panicking with "called `Option::unwrap()` on a `None`
    // value". This surfaced in the whiteboard the moment audio recording was
    // added (the recording failed at STOP); video-only never hit it because
    // `has_audio` was false. The fix drops the native frame until the audio
    // format is known, then starts the writer and flushes the buffered audio.
    #[test]
    fn native_video_with_audio_waits_for_audio_format() {
        let path = std::env::temp_dir().join("mw_native_av_regression.mp4");
        let _ = std::fs::remove_file(&path);

        let params = WorkerParams {
            path: path.clone(),
            has_video: true,
            has_audio: true,
            fps: 30,
            video_bitrate: None,
            audio_bitrate: None,
        };
        let mut enc = Encoder::new(params);
        let surf = make_test_surface(64, 48);

        // First native frame, audio not yet seen: must NOT panic and must NOT
        // start the writer (this is the exact panic site before the fix).
        enc.on_native(surf, 1_000);
        assert!(!enc.started, "writer must wait for the audio format");
        assert!(enc.failed.is_none(), "no failure while waiting: {:?}", enc.failed);

        // Audio arrives → both formats known → writer starts + buffered audio flushes.
        let chunk = vec![0.0f32; 480]; // ~10ms mono @ 48k
        enc.on_audio(48_000, 1, 1_000, chunk.clone());
        assert!(enc.started, "writer starts once the audio format is known");

        // A few more interleaved frames append cleanly (steady state).
        for i in 1..6u64 {
            enc.on_native(surf, 1_000 + i * 33_000);
            enc.on_audio(48_000, 1, 1_000 + i * 10_000, chunk.clone());
        }
        assert!(enc.failed.is_none(), "steady-state append failed: {:?}", enc.failed);

        // Finalize: a real, non-empty mp4 lands.
        let r = enc.finish();
        assert!(r.is_ok(), "finish failed: {r:?}");
        let meta = std::fs::metadata(&path).expect("output file exists");
        assert!(meta.len() > 0, "output mp4 is empty");

        unsafe { CFRelease(surf) };
        let _ = std::fs::remove_file(&path);
    }
}
