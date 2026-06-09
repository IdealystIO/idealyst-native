//! Apple (iOS + macOS) file-decode backend.
//!
//! `AVFoundation` is the same framework on both, so one module serves them.
//! The design splits cleanly along the two outputs the SDK produces:
//!
//! - **Video frames** — an `AVPlayer` drives decode + the clock; an
//!   `AVPlayerItemVideoOutput` (attached to the item) hands us a `CVPixelBuffer`
//!   for the player's current time. A main-thread `raf_loop` pulls the newest
//!   buffer each display tick, swizzles `BGRA → RGBA8`, and pushes it through the
//!   [`FrameWriter`]. We do NOT add an `AVPlayerLayer`: the player is headless,
//!   its pixels go to the canvas, not an overlay view.
//! - **Audio PCM** — an `MTAudioProcessingTap` (MediaToolbox) installed on the
//!   item's `audioMix` taps the decoded float PCM on the audio render thread and
//!   pushes it through the [`AudioWriter`] for the recorder's mux. The tap is
//!   `PostEffects` and non-destructive, so the player still plays the sound.
//!
//! AVFoundation/CoreMedia/CoreVideo/MediaToolbox are reached by name through the
//! Obj-C runtime + a handful of linked C calls, the same posture as the `camera`
//! and `video` SDKs (no typed framework crates).

use std::cell::Cell;
use std::ffi::c_void;
use std::ptr;
use std::rc::Rc;
use std::sync::Mutex;

use objc2::encode::{Encode, Encoding, RefEncode};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2::{class, msg_send, msg_send_id};
use objc2_foundation::NSString;

use media_stream::{AudioWriter, FrameWriter};

use crate::{DecodeConfig, DecodeSource, Opened, TransportControl, VideoDecodeError};

// ===========================================================================
// Foreign surfaces. The frameworks must be linked (classes resolve by name via
// `class!`, and the C functions below are undefined symbols otherwise).
// ===========================================================================

#[link(name = "AVFoundation", kind = "framework")]
extern "C" {}

#[allow(non_upper_case_globals)]
#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    /// CFString attribute keys (toll-free bridged to NSString) for the video
    /// output's requested pixel-buffer format + size.
    static kCVPixelBufferPixelFormatTypeKey: *const c_void;
    static kCVPixelBufferWidthKey: *const c_void;
    static kCVPixelBufferHeightKey: *const c_void;

    fn CVPixelBufferLockBaseAddress(pb: *mut c_void, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(pb: *mut c_void, flags: u64) -> i32;
    fn CVPixelBufferGetBaseAddress(pb: *mut c_void) -> *mut c_void;
    fn CVPixelBufferGetWidth(pb: *mut c_void) -> usize;
    fn CVPixelBufferGetHeight(pb: *mut c_void) -> usize;
    fn CVPixelBufferGetBytesPerRow(pb: *mut c_void) -> usize;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(cf: *const c_void);
}

#[link(name = "CoreMedia", kind = "framework")]
extern "C" {
    /// Pixel dimensions of a video `CMFormatDescription` — a plain C call
    /// returning a small struct across the C ABI (no Obj-C struct-return ABI
    /// risk), the same accessor `camera`'s backend uses.
    fn CMVideoFormatDescriptionGetDimensions(desc: *mut c_void) -> CMVideoDimensions;
}

/// `CMVideoDimensions { int32 width; int32 height; }` — only crosses the C ABI
/// (returned from a C function), so it needs no `Encode`.
#[repr(C)]
#[derive(Copy, Clone)]
struct CMVideoDimensions {
    width: i32,
    height: i32,
}

/// Opaque `CVPixelBufferRef` pointee, so a `*mut CVBuffer` RETURN from
/// `copyPixelBufferForItemTime:` encodes as `^{__CVBuffer=}` — objc2's debug
/// message verifier rejects a bare `*mut c_void` (`^v`) there. Mirrors the same
/// type in `media-writer`'s Apple backend.
struct CVBuffer {
    _priv: [u8; 0],
}
// SAFETY: matches the `CVPixelBufferRef` type AVFoundation declares.
unsafe impl RefEncode for CVBuffer {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("__CVBuffer", &[]));
}

