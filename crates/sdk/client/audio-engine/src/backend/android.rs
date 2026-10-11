//! Android output via **AAudio**, rendering the shared Rust mixer — the same
//! shape as `native.rs` (cpal): the data callback owns the [`Mixer`] and
//! renders straight into the device buffer; everything above the device
//! (commands, voices, deferred frees) is [`MixerEngine`].
//!
//! ## Loading: `dlopen`, not a link
//!
//! AAudio exists from API 26. The framework's Android libraries are linked
//! at API 21 (the NDK's API-21 sysroot has no `libaaudio.so`, so a `-laaudio`
//! link fails outright), and even when linked at 26+ a `DT_NEEDED` on
//! `libaaudio.so` makes `System.loadLibrary` fail on an API 21–25 device —
//! the whole app would die at launch, not just its audio. So the library is
//! `dlopen`ed at [`start`] and every entry point resolved with `dlsym`;
//! below API 27 (read from `ro.build.version.sdk`; 8.0's AAudio is skipped,
//! see `aaudio_policy::AAUDIO_MIN_API`) or when the `dlopen`
//! fails, `start` returns [`EngineError::Unsupported`]. Types and constants
//! come from `ndk-sys` (generated from the NDK headers), with the `audio`
//! feature off so nothing is linked. The handle is never `dlclose`d: the
//! resolved function pointers live in a process-wide `OnceLock`.
//!
//! ## The stream
//!
//! Low-latency performance mode; exclusive sharing (the MMAP path) falling
//! back to shared; float falling back to i16; stereo; the device's native
//! rate (unspecified, so no resampler sits in the path); buffer = two
//! bursts. Order and sizing live in [`aaudio_policy`](super::aaudio_policy)
//! (host-tested). Usage GAME + content type SONIFICATION on API 28+ (their
//! setters don't exist before) — the system routes, ducks and applies the
//! game volume stream accordingly.
//!
//! ## Threads and the "one stream at a time" invariant
//!
//! The [`Render`] state (mixer, format, scratch) is touched only by the data
//! callback of the single open stream, and — while **no** stream is open or
//! started — by the thread that is opening one. Every open, start, pause and
//! close happens under [`Shared::control`], and `AAudioStream_close`
//! guarantees the data callback has returned and will not run again, so
//! the two never overlap. The data callback itself takes no lock, allocates
//! nothing (an i16 stream's scratch is reserved to the stream's capacity
//! before it starts) and frees nothing (see `mixer.rs` on deferred frees).
//!
//! ## Disconnects (headphones unplugged, Bluetooth gone)
//!
//! AAudio reports them through the error callback, after which the stream
//! is dead. AAudio forbids closing or reopening a stream from inside that
//! callback, so the callback spawns a short-lived thread that closes the
//! dead stream and opens a new one on the new default route, asking for the
//! engine's original sample rate (AAudio resamples if the route differs, so
//! `AudioEngine::sample_rate()` stays true and the mixer's state — playing
//! voices, ramps — carries straight over). It honours a pending
//! `suspend()`, and backs off per `aaudio_policy::reopen_delay`.
//!
//! The error callback's user data is an **id**, not a pointer: AAudio does
//! not promise the error callback can't still be running when `close`
//! returns, so it looks the engine up in a registry of `Weak`s instead of
//! dereferencing memory the engine may already have freed.

// `as i32` / `as i64` casts are identity on aarch64 but not on every
// Android ABI (bindgen enum constants are `c_uint` for some groups,
// `timespec` fields are 32-bit on armv7), so they stay.
#![allow(clippy::unnecessary_cast)]

use std::any::Any;
use std::cell::UnsafeCell;
use std::ffi::{c_char, c_void, CStr};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::Duration;

use ndk_sys::{aaudio_result_t, AAudioStream, AAudioStreamBuilder};

use super::aaudio_policy::{self as policy, SampleFormat, Sharing};
use super::{EngineBackend, OutputDevice};
use crate::mixer::{self, Mixer, MixerEngine};
use crate::EngineError;

