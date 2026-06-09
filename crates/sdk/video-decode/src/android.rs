//! Android file-decode backend — `MediaExtractor` + `MediaCodec` via a Kotlin
//! shim ([`RustVideoDecoder`]), shipped from this crate via
//! `[package.metadata.idealyst.android].runtime_kotlin`.
//!
//! ## Why a Kotlin shim
//!
//! Android's media decode stack (`MediaExtractor` to demux the container,
//! `MediaCodec` to decode each track, `ImageReader` to receive decoded video
//! surfaces) is callback- and thread-driven Java API. Driving the codec dequeue
//! loops, subclassing `ImageReader.OnImageAvailableListener`, and running the
//! decode pump on a looper thread isn't feasible purely from JNI, so that
//! machinery lives in a tiny Kotlin shim. The shim:
//!
//! - demuxes the source URL, selecting the first video track and (if present)
//!   the first audio track;
//! - decodes the video track with `MediaCodec` into an `ImageReader` surface
//!   (`PixelFormat.RGBA_8888`), converts each acquired `Image` to tightly-packed
//!   top-down `RGBA8`, and trampolines it through [`nativeFrameDirect`];
//! - decodes the audio track with `MediaCodec` to PCM16, converts to interleaved
//!   `f32`, and trampolines it through [`nativeAudio`];
//! - owns the playback clock (driven off the video PTS, throttled to real time)
//!   and exposes `play`/`pause`/`seek`/`setMuted`/`setRate`, reporting
//!   `position`/`duration`/`isPlaying` back through [`nativeState`] so the
//!   transport getters read a cached value without a synchronous JNI round-trip.
//!
//! ## Async bridge (mirrors `camera/android.rs`)
//!
//! [`open`] mints a `u64` token, parks the frame + audio writers and a shared
//! state cell in a process-global registry keyed by that token, parks a oneshot
//! for the open result, and hands the token to the shim. `nativeOpened` /
//! `nativeError` resolve the open future; `nativeFrameDirect` / `nativeAudio`
//! look the writers up by token and deliver media; `nativeState` updates the
//! cached transport state. Dropping the [`StreamHandle`] calls the shim's
//! `close(token)` and unregisters everything.
//!
//! ## VERIFICATION
//!
//! Compile-shaped for `aarch64-linux-android` here, but **not yet
//! device-verified** — the JNI signatures, the Kotlin `MediaCodec` decode path,
//! and the `native*` symbol exports resolve only at runtime on a device (same
//! posture as the `camera` crate's Android backend). Every failure is surfaced
//! as a typed [`VideoDecodeError`] carrying the JNI/Android message. The exports
//! are pinned with `#[used]` so the linker keeps them in the app `cdylib`'s
//! dynsym for `dlsym` resolution by the JVM.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use futures_channel::oneshot;
use jni::objects::{JByteBuffer, JClass, JObject, JString, JValue};
use jni::sys::{jboolean, jfloat, jint, jlong};
use jni::{JNIEnv, JavaVM};

use media_stream::{AudioWriter, FrameWriter};

use crate::{DecodeConfig, DecodeSource, Opened, TransportControl, VideoDecodeError};

const HELPER_CLASS: &str = "io/idealyst/videodecode/RustVideoDecoder";

// Error-code sentinels the shim sends in `nativeError`, mapped by
// `map_open_error`.
const ERR_BAD_SOURCE: jint = -2;

/// The sender that resolves an awaiting `open()` once the shim reports the clip
/// open (with `has_audio` + natural size) or failed.
type OpenResult = Result<OpenInfo, VideoDecodeError>;
type OpenSender = oneshot::Sender<OpenResult>;

/// What the shim reports back at successful open.
struct OpenInfo {
    has_audio: bool,
    natural_size: Option<(u32, u32)>,
}

/// The two writers + the shared transport-state cell for one open clip. The JNI
/// trampolines clone the `Send` writers out from under the registry lock and
/// push into them without holding the global lock across the channel fan-out.
struct Entry {
    frames: FrameWriter,
    /// `None` when the clip carries no audio track.
    audio: Option<AudioWriter>,
    /// The transport-state cell the `AndroidTransport` reads; `nativeState`
    /// writes it. Shared (`Arc`) with the transport so getters need no JNI.
    state: std::sync::Arc<Mutex<TransportState>>,
}