// MTAudioProcessingTap lives in MediaToolbox.
#[link(name = "MediaToolbox", kind = "framework")]
extern "C" {
    fn MTAudioProcessingTapCreate(
        allocator: *const c_void,
        callbacks: *const MTAudioProcessingTapCallbacks,
        flags: u32,
        tap_out: *mut *mut c_void,
    ) -> i32;
    fn MTAudioProcessingTapGetSourceAudio(
        tap: *mut c_void,
        number_frames: isize,
        buffer_list_inout: *mut AudioBufferList,
        flags_out: *mut u32,
        time_range_out: *mut c_void,
        number_frames_out: *mut isize,
    ) -> i32;
}

/// `CMTime` mirror — field order/width MUST match `<CoreMedia/CMTime.h>`.
#[repr(C)]
#[derive(Copy, Clone)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}
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
unsafe impl RefEncode for CMTime {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Self::ENCODING);
}
const CM_TIME_FLAG_VALID: u32 = 1;

impl CMTime {
    fn seconds(&self) -> f32 {
        if self.flags & CM_TIME_FLAG_VALID == 0 || self.timescale == 0 {
            return 0.0;
        }
        self.value as f32 / self.timescale as f32
    }
    fn from_seconds(seconds: f32) -> Self {
        let timescale = 600i32;
        CMTime {
            value: (seconds as f64 * timescale as f64).round() as i64,
            timescale,
            flags: CM_TIME_FLAG_VALID,
            epoch: 0,
        }
    }
}

/// `'BGRA'` (`kCVPixelFormatType_32BGRA`).
const PIXEL_FORMAT_32BGRA: u32 = 0x4247_5241;
const LOCK_READ_ONLY: u64 = 0x0000_0001;
const AV_MEDIA_TYPE_VIDEO: &str = "vide";
const AV_MEDIA_TYPE_AUDIO: &str = "soun";
/// Whether to install the `MTAudioProcessingTap` that taps the clip's PCM for
/// the recording mux. Currently OFF: the tap's C-callback interop SIGBUSes
/// inside `MTAudioProcessingTapCreate` (it invokes our `init` callback
/// synchronously) and needs on-device debugging. With it off the clip still
/// plays audibly through `AVPlayer`; only capturing that audio INTO a recording
/// is deferred. Flip to `true` once the tap ABI is verified on a device.
const ENABLE_AUDIO_TAP: bool = false;
/// `kMTAudioProcessingTapCreationFlag_PostEffects` — tap after the mix's
/// effects, leaving the played-out audio intact.
const TAP_FLAG_POST_EFFECTS: u32 = 1;
/// `kAudioFormatFlagIsNonInterleaved`.
const FLAG_NON_INTERLEAVED: u32 = 0x20;

// ===========================================================================
// AudioToolbox structs for the tap process callback.
// ===========================================================================