// The policy module mirrors these so it can be host-tested; prove the
// mirror matches the NDK headers.
const _: () = {
    assert!(ndk_sys::AAUDIO_FORMAT_PCM_FLOAT as i32 == policy::FORMAT_PCM_FLOAT);
    assert!(ndk_sys::AAUDIO_FORMAT_PCM_I16 as i32 == policy::FORMAT_PCM_I16);
};

/// Tag for logcat lines.
const TAG: &str = "[audio-engine/aaudio]";

// ---------------------------------------------------------------------------
// The dynamically loaded AAudio API.
// ---------------------------------------------------------------------------

type BuilderSetI32 = unsafe extern "C" fn(*mut AAudioStreamBuilder, i32);
type StreamGetI32 = unsafe extern "C" fn(*mut AAudioStream) -> i32;
type StreamCall = unsafe extern "C" fn(*mut AAudioStream) -> aaudio_result_t;

struct Api {
    api_level: Option<u32>,
    create_builder: unsafe extern "C" fn(*mut *mut AAudioStreamBuilder) -> aaudio_result_t,
    set_direction: BuilderSetI32,
    set_performance_mode: BuilderSetI32,
    set_sharing_mode: BuilderSetI32,
    set_format: BuilderSetI32,
    set_channel_count: BuilderSetI32,
    set_sample_rate: BuilderSetI32,
    /// API 28+ (`None` when not resolvable or the level is lower).
    set_usage: Option<BuilderSetI32>,
    set_content_type: Option<BuilderSetI32>,
    set_data_callback: unsafe extern "C" fn(
        *mut AAudioStreamBuilder,
        ndk_sys::AAudioStream_dataCallback,
        *mut c_void,
    ),
    set_error_callback: unsafe extern "C" fn(
        *mut AAudioStreamBuilder,
        ndk_sys::AAudioStream_errorCallback,
        *mut c_void,
    ),
    open_stream:
        unsafe extern "C" fn(*mut AAudioStreamBuilder, *mut *mut AAudioStream) -> aaudio_result_t,
    builder_delete: unsafe extern "C" fn(*mut AAudioStreamBuilder) -> aaudio_result_t,
    close: StreamCall,
    request_start: StreamCall,
    request_pause: StreamCall,
    get_sample_rate: StreamGetI32,
    get_channel_count: StreamGetI32,
    get_format: StreamGetI32,
    get_sharing_mode: StreamGetI32,
    get_performance_mode: StreamGetI32,
    get_frames_per_burst: StreamGetI32,
    get_buffer_capacity: StreamGetI32,
    get_buffer_size: StreamGetI32,
    set_buffer_size: unsafe extern "C" fn(*mut AAudioStream, i32) -> aaudio_result_t,
    get_timestamp:
        unsafe extern "C" fn(*mut AAudioStream, libc::clockid_t, *mut i64, *mut i64) -> aaudio_result_t,
    get_frames_written: unsafe extern "C" fn(*mut AAudioStream) -> i64,
    result_to_text: unsafe extern "C" fn(aaudio_result_t) -> *const c_char,
}

/// Why AAudio could not be loaded.
#[derive(Clone, Debug)]
enum LoadError {
    /// API < 26, or `libaaudio.so` absent.
    Unsupported,
    /// The library loaded but lacks a required entry point.
    Missing(&'static str),
}

static API: OnceLock<Result<Api, LoadError>> = OnceLock::new();

fn api() -> Result<&'static Api, EngineError> {
    match API.get_or_init(load_api) {
        Ok(api) => Ok(api),
        Err(LoadError::Unsupported) => Err(EngineError::Unsupported),
        Err(LoadError::Missing(name)) => Err(EngineError::Backend(format!(
            "libaaudio.so has no `{name}`"
        ))),
    }
}