/// token → entry, for media in flight.
static ENTRIES: OnceLock<Mutex<HashMap<u64, Entry>>> = OnceLock::new();
/// token → the sender awaiting the open result.
static PENDING_OPEN: OnceLock<Mutex<HashMap<u64, OpenSender>>> = OnceLock::new();
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

fn entries() -> &'static Mutex<HashMap<u64, Entry>> {
    ENTRIES.get_or_init(|| Mutex::new(HashMap::new()))
}
fn pending_open() -> &'static Mutex<HashMap<u64, OpenSender>> {
    PENDING_OPEN.get_or_init(|| Mutex::new(HashMap::new()))
}

// ---------------------------------------------------------------------------
// Transport — drives the Kotlin shim's clock + reads cached state.
// ---------------------------------------------------------------------------

/// Cached playback state, written by `nativeState` and read by the transport
/// getters. Avoids a synchronous JNI round-trip on every `position()` poll (a
/// scrubber polls this every display tick).
#[derive(Clone, Copy, Default)]
struct TransportState {
    position: f32,
    duration: f32,
    playing: bool,
    muted: bool,
}

/// Per-platform transport. Commands (`play`/`pause`/`seek`/…) are forwarded to
/// the shim over JNI; state getters read the cached [`TransportState`].
struct AndroidTransport {
    token: u64,
    state: std::sync::Arc<Mutex<TransportState>>,
}

impl AndroidTransport {
    /// Best-effort static-void call into the shim, `(J<extra>)V`. Errors are
    /// swallowed — a transport command on a torn-down clip is a no-op, matching
    /// the trait's defaulted, non-panicking posture.
    fn call_void(&self, method: &str, sig: &str, extra: &[JValue]) {
        let Ok(vm) = java_vm() else { return };
        let Ok(mut env) = vm.attach_current_thread() else {
            return;
        };
        let mut args = Vec::with_capacity(1 + extra.len());
        args.push(JValue::Long(self.token as jlong));
        args.extend_from_slice(extra);
        let _ = env.call_static_method(HELPER_CLASS, method, sig, &args);
    }
}

impl TransportControl for AndroidTransport {
    fn play(&self) {
        self.call_void("play", "(J)V", &[]);
    }
    fn pause(&self) {
        self.call_void("pause", "(J)V", &[]);
    }
    fn seek(&self, seconds: f32) {
        // Optimistically reflect the seek locally so a scrubber doesn't snap
        // back for the tick before the shim's next `nativeState`.
        if let Ok(mut s) = self.state.lock() {
            s.position = seconds.max(0.0);
        }
        self.call_void("seek", "(JF)V", &[JValue::Float(seconds.max(0.0) as jfloat)]);
    }
    fn set_muted(&self, muted: bool) {
        if let Ok(mut s) = self.state.lock() {
            s.muted = muted;
        }
        self.call_void(
            "setMuted",
            "(JZ)V",
            &[JValue::Bool(muted as jboolean)],
        );
    }
    fn set_rate(&self, rate: f32) {
        self.call_void("setRate", "(JF)V", &[JValue::Float(rate.max(0.0) as jfloat)]);
    }
    fn position(&self) -> f32 {
        self.state.lock().map(|s| s.position).unwrap_or(0.0)
    }
    fn duration(&self) -> f32 {
        self.state.lock().map(|s| s.duration).unwrap_or(0.0)
    }
    fn is_playing(&self) -> bool {
        self.state.lock().map(|s| s.playing).unwrap_or(false)
    }
    fn is_muted(&self) -> bool {
        self.state.lock().map(|s| s.muted).unwrap_or(false)
    }
}

// ---------------------------------------------------------------------------
// StreamHandle — keeps decode alive; Drop stops it.
// ---------------------------------------------------------------------------