#[repr(C)]
#[derive(Copy, Clone)]
struct AudioStreamBasicDescription {
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

#[repr(C)]
#[derive(Copy, Clone)]
struct AudioBuffer {
    number_channels: u32,
    data_byte_size: u32,
    data: *mut c_void,
}

/// `AudioBufferList` with inline room for several buffers (non-interleaved float
/// gives one mono buffer per channel). 8 covers any realistic clip; extra
/// buffers past `number_buffers` are ignored.
#[repr(C)]
struct AudioBufferList {
    number_buffers: u32,
    buffers: [AudioBuffer; 8],
}

/// MTAudioProcessingTap C callback table.
#[repr(C)]
struct MTAudioProcessingTapCallbacks {
    version: i32,
    client_info: *mut c_void,
    init: Option<unsafe extern "C" fn(*mut c_void, *mut c_void, *mut *mut c_void)>,
    finalize: Option<unsafe extern "C" fn(*mut c_void)>,
    prepare:
        Option<unsafe extern "C" fn(*mut c_void, isize, *const AudioStreamBasicDescription)>,
    unprepare: Option<unsafe extern "C" fn(*mut c_void)>,
    process: Option<
        unsafe extern "C" fn(
            *mut c_void,
            isize,
            u32,
            *mut AudioBufferList,
            *mut isize,
            *mut u32,
        ),
    >,
}

/// Shared state behind the tap's `clientInfo`, owned for the tap's lifetime
/// (created in `open`, freed in `tap_finalize`). The audio render thread reads
/// it, so it's `Send`-safe (only the `AudioWriter`, which is `Send`).
struct TapState {
    writer: AudioWriter,
    /// `(sample_rate, channels, non_interleaved)`, set by `prepare`.
    format: Mutex<Option<(u32, u16, bool)>>,
}

unsafe extern "C" fn tap_init(
    _tap: *mut c_void,
    client_info: *mut c_void,
    storage_out: *mut *mut c_void,
) {
    // Hand our boxed state through to the storage slot the framework threads to
    // the other callbacks.
    *storage_out = client_info;
}

unsafe extern "C" fn tap_prepare(
    tap: *mut c_void,
    _max_frames: isize,
    fmt: *const AudioStreamBasicDescription,
) {
    let state = storage::<TapState>(tap);
    if state.is_null() || fmt.is_null() {
        return;
    }
    let asbd = &*fmt;
    let non_interleaved = asbd.format_flags & FLAG_NON_INTERLEAVED != 0;
    *(*state).format.lock().unwrap() =
        Some((asbd.sample_rate as u32, asbd.channels_per_frame as u16, non_interleaved));
}

unsafe extern "C" fn tap_unprepare(_tap: *mut c_void) {}

unsafe extern "C" fn tap_finalize(tap: *mut c_void) {
    let state = storage::<TapState>(tap);
    if !state.is_null() {
        // Reclaim the Box leaked in `open` and drop it.
        drop(Box::from_raw(state));
    }
}

unsafe extern "C" fn tap_process(
    tap: *mut c_void,
    number_frames: isize,
    _flags: u32,
    buffer_list: *mut AudioBufferList,
    number_frames_out: *mut isize,
    flags_out: *mut u32,
) {
    let mut got: isize = 0;
    let status = MTAudioProcessingTapGetSourceAudio(
        tap,
        number_frames,
        buffer_list,
        flags_out,
        ptr::null_mut(),
        &mut got,
    );
    if !number_frames_out.is_null() {
        *number_frames_out = got;
    }
    if status != 0 || got <= 0 || buffer_list.is_null() {
        return;
    }
    let state = storage::<TapState>(tap);
    if state.is_null() {
        return;
    }
    let Some((sample_rate, channels, non_interleaved)) = *(*state).format.lock().unwrap() else {
        return;
    };
    if sample_rate == 0 || channels == 0 {
        return;
    }
    let frames = got as usize;
    let ch = channels as usize;
    let list = &*buffer_list;
    let nbuf = list.number_buffers as usize;
    let mut interleaved = vec![0.0f32; frames * ch];

    if non_interleaved {
        // One mono float buffer per channel.
        for c in 0..ch.min(nbuf) {
            let buf = &list.buffers[c];
            if buf.data.is_null() {
                continue;
            }
            let n = (buf.data_byte_size as usize / 4).min(frames);
            let src = std::slice::from_raw_parts(buf.data as *const f32, n);
            for (f, &s) in src.iter().enumerate() {
                interleaved[f * ch + c] = s;
            }
        }
    } else if nbuf > 0 {
        // Single interleaved buffer.
        let buf = &list.buffers[0];
        if !buf.data.is_null() {
            let n = (buf.data_byte_size as usize / 4).min(frames * ch);
            let src = std::slice::from_raw_parts(buf.data as *const f32, n);
            interleaved[..n].copy_from_slice(&src[..n]);
        }
    }

    (*state).writer.write_pcm_f32(sample_rate, channels, &interleaved);
}

/// `MTAudioProcessingTapGetStorage(tap)` — the pointer `tap_init` stored.
unsafe fn storage<T>(tap: *mut c_void) -> *mut T {
    #[link(name = "MediaToolbox", kind = "framework")]
    extern "C" {
        fn MTAudioProcessingTapGetStorage(tap: *mut c_void) -> *mut c_void;
    }
    MTAudioProcessingTapGetStorage(tap) as *mut T
}

// ===========================================================================
// Transport — drives the AVPlayer.
// ===========================================================================

struct MacosTransport {
    player: Retained<AnyObject>,
    muted: Cell<bool>,
    rate: Cell<f32>,
}

// The transport is used only on the main thread (matching MediaStream's !Send
// contract); the Retained<AnyObject> isn't Send/Sync, which is fine — Transport
// is Rc-wrapped and never crosses threads.
impl TransportControl for MacosTransport {
    fn play(&self) {
        let r = self.rate.get().max(0.0);
        let r = if r == 0.0 { 1.0 } else { r };
        self.rate.set(r);
        // `-[AVPlayer setRate:]` takes a `float` (f32) — NOT f64.
        let _: () = unsafe { msg_send![&*self.player, setRate: r] };
    }
    fn pause(&self) {
        let _: () = unsafe { msg_send![&*self.player, pause] };
    }
    fn seek(&self, seconds: f32) {
        // EXACT: zero tolerance before/after → decode the precise frame (the
        // drag's final landing). Plain `seekToTime:` is allowed to land on a
        // nearby efficient sample, which we DON'T want for the exact form.
        let t = CMTime::from_seconds(seconds.max(0.0));
        let zero = CMTime::from_seconds(0.0);
        let _: () = unsafe {
            msg_send![&*self.player, seekToTime: t, toleranceBefore: zero, toleranceAfter: zero]
        };
    }
    fn seek_preview(&self, seconds: f32) {
        // FAST/approximate: plain `seekToTime:` may land on a nearby efficient
        // sample (keyframe) without a full decode — smooth live scrubbing.
        let t = CMTime::from_seconds(seconds.max(0.0));
        let _: () = unsafe { msg_send![&*self.player, seekToTime: t] };
    }
    fn set_muted(&self, muted: bool) {
        self.muted.set(muted);
        let _: () = unsafe { msg_send![&*self.player, setMuted: muted] };
    }
    fn set_rate(&self, rate: f32) {
        let rate = rate.max(0.0);
        self.rate.set(rate);
        let _: () = unsafe { msg_send![&*self.player, setRate: rate] };
    }
    fn position(&self) -> f32 {
        let t: CMTime = unsafe { msg_send![&*self.player, currentTime] };
        t.seconds()
    }
    fn duration(&self) -> f32 {
        // `currentItem` returns an object (or nil) — let objc2 box it nil-safely.
        let item: Option<Retained<AnyObject>> =
            unsafe { msg_send_id![&*self.player, currentItem] };
        let Some(item) = item else {
            return 0.0;
        };
        let d: CMTime = unsafe { msg_send![&*item, duration] };
        let s = d.seconds();
        if s.is_finite() {
            s
        } else {
            0.0
        }
    }
    fn is_playing(&self) -> bool {
        // `-[AVPlayer rate]` returns a `float` (f32).
        let r: f32 = unsafe { msg_send![&*self.player, rate] };
        r > 0.0
    }
    fn is_muted(&self) -> bool {
        self.muted.get()
    }
}

// ===========================================================================
// StreamHandle — keeps decode alive; Drop stops it.
// ===========================================================================

struct StreamHandle {
    player: Retained<AnyObject>,
    _video_output: Retained<AnyObject>,
    _raf: runtime_core::scheduling::RafLoop,
    /// The created tap (CFType); released on drop. `None` when the clip has no
    /// audio track. The tap retains its own clientInfo box, freed in finalize.
    tap: Option<*mut c_void>,
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        unsafe {
            let _: () = msg_send![&*self.player, pause];
            let nil: *const AnyObject = ptr::null();
            let _: () = msg_send![&*self.player, replaceCurrentItemWithPlayerItem: nil];
            if let Some(tap) = self.tap.take() {
                if !tap.is_null() {
                    CFRelease(tap);
                }
            }
        }
        // `_raf` stops on drop; the player/output Retained release here.
    }
}