/// The device's API level from `ro.build.version.sdk` (what the NDK's
/// `android_get_device_api_level` reads; that function itself is only a
/// libc symbol from API 29).
fn device_api_level() -> Option<u32> {
    let mut buf = [0u8; libc::PROP_VALUE_MAX as usize];
    // SAFETY: `buf` is PROP_VALUE_MAX bytes, the documented maximum the
    // property API writes (value + NUL).
    let n = unsafe {
        libc::__system_property_get(c"ro.build.version.sdk".as_ptr(), buf.as_mut_ptr().cast())
    };
    if n <= 0 {
        None
    } else {
        policy::parse_sdk_level(&buf)
    }
}

/// `dlsym` one entry point as a typed function pointer.
///
/// # Safety
/// `T` must be the exact `extern "C"` function-pointer type of `name`.
unsafe fn sym<T: Copy>(lib: *mut c_void, name: &CStr) -> Option<T> {
    debug_assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<*mut c_void>());
    let p = libc::dlsym(lib, name.as_ptr());
    if p.is_null() {
        None
    } else {
        Some(std::mem::transmute_copy::<*mut c_void, T>(&p))
    }
}

fn load_api() -> Result<Api, LoadError> {
    let api_level = device_api_level();
    if !policy::aaudio_allowed(api_level) {
        return Err(LoadError::Unsupported);
    }
    // SAFETY: plain dlopen of a system library by soname.
    let lib = unsafe { libc::dlopen(c"libaaudio.so".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
    if lib.is_null() {
        return Err(LoadError::Unsupported);
    }

    macro_rules! req {
        ($name:literal) => {{
            let cname: &CStr = match CStr::from_bytes_with_nul(concat!($name, "\0").as_bytes()) {
                Ok(c) => c,
                Err(_) => unreachable!(),
            };
            // SAFETY: each field's type below is the NDK header's signature
            // for the named symbol.
            match unsafe { sym(lib, cname) } {
                Some(f) => f,
                None => return Err(LoadError::Missing($name)),
            }
        }};
    }
    macro_rules! opt {
        ($name:literal) => {{
            let cname: &CStr = match CStr::from_bytes_with_nul(concat!($name, "\0").as_bytes()) {
                Ok(c) => c,
                Err(_) => unreachable!(),
            };
            // SAFETY: as for `req!`.
            unsafe { sym(lib, cname) }
        }};
    }

    // `setChannelCount` / `getChannelCount` are the API-26 names;
    // `…SamplesPerFrame` are their older aliases, kept as a fallback.
    let set_channel_count: BuilderSetI32 = match opt!("AAudioStreamBuilder_setChannelCount") {
        Some(f) => f,
        None => req!("AAudioStreamBuilder_setSamplesPerFrame"),
    };
    let get_channel_count: StreamGetI32 = match opt!("AAudioStream_getChannelCount") {
        Some(f) => f,
        None => req!("AAudioStream_getSamplesPerFrame"),
    };
    let set_usage: Option<BuilderSetI32> = opt!("AAudioStreamBuilder_setUsage");
    let set_content_type: Option<BuilderSetI32> = opt!("AAudioStreamBuilder_setContentType");
    let attrs = policy::usage_attrs_allowed(
        api_level,
        set_usage.is_some() && set_content_type.is_some(),
    );

    Ok(Api {
        api_level,
        create_builder: req!("AAudio_createStreamBuilder"),
        set_direction: req!("AAudioStreamBuilder_setDirection"),
        set_performance_mode: req!("AAudioStreamBuilder_setPerformanceMode"),
        set_sharing_mode: req!("AAudioStreamBuilder_setSharingMode"),
        set_format: req!("AAudioStreamBuilder_setFormat"),
        set_channel_count,
        set_sample_rate: req!("AAudioStreamBuilder_setSampleRate"),
        set_usage: set_usage.filter(|_| attrs),
        set_content_type: set_content_type.filter(|_| attrs),
        set_data_callback: req!("AAudioStreamBuilder_setDataCallback"),
        set_error_callback: req!("AAudioStreamBuilder_setErrorCallback"),
        open_stream: req!("AAudioStreamBuilder_openStream"),
        builder_delete: req!("AAudioStreamBuilder_delete"),
        close: req!("AAudioStream_close"),
        request_start: req!("AAudioStream_requestStart"),
        request_pause: req!("AAudioStream_requestPause"),
        get_sample_rate: req!("AAudioStream_getSampleRate"),
        get_channel_count,
        get_format: req!("AAudioStream_getFormat"),
        get_sharing_mode: req!("AAudioStream_getSharingMode"),
        get_performance_mode: req!("AAudioStream_getPerformanceMode"),
        get_frames_per_burst: req!("AAudioStream_getFramesPerBurst"),
        get_buffer_capacity: req!("AAudioStream_getBufferCapacityInFrames"),
        get_buffer_size: req!("AAudioStream_getBufferSizeInFrames"),
        set_buffer_size: req!("AAudioStream_setBufferSizeInFrames"),
        get_timestamp: req!("AAudioStream_getTimestamp"),
        get_frames_written: req!("AAudioStream_getFramesWritten"),
        result_to_text: req!("AAudio_convertResultToText"),
    })
}

impl Api {
    fn text(&self, r: aaudio_result_t) -> String {
        // SAFETY: AAudio returns a static C string for every code.
        let p = unsafe { (self.result_to_text)(r) };
        if p.is_null() {
            format!("AAudio error {r}")
        } else {
            unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
        }
    }
}

// ---------------------------------------------------------------------------
// Engine state.
// ---------------------------------------------------------------------------

/// What the data callback renders with. See the module docs for who may
/// touch it when.
struct Render {
    mixer: Mixer,
    format: SampleFormat,
    channels: u16,
    rate: u32,
    /// Float scratch for an i16 stream, reserved to the stream's capacity
    /// before it starts.
    scratch: Vec<f32>,
}

/// An open AAudio stream handle.
struct StreamPtr(*mut AAudioStream);
// SAFETY: AAudio stream handles may be used from any thread; every use
// here is serialized by `Shared::control`.
unsafe impl Send for StreamPtr {}

struct Control {
    stream: Option<StreamPtr>,
    /// `suspend()` was called (and not undone): a reopened stream stays
    /// paused.
    suspended: bool,
    /// The device was dropped: never open again.
    closed: bool,
    /// The engine's rate (what `AudioEngine::sample_rate` reports);
    /// requested again when reopening after a disconnect.
    sample_rate: u32,
}

struct Shared {
    render: UnsafeCell<Render>,
    control: Mutex<Control>,
    /// A reopen thread is pending; further error callbacks don't spawn
    /// another.
    reopening: AtomicBool,
    /// Registry key; the error callback's user data.
    id: u64,
}

// SAFETY: `control` is a Mutex and `reopening` atomic. `render` is accessed
// by exactly one thread at a time — the data callback of the single open
// stream, or the opener while no stream is open — as the module docs lay
// out; `Render` itself is `Send` (asserted below).
unsafe impl Sync for Shared {}
unsafe impl Send for Shared {}

const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<Render>();
};

impl Shared {
    fn control(&self) -> MutexGuard<'_, Control> {
        // A panic while holding the lock aborts (crash-loud), so poison can
        // only come from a non-FFI caller; the data is still consistent.
        self.control.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Error-callback targets by id (see the module docs for why not a pointer).
static ERROR_TARGETS: Mutex<Vec<(u64, Weak<Shared>)>> = Mutex::new(Vec::new());
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn registry() -> MutexGuard<'static, Vec<(u64, Weak<Shared>)>> {
    ERROR_TARGETS.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---------------------------------------------------------------------------
// Opening.
// ---------------------------------------------------------------------------

/// What an opened (not yet started) stream reports.
struct Opened {
    stream: *mut AAudioStream,
    format: SampleFormat,
    channels: u16,
    rate: u32,
    capacity: i32,
}

/// Deletes the builder on every path.
struct Builder<'a> {
    api: &'a Api,
    ptr: *mut AAudioStreamBuilder,
}

impl Drop for Builder<'_> {
    fn drop(&mut self) {
        // SAFETY: created by AAudio_createStreamBuilder, deleted once.
        unsafe { (self.api.builder_delete)(self.ptr) };
    }
}