/// Stops decode on drop: tells the shim to tear down its codecs / extractor and
/// unregisters this token's writers + pending open.
pub(crate) struct StreamHandle {
    token: u64,
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        // Best-effort close via the shim, then drop the writers so no late
        // frame/PCM can reach freed state.
        if let Ok(vm) = java_vm() {
            if let Ok(mut env) = vm.attach_current_thread() {
                let _ = env.call_static_method(
                    HELPER_CLASS,
                    "close",
                    "(J)V",
                    &[JValue::Long(self.token as jlong)],
                );
            }
        }
        entries().lock().unwrap().remove(&self.token);
        pending_open().lock().unwrap().remove(&self.token);
    }
}

// ---------------------------------------------------------------------------
// Open.
// ---------------------------------------------------------------------------

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

    let vm = java_vm()?;
    let mut env = vm.attach_current_thread().map_err(jni_err)?;
    let activity = android_context();

    let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
    let state = std::sync::Arc::new(Mutex::new(TransportState {
        muted: config.muted,
        ..TransportState::default()
    }));

    // Park the writers + state BEFORE asking the shim to open: a frame/state
    // callback can fire the instant `open` returns on the shim's thread. We
    // don't yet know `has_audio`, so register the audio writer too; if the
    // shim reports no audio track we drop it after the open resolves (and the
    // shim never calls `nativeAudio`).
    entries().lock().unwrap().insert(
        token,
        Entry {
            frames,
            audio: Some(audio),
            state: state.clone(),
        },
    );
    let (tx, rx) = oneshot::channel::<OpenResult>();
    pending_open().lock().unwrap().insert(token, tx);

    let url = env.new_string(&url_str).map_err(|e| {
        cleanup_token(token);
        jni_err(e)
    })?;

    // max_dimension: 0 = "no constraint" for the shim (decode at natural size).
    let max_dim = config.max_dimension.unwrap_or(0) as jint;

    let launch = env.call_static_method(
        HELPER_CLASS,
        "open",
        "(Landroid/content/Context;Ljava/lang/String;ZZZIJ)V",
        &[
            JValue::Object(&activity),
            JValue::Object(&JObject::from(url)),
            JValue::Bool(config.autoplay as jboolean),
            JValue::Bool(config.loop_playback as jboolean),
            JValue::Bool(config.muted as jboolean),
            JValue::Int(max_dim),
            JValue::Long(token as jlong),
        ],
    );
    if let Err(e) = launch {
        // The shim never got a chance to call back — clean up the registries.
        cleanup_token(token);
        return Err(jni_err(e));
    }

    match rx.await {
        Ok(Ok(info)) => {
            // If the clip has no audio track, drop the parked audio writer so
            // `lib.rs` reports `audio() == None` (it keys off `has_audio`).
            if !info.has_audio {
                if let Some(entry) = entries().lock().unwrap().get_mut(&token) {
                    entry.audio = None;
                }
            }
            let control: Rc<dyn TransportControl> =
                Rc::new(AndroidTransport { token, state });
            Ok(Opened {
                handle: Box::new(StreamHandle { token }),
                control,
                has_audio: info.has_audio,
                natural_size: info.natural_size,
            })
        }
        Ok(Err(e)) => {
            cleanup_token(token);
            Err(e)
        }
        Err(_) => {
            cleanup_token(token);
            Err(VideoDecodeError::Backend(
                "video-decode open channel dropped".into(),
            ))
        }
    }
}

fn cleanup_token(token: u64) {
    entries().lock().unwrap().remove(&token);
    pending_open().lock().unwrap().remove(&token);
}

// ---------------------------------------------------------------------------
// JNI helpers
// ---------------------------------------------------------------------------

fn java_vm() -> Result<JavaVM, VideoDecodeError> {
    let ctx = ndk_context::android_context();
    let vm_ptr = ctx.vm() as *mut jni::sys::JavaVM;
    unsafe { JavaVM::from_raw(vm_ptr) }
        .map_err(|e| VideoDecodeError::Backend(format!("invalid JavaVM pointer: {e}")))
}

fn jni_err(e: jni::errors::Error) -> VideoDecodeError {
    VideoDecodeError::Backend(format!("JNI: {e}"))
}

fn android_context<'a>() -> JObject<'a> {
    let ctx = ndk_context::android_context();
    unsafe { JObject::from_raw(ctx.context().cast()) }
}