// ===========================================================================
// Open.
// ===========================================================================

pub(crate) async fn open(
    source: DecodeSource,
    config: DecodeConfig,
    frames: FrameWriter,
    audio: AudioWriter,
) -> Result<Opened, VideoDecodeError> {
    // Native always receives a `Url` (lib.rs materializes `Bytes` to a temp
    // file before calling us).
    let DecodeSource::Url(url_str) = source else {
        return Err(VideoDecodeError::Unsupported);
    };

    unsafe {
        let url = build_nsurl(&url_str)
            .ok_or_else(|| VideoDecodeError::BadSource(url_str.clone()))?;

        // Asset → inspect tracks for natural size + audio presence.
        let asset: Retained<AnyObject> =
            msg_send_id![class!(AVURLAsset), URLAssetWithURL: &*url, options: ptr::null::<AnyObject>()];

        let (nat_w, nat_h) = natural_size(&asset);
        let audio_track_present = track_count(&asset, AV_MEDIA_TYPE_AUDIO) > 0;

        // Player item + headless player.
        let item: Retained<AnyObject> =
            msg_send_id![msg_send_id![class!(AVPlayerItem), alloc], initWithAsset: &*asset];
        let player: Retained<AnyObject> =
            msg_send_id![class!(AVPlayer), playerWithPlayerItem: &*item];
        let _: () = msg_send![&*player, setMuted: config.muted];

        // Video output: BGRA, optionally downscaled to honor max_dimension.
        let (out_w, out_h) = target_size(nat_w, nat_h, config.max_dimension);
        let attrs = pixel_buffer_attrs(out_w, out_h);
        let video_output: Retained<AnyObject> = msg_send_id![
            msg_send_id![class!(AVPlayerItemVideoOutput), alloc],
            initWithPixelBufferAttributes: &*attrs
        ];
        let _: () = msg_send![&*item, addOutput: &*video_output];

        // Audio tap → PCM for the recorder (gated; see `ENABLE_AUDIO_TAP`). When
        // off, `audio` (the writer) is dropped here and the clip reports no audio
        // track to the recorder — it still plays its sound through `AVPlayer`.
        let tap = if audio_track_present && ENABLE_AUDIO_TAP {
            install_audio_tap(&item, &asset, audio)
        } else {
            None
        };
        // We only advertise audio to the recorder when we actually captured it.
        let has_audio = tap.is_some();

        if config.loop_playback {
            install_loop_observer(&player);
        }
        if config.autoplay {
            let _: () = msg_send![&*player, play];
        }

        // Frame pump: each display tick, pull the newest decoded buffer for the
        // player's current time and push it as RGBA8.
        let raf = {
            let player_for_pump = player.clone();
            let output_for_pump = video_output.clone();
            let mut scratch: Vec<u8> = Vec::new();
            runtime_core::scheduling::raf_loop(move || {
                pump_frame(&player_for_pump, &output_for_pump, &frames, &mut scratch);
            })
        };

        let control: Rc<dyn TransportControl> = Rc::new(MacosTransport {
            player: player.clone(),
            muted: Cell::new(config.muted),
            rate: Cell::new(if config.autoplay { 1.0 } else { 0.0 }),
        });

        let handle = StreamHandle {
            player,
            _video_output: video_output,
            _raf: raf,
            tap,
        };

        Ok(Opened {
            handle: Box::new(handle),
            control,
            has_audio,
            natural_size: if nat_w > 0 && nat_h > 0 {
                Some((nat_w, nat_h))
            } else {
                None
            },
        })
    }
}