/// Open an output stream, walking `policy::OPEN_ATTEMPTS`. `rate` = `None`
/// for the device's native rate. The caller holds `Shared::control` and no
/// other stream of this engine is open.
fn open_stream(api: &Api, shared: &Arc<Shared>, rate: Option<u32>) -> Result<Opened, EngineError> {
    let mut last = String::from("no attempt made");
    for &(sharing, format) in policy::OPEN_ATTEMPTS.iter() {
        match open_one(api, shared, rate, sharing, format) {
            Ok(o) => return Ok(o),
            Err(e) => {
                log::warn!("{TAG} open ({sharing:?}, {format:?}) failed: {e}");
                last = e;
            }
        }
    }
    Err(EngineError::Backend(format!("AAudio openStream: {last}")))
}

fn open_one(
    api: &Api,
    shared: &Arc<Shared>,
    rate: Option<u32>,
    sharing: Sharing,
    format: SampleFormat,
) -> Result<Opened, String> {
    let mut ptr = std::ptr::null_mut();
    // SAFETY: out-pointer to a local.
    let r = unsafe { (api.create_builder)(&mut ptr) };
    if r != ndk_sys::AAUDIO_OK as i32 || ptr.is_null() {
        return Err(format!("createStreamBuilder: {}", api.text(r)));
    }
    let b = Builder { api, ptr };
    let sharing_mode = match sharing {
        Sharing::Exclusive => ndk_sys::AAUDIO_SHARING_MODE_EXCLUSIVE,
        Sharing::Shared => ndk_sys::AAUDIO_SHARING_MODE_SHARED,
    } as i32;
    // SAFETY: valid builder; argument types per the NDK headers. The data
    // callback's user data points into `shared`, which outlives every stream
    // (streams are closed before the device drops its `Arc`, and the reopen
    // thread holds its own). The error callback's user data is an id.
    unsafe {
        (api.set_direction)(b.ptr, ndk_sys::AAUDIO_DIRECTION_OUTPUT as i32);
        (api.set_performance_mode)(b.ptr, ndk_sys::AAUDIO_PERFORMANCE_MODE_LOW_LATENCY as i32);
        (api.set_sharing_mode)(b.ptr, sharing_mode);
        (api.set_format)(b.ptr, format.to_aaudio());
        (api.set_channel_count)(b.ptr, policy::CHANNELS);
        if let Some(rate) = rate {
            (api.set_sample_rate)(b.ptr, rate as i32);
        }
        if let (Some(usage), Some(content)) = (api.set_usage, api.set_content_type) {
            usage(b.ptr, ndk_sys::AAUDIO_USAGE_GAME as i32);
            content(b.ptr, ndk_sys::AAUDIO_CONTENT_TYPE_SONIFICATION as i32);
        }
        (api.set_data_callback)(b.ptr, Some(data_callback), shared.render.get().cast());
        (api.set_error_callback)(b.ptr, Some(error_callback), shared.id as usize as *mut c_void);
    }
    let mut stream = std::ptr::null_mut();
    // SAFETY: valid builder, out-pointer to a local.
    let r = unsafe { (api.open_stream)(b.ptr, &mut stream) };
    drop(b);
    if r != ndk_sys::AAUDIO_OK as i32 || stream.is_null() {
        return Err(api.text(r));
    }

    // SAFETY: `stream` is open; getters are plain reads.
    let (got_format, channels, got_rate, burst, capacity, got_sharing, perf) = unsafe {
        (
            (api.get_format)(stream),
            (api.get_channel_count)(stream),
            (api.get_sample_rate)(stream),
            (api.get_frames_per_burst)(stream),
            (api.get_buffer_capacity)(stream),
            (api.get_sharing_mode)(stream),
            (api.get_performance_mode)(stream),
        )
    };
    let close = |why: String| {
        // SAFETY: open, never started, closed once.
        unsafe { (api.close)(stream) };
        Err(why)
    };
    let Some(format) = SampleFormat::from_aaudio(got_format) else {
        return close(format!("stream came back in format {got_format}"));
    };
    if channels <= 0 || got_rate <= 0 {
        return close(format!("stream reports {channels} channels at {got_rate} Hz"));
    }
    if let Some(frames) = policy::target_buffer_frames(burst, capacity) {
        // SAFETY: open stream. Returns the size actually set (or an error,
        // which leaves AAudio's default — still a working stream).
        let r = unsafe { (api.set_buffer_size)(stream, frames) };
        if r < 0 {
            log::warn!("{TAG} setBufferSizeInFrames({frames}): {}", api.text(r));
        }
    }
    log::info!(
        "{TAG} opened: {got_rate} Hz, {channels} ch, {format:?}, sharing {}, perf mode {perf}, burst {burst}, capacity {capacity}",
        if got_sharing == ndk_sys::AAUDIO_SHARING_MODE_EXCLUSIVE as i32 { "exclusive" } else { "shared" },
    );
    Ok(Opened {
        stream,
        format,
        channels: channels as u16,
        rate: got_rate as u32,
        capacity,
    })
}