/// Map the shim's error `(code, message)` to a typed [`VideoDecodeError`].
fn map_open_error(code: jint, message: Option<String>) -> VideoDecodeError {
    match code {
        ERR_BAD_SOURCE => {
            VideoDecodeError::BadSource(message.unwrap_or_else(|| "unreadable source".into()))
        }
        _ => VideoDecodeError::Backend(format!(
            "video decode open failed (code {code}): {}",
            message.unwrap_or_default()
        )),
    }
}

// ---------------------------------------------------------------------------
// JNI exports — the Kotlin shim's trampolines.
// ---------------------------------------------------------------------------

/// `RustVideoDecoder.nativeOpened` — the clip is open and decode has started.
/// Carries `has_audio` + natural size so `open` can build [`Opened`].
///
/// # Safety
/// Called by the JVM with a valid `env`/`class`.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_videodecode_RustVideoDecoder_nativeOpened(
    _env: JNIEnv,
    _class: JClass,
    token: jlong,
    has_audio: jboolean,
    width: jint,
    height: jint,
) {
    let result = std::panic::catch_unwind(|| {
        let natural_size = if width > 0 && height > 0 {
            Some((width as u32, height as u32))
        } else {
            None
        };
        let info = OpenInfo {
            has_audio: has_audio != 0,
            natural_size,
        };
        if let Some(tx) = pending_open().lock().unwrap().remove(&(token as u64)) {
            let _ = tx.send(Ok(info));
        }
    });
    if result.is_err() {
        eprintln!("video-decode: panic in nativeOpened trampoline; aborting");
        std::process::abort();
    }
}

/// `RustVideoDecoder.nativeError` — the clip failed to open / decode. Resolves
/// the open future with a typed error.
///
/// # Safety
/// Called by the JVM with a valid `env`/`class`; `message` is a `String?`.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_videodecode_RustVideoDecoder_nativeError(
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
        eprintln!("video-decode: panic in nativeError trampoline; aborting");
        std::process::abort();
    }
}

/// `RustVideoDecoder.nativeFrameDirect` — one decoded video frame in a REUSED
/// direct `ByteBuffer` of tightly-packed top-down `RGBA8` (`width*height*4`
/// bytes). Read ZERO-COPY via `GetDirectBufferAddress`; the shim only reuses the
/// buffer after this synchronous call returns. Delivered on the shim's decode /
/// ImageReader handler thread.
///
/// # Safety
/// Called by the JVM with a valid `env`/`class`; `buffer` is a direct
/// `java.nio.ByteBuffer` that stays alive and unmodified for the duration of
/// this synchronous call.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_videodecode_RustVideoDecoder_nativeFrameDirect(
    env: JNIEnv,
    _class: JClass,
    token: jlong,
    buffer: JByteBuffer,
    width: jint,
    height: jint,
    pts_micros: jlong,
) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if width <= 0 || height <= 0 {
            return;
        }
        // Clone the frame writer out from under the registry lock, then push
        // into it without holding the global lock across the channel fan-out.
        let writer = entries()
            .lock()
            .unwrap()
            .get(&(token as u64))
            .map(|e| e.frames.clone());
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
        // exactly `needed` bytes during this synchronous call, before the shim
        // reuses the buffer for the next frame.
        let bytes = unsafe { std::slice::from_raw_parts(addr, needed) };
        // Use the decoder's presentation timestamp so a muxer sees the true
        // clip cadence rather than the moment the copy happened.
        let pts = pts_micros.max(0) as u64;
        writer.write_rgba8_at(width as u32, height as u32, bytes, pts);
    }));
    if result.is_err() {
        eprintln!("video-decode: panic in nativeFrameDirect trampoline; aborting");
        std::process::abort();
    }
}