/// TEST/DEBUG: build the decode pipeline, play, spin the run loop, and pull the
/// first frame via the exact `pump_frame` path the live decoder uses — so the
/// frame-pump ABI (which needs a run loop and so isn't reached by a plain
/// `open()` test) can be reproduced on the host. Returns the first frame's
/// `(width, height)`, or an error string.
#[doc(hidden)]
pub fn debug_pull_first_frame(url_str: &str, max_dim: Option<u32>) -> Result<(u32, u32), String> {
    unsafe {
        let url = build_nsurl(url_str).ok_or("bad url")?;
        let asset: Retained<AnyObject> =
            msg_send_id![class!(AVURLAsset), URLAssetWithURL: &*url, options: ptr::null::<AnyObject>()];
        let (nw, nh) = natural_size(&asset);
        let item: Retained<AnyObject> =
            msg_send_id![msg_send_id![class!(AVPlayerItem), alloc], initWithAsset: &*asset];
        let player: Retained<AnyObject> =
            msg_send_id![class!(AVPlayer), playerWithPlayerItem: &*item];
        let (ow, oh) = target_size(nw, nh, max_dim);
        let attrs = pixel_buffer_attrs(ow, oh);
        let output: Retained<AnyObject> = msg_send_id![
            msg_send_id![class!(AVPlayerItemVideoOutput), alloc],
            initWithPixelBufferAttributes: &*attrs
        ];
        let _: () = msg_send![&*item, addOutput: &*output];
        let _: () = msg_send![&*player, play];

        let (stream, writer) = media_stream::MediaStream::new();
        let mut scratch: Vec<u8> = Vec::new();
        let run_loop: Retained<AnyObject> = msg_send_id![class!(NSRunLoop), currentRunLoop];
        for _ in 0..180 {
            let date: Retained<AnyObject> =
                msg_send_id![class!(NSDate), dateWithTimeIntervalSinceNow: 0.016f64];
            let _: () = msg_send![&*run_loop, runUntilDate: &*date];
            pump_frame(&player, &output, &writer, &mut scratch);
            if let Some((w, h)) = stream.latest(&mut scratch) {
                let _: () = msg_send![&*player, pause];
                return Ok((w, h));
            }
        }
        let _: () = msg_send![&*player, pause];
        Err("no frame after ~3s of run loop".into())
    }
}