/// Point the render state at a freshly opened stream.
///
/// # Safety
/// No stream of this engine may be started (the caller holds `control`
/// and has just opened `o`, the only stream, and not yet started it).
unsafe fn configure_render(shared: &Shared, o: &Opened) {
    let r = &mut *shared.render.get();
    r.format = o.format;
    r.channels = o.channels;
    r.rate = o.rate;
    if o.format == SampleFormat::I16 {
        // Callbacks never exceed the capacity: reserve it now, off the
        // audio thread.
        let need = (o.capacity.max(0) as usize).max(4096) * o.channels as usize;
        r.scratch.reserve(need.saturating_sub(r.scratch.len()));
    }
}

// ---------------------------------------------------------------------------
// Callbacks. Both are `extern "C"` entry points called by AAudio threads:
// a panic must never unwind into C. Crash-loud policy: catch, log to
// logcat, abort.
// ---------------------------------------------------------------------------

fn abort_on_panic(site: &str, payload: Box<dyn Any + Send>) -> ! {
    let msg = payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "<non-string panic payload>".to_owned());
    log::error!("{TAG} panic in {site}: {msg}");
    std::process::abort()
}

unsafe extern "C" fn data_callback(
    _stream: *mut AAudioStream,
    user: *mut c_void,
    audio: *mut c_void,
    num_frames: i32,
) -> ndk_sys::aaudio_data_callback_result_t {
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: `user` is `Shared::render` of the engine that opened this
        // stream; only this callback touches it while the stream runs.
        let r = unsafe { &mut *(user as *mut Render) };
        // SAFETY: AAudio hands `num_frames` frames of the stream's format
        // and channel count.
        unsafe { render_into(r, audio, num_frames) };
    }));
    if let Err(payload) = result {
        abort_on_panic("AAudio data callback", payload);
    }
    ndk_sys::AAUDIO_CALLBACK_RESULT_CONTINUE as i32
}