/// `RustVideoDecoder.nativeAudio` — a chunk of decoded audio in a REUSED direct
/// `ByteBuffer` of interleaved `f32` PCM (`frames*channels` samples). Read
/// ZERO-COPY via `GetDirectBufferAddress`. Delivered on the shim's audio-decode
/// thread.
///
/// # Safety
/// Called by the JVM with a valid `env`/`class`; `buffer` is a direct
/// `java.nio.ByteBuffer` (native byte order) live + unmodified for this call.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_videodecode_RustVideoDecoder_nativeAudio(
    env: JNIEnv,
    _class: JClass,
    token: jlong,
    buffer: JByteBuffer,
    sample_rate: jint,
    channels: jint,
    frames: jint,
    pts_micros: jlong,
) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if sample_rate <= 0 || channels <= 0 || frames <= 0 {
            return;
        }
        let writer = entries()
            .lock()
            .unwrap()
            .get(&(token as u64))
            .and_then(|e| e.audio.clone());
        let Some(writer) = writer else {
            return;
        };

        let sample_count = (channels as usize) * (frames as usize);
        let needed = sample_count * std::mem::size_of::<f32>();
        let addr = match env.get_direct_buffer_address(&buffer) {
            Ok(p) if !p.is_null() => p,
            _ => return,
        };
        if env.get_direct_buffer_capacity(&buffer).unwrap_or(0) < needed {
            return;
        }
        // SAFETY: `addr` points at the off-heap region of a live direct
        // ByteBuffer whose capacity we checked is >= `needed`; the shim writes
        // native-endian `f32`s and only reuses the buffer after this returns.
        // `addr` is 4-aligned (allocateDirect returns at least 8-byte
        // alignment), so the `f32` read is well-formed.
        let samples =
            unsafe { std::slice::from_raw_parts(addr as *const f32, sample_count) };
        writer.write_pcm_f32_at(
            sample_rate as u32,
            channels as u16,
            samples,
            pts_micros.max(0) as u64,
        );
    }));
    if result.is_err() {
        eprintln!("video-decode: panic in nativeAudio trampoline; aborting");
        std::process::abort();
    }
}

/// `RustVideoDecoder.nativeState` — the shim's periodic transport-state push
/// (position/duration in seconds, playing/muted flags). Cached so the transport
/// getters need no synchronous JNI round-trip.
///
/// # Safety
/// Called by the JVM with a valid `env`/`class`.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_videodecode_RustVideoDecoder_nativeState(
    _env: JNIEnv,
    _class: JClass,
    token: jlong,
    position: jfloat,
    duration: jfloat,
    playing: jboolean,
    muted: jboolean,
) {
    let result = std::panic::catch_unwind(|| {
        if let Some(entry) = entries().lock().unwrap().get(&(token as u64)) {
            if let Ok(mut s) = entry.state.lock() {
                s.position = position.max(0.0);
                s.duration = duration.max(0.0);
                s.playing = playing != 0;
                s.muted = muted != 0;
            }
        }
    });
    if result.is_err() {
        eprintln!("video-decode: panic in nativeState trampoline; aborting");
        std::process::abort();
    }
}

// Pin the exports so the linker keeps them in the app `cdylib`'s dynamic symbol
// table (the JVM resolves them by `dlsym`).
#[used]
static KEEP_NATIVE_OPENED: extern "system" fn(JNIEnv, JClass, jlong, jboolean, jint, jint) =
    Java_io_idealyst_videodecode_RustVideoDecoder_nativeOpened;
#[used]
static KEEP_NATIVE_ERROR: extern "system" fn(JNIEnv, JClass, jlong, jint, JString) =
    Java_io_idealyst_videodecode_RustVideoDecoder_nativeError;
#[used]
static KEEP_NATIVE_FRAME: extern "system" fn(JNIEnv, JClass, jlong, JByteBuffer, jint, jint, jlong) =
    Java_io_idealyst_videodecode_RustVideoDecoder_nativeFrameDirect;
#[used]
static KEEP_NATIVE_AUDIO: extern "system" fn(
    JNIEnv,
    JClass,
    jlong,
    JByteBuffer,
    jint,
    jint,
    jint,
    jlong,
) = Java_io_idealyst_videodecode_RustVideoDecoder_nativeAudio;
#[used]
static KEEP_NATIVE_STATE: extern "system" fn(JNIEnv, JClass, jlong, jfloat, jfloat, jboolean, jboolean) =
    Java_io_idealyst_videodecode_RustVideoDecoder_nativeState;