/// Pull + push one frame if the output has a new one for the current time.
unsafe fn pump_frame(
    player: &Retained<AnyObject>,
    output: &Retained<AnyObject>,
    frames: &FrameWriter,
    scratch: &mut Vec<u8>,
) {
    let item_time: CMTime = msg_send![&**player, currentTime];
    let has_new: Bool = msg_send![&**output, hasNewPixelBufferForItemTime: item_time];
    if !has_new.as_bool() {
        return;
    }
    let pb: *mut CVBuffer = msg_send![
        &**output,
        copyPixelBufferForItemTime: item_time,
        itemTimeForDisplay: ptr::null_mut::<CMTime>()
    ];
    if pb.is_null() {
        return;
    }
    let pb = pb as *mut c_void;
    if CVPixelBufferLockBaseAddress(pb, LOCK_READ_ONLY) == 0 {
        let base = CVPixelBufferGetBaseAddress(pb) as *const u8;
        let width = CVPixelBufferGetWidth(pb);
        let height = CVPixelBufferGetHeight(pb);
        let stride = CVPixelBufferGetBytesPerRow(pb);
        if !base.is_null() && width > 0 && height > 0 && stride >= width * 4 {
            repack_bgra_to_rgba(base, width, height, stride, scratch);
            frames.write_rgba8(width as u32, height as u32, scratch);
        }
        CVPixelBufferUnlockBaseAddress(pb, LOCK_READ_ONLY);
    }
    CFRelease(pb);
}