/// # Safety
/// `audio` holds `num_frames × r.channels` samples of `r.format`.
unsafe fn render_into(r: &mut Render, audio: *mut c_void, num_frames: i32) {
    let n = num_frames.max(0) as usize * r.channels as usize;
    if n == 0 || audio.is_null() {
        return;
    }
    match r.format {
        SampleFormat::F32 => {
            let out = std::slice::from_raw_parts_mut(audio as *mut f32, n);
            r.mixer.render(out, r.channels, r.rate);
        }
        SampleFormat::I16 => {
            if r.scratch.len() < n {
                // Within the reserved capacity: no allocation.
                r.scratch.resize(n, 0.0);
            }
            let scratch = &mut r.scratch[..n];
            r.mixer.render(scratch, r.channels, r.rate);
            let out = std::slice::from_raw_parts_mut(audio as *mut i16, n);
            for (d, s) in out.iter_mut().zip(scratch.iter()) {
                *d = policy::f32_to_i16(*s);
            }
        }
    }
}

unsafe extern "C" fn error_callback(
    stream: *mut AAudioStream,
    user: *mut c_void,
    error: aaudio_result_t,
) {
    let result = catch_unwind(AssertUnwindSafe(|| {
        on_stream_error(stream as usize, user as usize as u64, error)
    }));
    if let Err(payload) = result {
        abort_on_panic("AAudio error callback", payload);
    }
}