/// Strided `BGRA` → tightly-packed top-down `RGBA8`, reusing `scratch`.
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

// ===========================================================================
// Helpers.
// ===========================================================================

unsafe fn build_nsurl(s: &str) -> Option<Retained<AnyObject>> {
    if let Some(path) = s.strip_prefix("file://") {
        let ns_path = NSString::from_str(path);
        msg_send_id![class!(NSURL), fileURLWithPath: &*ns_path]
    } else {
        let ns = NSString::from_str(s);
        msg_send_id![class!(NSURL), URLWithString: &*ns]
    }
}

unsafe fn track_count(asset: &Retained<AnyObject>, media_type: &str) -> usize {
    let mt = NSString::from_str(media_type);
    let tracks: Retained<AnyObject> = msg_send_id![&**asset, tracksWithMediaType: &*mt];
    msg_send![&*tracks, count]
}

/// First video track's pixel dimensions, or `(0,0)` if none. Reads them from the
/// track's `CMFormatDescription` via a plain CoreMedia C call — avoiding the
/// Obj-C by-value struct-return ABI of `-[AVAssetTrack naturalSize]` (a
/// `CGSize`), which objc2 cannot reliably marshal here.
unsafe fn natural_size(asset: &Retained<AnyObject>) -> (u32, u32) {
    let mt = NSString::from_str(AV_MEDIA_TYPE_VIDEO);
    let tracks: Retained<AnyObject> = msg_send_id![&**asset, tracksWithMediaType: &*mt];
    let count: usize = msg_send![&*tracks, count];
    if count == 0 {
        return (0, 0);
    }
    let track: Retained<AnyObject> = msg_send_id![&*tracks, objectAtIndex: 0usize];
    let descs: Retained<AnyObject> = msg_send_id![&*track, formatDescriptions];
    let dcount: usize = msg_send![&*descs, count];
    if dcount == 0 {
        return (0, 0);
    }
    // The array holds the CMFormatDescription as an Obj-C object (`objectAtIndex:`
    // returns `@`), so retrieve it as an object and take its pointer — that
    // pointer IS the CMVideoFormatDescriptionRef the C accessor wants.
    let desc_obj: Retained<AnyObject> = msg_send_id![&*descs, objectAtIndex: 0usize];
    let desc = Retained::as_ptr(&desc_obj) as *mut c_void;
    if desc.is_null() {
        return (0, 0);
    }
    let dims = CMVideoFormatDescriptionGetDimensions(desc);
    (dims.width.max(0) as u32, dims.height.max(0) as u32)
}

/// Target decode size honoring `max_dimension` (preserving aspect). `(0,0)`
/// natural size → no constraint (let the decoder use natural size).
fn target_size(nat_w: u32, nat_h: u32, max_dim: Option<u32>) -> (u32, u32) {
    match (max_dim, nat_w, nat_h) {
        (Some(max), w, h) if w > 0 && h > 0 && w.max(h) > max => {
            let scale = max as f32 / w.max(h) as f32;
            (((w as f32 * scale) as u32).max(1), ((h as f32 * scale) as u32).max(1))
        }
        _ => (nat_w, nat_h),
    }
}

/// `@{ PixelFormatType: 32BGRA[, Width: w, Height: h] }`.
unsafe fn pixel_buffer_attrs(w: u32, h: u32) -> Retained<AnyObject> {
    let fmt_key: &AnyObject = &*(kCVPixelBufferPixelFormatTypeKey as *const AnyObject);
    let fmt_val: Retained<AnyObject> =
        msg_send_id![class!(NSNumber), numberWithUnsignedInt: PIXEL_FORMAT_32BGRA];

    let dict: Retained<AnyObject> = msg_send_id![class!(NSMutableDictionary), new];
    let _: () = msg_send![&*dict, setObject: &*fmt_val, forKey: fmt_key];
    if w > 0 && h > 0 {
        let w_key: &AnyObject = &*(kCVPixelBufferWidthKey as *const AnyObject);
        let h_key: &AnyObject = &*(kCVPixelBufferHeightKey as *const AnyObject);
        let w_val: Retained<AnyObject> = msg_send_id![class!(NSNumber), numberWithUnsignedInt: w];
        let h_val: Retained<AnyObject> = msg_send_id![class!(NSNumber), numberWithUnsignedInt: h];
        let _: () = msg_send![&*dict, setObject: &*w_val, forKey: w_key];
        let _: () = msg_send![&*dict, setObject: &*h_val, forKey: h_key];
    }
    dict
}