fn on_stream_error(stream: usize, id: u64, error: aaudio_result_t) {
    let text = api().map(|a| a.text(error)).unwrap_or_default();
    log::warn!("{TAG} stream error {error} ({text}); reopening");
    let target = registry()
        .iter()
        .find(|(i, _)| *i == id)
        .and_then(|(_, w)| w.upgrade());
    let Some(shared) = target else { return };
    if shared.reopening.swap(true, Ordering::AcqRel) {
        return;
    }
    let keep = shared.clone();
    // AAudio: "do not close or reopen the stream from the error callback" —
    // hand it to a thread of our own.
    let spawned = std::thread::Builder::new()
        .name("audio-engine-reopen".into())
        .spawn(move || reopen(shared, stream));
    if let Err(e) = spawned {
        log::error!("{TAG} could not spawn the reopen thread: {e}");
        keep.reopening.store(false, Ordering::Release);
    }
}

/// Replace the dead stream `failed` with a new one on the current route.
fn reopen(shared: Arc<Shared>, failed: usize) {
    let Ok(api) = api() else { return };
    let mut ctl = shared.control();
    let current = ctl.stream.as_ref().map(|s| s.0 as usize);
    if ctl.closed || current != Some(failed) {
        // Torn down, or a stale report for a stream already replaced.
        shared.reopening.store(false, Ordering::Release);
        return;
    }
    if let Some(old) = ctl.stream.take() {
        // SAFETY: the dead stream; closing waits out its data callback.
        unsafe { (api.close)(old.0) };
    }
    let rate = ctl.sample_rate;
    let mut attempt = 0;
    loop {
        let Some(delay) = policy::reopen_delay(attempt) else {
            log::error!("{TAG} giving up reopening the output after {attempt} attempts");
            break;
        };
        if !delay.is_zero() {
            drop(ctl);
            std::thread::sleep(delay);
            ctl = shared.control();
        }
        if ctl.closed {
            break;
        }
        match open_stream(api, &shared, Some(rate)) {
            Ok(o) => {
                // SAFETY: the old stream is closed and `o` not started.
                unsafe { configure_render(&shared, &o) };
                if !ctl.suspended {
                    // SAFETY: open stream.
                    let r = unsafe { (api.request_start)(o.stream) };
                    if r != ndk_sys::AAUDIO_OK as i32 {
                        log::error!("{TAG} requestStart after reopen: {}", api.text(r));
                    }
                }
                ctl.stream = Some(StreamPtr(o.stream));
                break;
            }
            Err(e) => {
                log::warn!("{TAG} reopen attempt {attempt}: {e}");
                attempt += 1;
            }
        }
    }
    shared.reopening.store(false, Ordering::Release);
}

// ---------------------------------------------------------------------------
// The device.
// ---------------------------------------------------------------------------

struct AAudioDevice {
    shared: Arc<Shared>,
}

fn monotonic_now_ns() -> Option<i64> {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: out-pointer to a local.
    let r = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    (r == 0).then(|| ts.tv_sec as i64 * 1_000_000_000 + ts.tv_nsec as i64)
}

impl OutputDevice for AAudioDevice {
    /// From the presentation timestamp while running
    /// (`AAudioStream_getTimestamp`, CLOCK_MONOTONIC), else the buffer size.
    fn output_latency(&self) -> Option<Duration> {
        let api = api().ok()?;
        let ctl = self.shared.control();
        let s = ctl.stream.as_ref()?.0;
        let rate = ctl.sample_rate;
        let (mut pos, mut time) = (0i64, 0i64);
        // SAFETY: open stream; out-pointers to locals.
        let r = unsafe { (api.get_timestamp)(s, libc::CLOCK_MONOTONIC, &mut pos, &mut time) };
        if r == ndk_sys::AAUDIO_OK as i32 {
            // SAFETY: open stream.
            let written = unsafe { (api.get_frames_written)(s) };
            if let Some(now) = monotonic_now_ns() {
                if let Some(d) = policy::latency_from_timestamp(written, pos, time, now, rate) {
                    return Some(d);
                }
            }
        }
        // SAFETY: open stream.
        policy::latency_from_buffer(unsafe { (api.get_buffer_size)(s) }, rate)
    }

    fn suspend(&self) {
        let Ok(api) = api() else { return };
        let mut ctl = self.shared.control();
        ctl.suspended = true;
        if let Some(s) = &ctl.stream {
            // SAFETY: open stream.
            let r = unsafe { (api.request_pause)(s.0) };
            if r != ndk_sys::AAUDIO_OK as i32 {
                log::warn!("{TAG} requestPause: {}", api.text(r));
            }
        }
    }

    fn resume(&self) {
        let Ok(api) = api() else { return };
        let mut ctl = self.shared.control();
        ctl.suspended = false;
        if let Some(s) = &ctl.stream {
            // SAFETY: open stream.
            let r = unsafe { (api.request_start)(s.0) };
            if r != ndk_sys::AAUDIO_OK as i32 {
                log::warn!("{TAG} requestStart: {}", api.text(r));
            }
        }
    }
}

impl Drop for AAudioDevice {
    /// Close the stream (which waits out the data callback), then forget
    /// the error-callback route. A reopen thread that is mid-back-off sees
    /// `closed` and exits; the mixer drops with the last `Arc<Shared>`.
    fn drop(&mut self) {
        {
            let mut ctl = self.shared.control();
            ctl.closed = true;
            if let (Some(s), Ok(api)) = (ctl.stream.take(), api()) {
                // SAFETY: open stream, closed once.
                unsafe { (api.close)(s.0) };
            }
        }
        let id = self.shared.id;
        registry().retain(|(i, _)| *i != id);
    }
}

pub(crate) async fn start() -> Result<Rc<dyn EngineBackend>, EngineError> {
    let api = api()?;
    log::info!("{TAG} AAudio loaded (API level {:?})", api.api_level);

    let queue = mixer::new_queue();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let shared = Arc::new(Shared {
        // Placeholder rate; replaced below once the stream reports its own.
        render: UnsafeCell::new(Render {
            mixer: Mixer::new(queue.clone(), 48_000),
            format: SampleFormat::F32,
            channels: policy::CHANNELS as u16,
            rate: 48_000,
            scratch: Vec::new(),
        }),
        control: Mutex::new(Control {
            stream: None,
            suspended: false,
            closed: false,
            sample_rate: 0,
        }),
        reopening: AtomicBool::new(false),
        id,
    });
    registry().push((id, Arc::downgrade(&shared)));

    let device = AAudioDevice {
        shared: shared.clone(),
    };
    // From here `device`'s Drop closes whatever was opened and unregisters.
    let rate = {
        let mut ctl = shared.control();
        let o = open_stream(api, &shared, None)?;
        // SAFETY: `o` is the only stream and is not started.
        unsafe {
            configure_render(&shared, &o);
            (*shared.render.get()).mixer = Mixer::new(queue.clone(), o.rate);
        }
        ctl.sample_rate = o.rate;
        ctl.stream = Some(StreamPtr(o.stream));
        // SAFETY: open stream.
        let r = unsafe { (api.request_start)(o.stream) };
        if r != ndk_sys::AAUDIO_OK as i32 {
            return Err(EngineError::Backend(format!(
                "AAudio requestStart: {}",
                api.text(r)
            )));
        }
        o.rate
    };
    drop(shared);
    Ok(Rc::new(MixerEngine::new(queue, rate, Box::new(device))))
}