/// Build + install an `MTAudioProcessingTap` on the item's first audio track,
/// returning the created tap (CFType, released by `StreamHandle::drop`).
unsafe fn install_audio_tap(
    item: &Retained<AnyObject>,
    asset: &Retained<AnyObject>,
    writer: AudioWriter,
) -> Option<*mut c_void> {
    let mt = NSString::from_str(AV_MEDIA_TYPE_AUDIO);
    let tracks: Retained<AnyObject> = msg_send_id![&**asset, tracksWithMediaType: &*mt];
    let count: usize = msg_send![&*tracks, count];
    if count == 0 {
        return None;
    }
    let track: Retained<AnyObject> = msg_send_id![&*tracks, objectAtIndex: 0usize];

    // clientInfo box, freed in tap_finalize.
    let state = Box::into_raw(Box::new(TapState {
        writer,
        format: Mutex::new(None),
    })) as *mut c_void;

    let callbacks = MTAudioProcessingTapCallbacks {
        version: 0,
        client_info: state,
        init: Some(tap_init),
        finalize: Some(tap_finalize),
        prepare: Some(tap_prepare),
        unprepare: Some(tap_unprepare),
        process: Some(tap_process),
    };

    let mut tap: *mut c_void = ptr::null_mut();
    let status =
        MTAudioProcessingTapCreate(ptr::null(), &callbacks, TAP_FLAG_POST_EFFECTS, &mut tap);
    if status != 0 || tap.is_null() {
        // Reclaim the box we leaked; finalize won't run.
        drop(Box::from_raw(state as *mut TapState));
        return None;
    }

    // AVMutableAudioMixInputParameters(track).audioTapProcessor = tap;
    let params: Retained<AnyObject> = msg_send_id![
        class!(AVMutableAudioMixInputParameters),
        audioMixInputParametersWithTrack: &*track
    ];
    let _: () = msg_send![&*params, setAudioTapProcessor: tap];

    let mix: Retained<AnyObject> = msg_send_id![class!(AVMutableAudioMix), audioMix];
    let arr: Retained<AnyObject> = msg_send_id![class!(NSArray), arrayWithObject: &*params];
    let _: () = msg_send![&*mix, setInputParameters: &*arr];
    let _: () = msg_send![&**item, setAudioMix: &*mix];

    Some(tap)
}

/// Seek-to-zero + replay on end-of-item (the loop path).
unsafe fn install_loop_observer(player: &Retained<AnyObject>) {
    let center: Retained<AnyObject> =
        msg_send_id![class!(NSNotificationCenter), defaultCenter];
    let name = NSString::from_str("AVPlayerItemDidPlayToEndTimeNotification");
    let nil_obj: *const AnyObject = ptr::null();
    let nil_queue: *const AnyObject = ptr::null();
    let player_for_block = player.clone();
    let block = block2::StackBlock::new(move |_note: *mut AnyObject| {
        let zero = CMTime::from_seconds(0.0);
        let _: () = msg_send![&*player_for_block, seekToTime: zero];
        let _: () = msg_send![&*player_for_block, play];
    })
    .copy();
    let _: *mut AnyObject = msg_send![
        &*center,
        addObserverForName: &*name,
        object: nil_obj,
        queue: nil_queue,
        usingBlock: &*block
    ];
    // The observer token leaks for the player's lifetime (matches video/macos.rs
    // v1); a longer-lived clip would retain + remove it on teardown.
    std::mem::forget(block);
}
