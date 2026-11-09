//! A platform-agnostic handle to a live video source.
//!
//! `MediaStream` is the common currency between capture SDKs that *produce*
//! video (`camera`, `screen-recorder`) and the layers that *consume* it
//! (a display component, a GPU compositor). A developer wires `camera ->
//! video` and never names a platform type — the per-platform transport
//! (a web `MediaStream`, an Apple `CVPixelBuffer`, an Android preview
//! `Surface`) is hidden.
//!
//! A stream exposes two access paths over one source:
//!
//! - **CPU frame tap** — [`MediaStream::subscribe`] delivers normalized
//!   `RGBA8` [`VideoFrame`]s on a callback (analysis, a
//!   lowest-common-denominator display, a fallback), and
//!   [`MediaStream::latest`] polls the most recent frame. A producer that can
//!   display without them makes these frames only on demand: see
//!   [`FrameWriter::wants_cpu_frames`].
//! - **Native source** — [`MediaStream::native_source`] returns an opaque
//!   `Rc<dyn Any>` that a *same-platform* display or GPU layer downcasts to
//!   the platform's zero-copy frame source. Authors never touch it.
//!
//! This crate is **thin and GPU-free** on purpose: a real-time GPU
//! compositor is a layer on top that consumes a `MediaStream` (importing
//! frames as textures, zero-copy via [`native_source`](MediaStream::native_source),
//! or via the CPU tap as a fallback). The only guarantee here is that such
//! a consumer is *possible*.
//!
//! # Producing a stream
//!
//! A capture backend builds both halves with [`MediaStream::new`], hands
//! the `Send` [`FrameWriter`] to its capture thread, and attaches a stopper
//! that tears capture down when the last `MediaStream` clone drops:
//!
//! ```no_run
//! use media_stream::MediaStream;
//! # fn spawn_capture(_w: media_stream::FrameWriter) -> Box<dyn FnOnce()> { Box::new(|| {}) }
//! let (stream, writer) = MediaStream::new();
//! let stop = spawn_capture(writer);       // capture thread calls writer.write_rgba8(..)
//! stream.attach_stopper(move || stop());
//! // hand `stream` to the consumer; dropping it stops capture.
//! ```

#![deny(missing_docs)]

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

pub mod clock;

/// Zero-copy Apple frame source shared between a producer (`screen-recorder` /
/// `camera`) and a same-platform display consumer (`video`). Carries an opaque
/// retained Apple frame handle for the native fast-path so display never
/// round-trips pixels through the CPU `RGBA8` channel:
///
/// - **macOS** → an `IOSurface` (`CVPixelBufferGetIOSurface`), set directly as
///   `CALayer.contents`.
/// - **iOS** → a `CMSampleBuffer`, enqueued into an `AVSampleBufferDisplayLayer`
///   (iOS `CALayer.contents` doesn't accept an `IOSurface`).
///
/// Both are CoreFoundation types managed by the same `CFRetain`/`CFRelease`
/// dance, so one channel type serves both — each platform's consumer
/// interprets the handle it knows its producer publishes.
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub mod apple_surface;
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub use apple_surface::{surface_channel, NativeTap, SurfaceSource, SurfaceWriter};

/// Zero-copy Android display: Camera2 pushes frames into consumer `Surface`s,
/// so this native source is a control object a display hands its `Surface` to,
/// not a frame handle. Compiled for tests on every host, like the camera's pure
/// Android modules, because its claim rules need no device.
#[cfg(any(test, target_os = "android"))]
pub mod android_preview;
#[cfg(any(test, target_os = "android"))]
pub use android_preview::{
    preview_channel, PreviewClaim, PreviewDriver, PreviewGeometry, PreviewProducer, PreviewSource,
};

/// Zero-copy Linux frame source: a **dma-buf** handle (a GPU buffer exported from
/// the canvas render target via `EGL_MESA_image_dma_buf_export`) shared between a
/// GPU producer (`canvas-vello`) and a same-platform consumer (a recorder). The
/// Linux analog of [`apple_surface`] — `MediaStream::native_source` carries a
/// [`dmabuf::DmaBufSource`] the recorder downcasts, so a GPU-rendered frame stays
/// a GPU handle through the stream and only touches the CPU if a software encoder
/// demands it.
#[cfg(target_os = "linux")]
pub mod dmabuf;
#[cfg(target_os = "linux")]
pub use dmabuf::{
    dmabuf_channel, DmaBufFrame, DmaBufSource, DmaBufWriter, NativeTap, DRM_FORMAT_MOD_INVALID,
};

mod audio;
pub use audio::{
    AudioFormat, AudioFrame, AudioFrameCallback, AudioStream, AudioSubscription, AudioWriter,
};

// Web-only: lazily back a synthetic `AudioStream` with a real `MediaStreamTrack`
// so browser sinks (recorder, playback) can consume it — see the module docs.
#[cfg(target_arch = "wasm32")]
mod audio_web_bridge;

// ---------------------------------------------------------------------------
// Frame types — the normalized currency every producer emits.
// ---------------------------------------------------------------------------

/// Pixel layout of a [`VideoFrame`]. There's one variant today — every
/// producer normalizes to it — but it's named (not assumed) so a future
/// zero-copy planar path can be added without breaking the field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PixelFormat {
    /// 8 bits per channel, byte order `R G B A`, straight (non-premultiplied)
    /// alpha. Alpha is `255` for opaque camera/screen frames.
    Rgba8,
}

/// One captured frame, borrowed for the duration of a [`subscribe`] callback
/// (or copied out of [`latest`]). Pixels are **tightly-packed `RGBA8`**
/// (`data.len() == width * height * 4`), top-down. Every backend converts
/// its native layout into this one shape, so consumer code is identical
/// everywhere.
///
/// [`subscribe`]: MediaStream::subscribe
/// [`latest`]: MediaStream::latest
pub struct VideoFrame<'a> {
    /// Tightly-packed `RGBA8` pixels, top-down.
    pub data: &'a [u8],
    /// Frame width in pixels (authoritative — what the device produced).
    pub width: u32,
    /// Frame height in pixels (authoritative).
    pub height: u32,
    /// Pixel layout of `data`. Always [`PixelFormat::Rgba8`] today.
    pub format: PixelFormat,
    /// Capture timestamp on the shared [`clock`] timeline, in microseconds.
    /// A muxer uses this to place the frame on the file's presentation
    /// timeline and lip-sync it against audio captured from another source.
    /// `write_rgba8`/`write_bgra8` stamp it with [`clock::now_micros`]; the
    /// `*_at` variants let a producer that already has a hardware capture
    /// timestamp supply its own.
    pub pts_micros: u64,
}

impl VideoFrame<'_> {
    /// Number of pixels (`width * height`).
    pub fn pixel_count(&self) -> usize {
        self.width as usize * self.height as usize
    }

    /// Expected `data` length in bytes (`width * height * 4`).
    pub fn byte_len(&self) -> usize {
        self.pixel_count() * 4
    }
}

// ---------------------------------------------------------------------------
// The callback bound. Mirrors `microphone`/`camera`: producers deliver
// frames on a capture thread on native/Android (so `Send`) and on the main
// thread on web (so not `Send`).
// ---------------------------------------------------------------------------

/// The bound a [`subscribe`](MediaStream::subscribe) callback must satisfy.
/// Implemented automatically for any matching closure. `Send` on native /
/// Android (the callback runs on a capture thread), not on web (main
/// thread). `FnMut`, so it may own and mutate state across frames.
#[cfg(not(target_arch = "wasm32"))]
pub trait FrameCallback: FnMut(&VideoFrame) + Send + 'static {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: FnMut(&VideoFrame) + Send + 'static> FrameCallback for T {}

/// See the non-wasm definition; on web the `Send` bound is dropped.
#[cfg(target_arch = "wasm32")]
pub trait FrameCallback: FnMut(&VideoFrame) + 'static {}
#[cfg(target_arch = "wasm32")]
impl<T: FnMut(&VideoFrame) + 'static> FrameCallback for T {}

#[cfg(not(target_arch = "wasm32"))]
type BoxedCallback = Box<dyn FnMut(&VideoFrame) + Send + 'static>;
#[cfg(target_arch = "wasm32")]
type BoxedCallback = Box<dyn FnMut(&VideoFrame) + 'static>;

// ---------------------------------------------------------------------------
// FrameChannel — the Send half. The capture thread writes; consumers read
// (latest-frame pull) or are fanned out to (subscribe push).
// ---------------------------------------------------------------------------

struct OwnedFrame {
    width: u32,
    height: u32,
    /// Tightly-packed `RGBA8`, `width * height * 4` bytes.
    rgba: Vec<u8>,
    /// Capture timestamp on the shared [`clock`] timeline, in microseconds.
    pts_micros: u64,
}

/// How long one [`MediaStream::latest`] keeps CPU frames coming, in
/// microseconds. Long enough to cover a producer waking its CPU path (a Camera2
/// repeating request takes several frames to change) and a caller polling
/// slower than the display; short enough that a one-off pull does not leave the
/// conversion running.
const PULL_LEASE_MICROS: u64 = 2_000_000;

/// Fires when CPU demand changes. See [`FrameWriter::on_cpu_demand_change`].
type DemandCallback = Box<dyn Fn(bool) + Send + 'static>;

/// What the producer's demand callback was last told.
#[derive(Default)]
struct DemandWatch {
    reported: bool,
    callback: Option<DemandCallback>,
}

#[derive(Default)]
struct FrameChannel {
    /// Most recent frame, for pull consumers (a GPU blit samples this).
    latest: Mutex<Option<OwnedFrame>>,
    /// Bumped on every write; pull consumers compare it to skip work.
    generation: AtomicU64,
    /// Push consumers, keyed by id so a dropped `Subscription` removes itself.
    subscribers: Mutex<Vec<(u64, BoxedCallback)>>,
    next_sub_id: AtomicU64,
    /// Live subscriber count, mirrored from `subscribers` for a lock-free read.
    /// A producer checks this (via [`FrameWriter::wants_cpu_frames`]) to skip
    /// the per-frame CPU normalization (e.g. the screen-recorder's BGRA→RGBA
    /// swizzle) when nobody is tapping CPU frames — pure native-source display
    /// reads the zero-copy handle instead and never needs the RGBA channel.
    subscriber_count: AtomicUsize,
    /// [`clock::now_micros`] deadline of the last pull: until then a
    /// [`MediaStream::latest`] caller counts as wanting CPU frames. 0 = no pull
    /// yet, which the clock has already passed.
    pull_until: AtomicU64,
    /// Producer-side demand notification. Absent for a producer that writes
    /// regardless of demand or polls [`FrameWriter::wants_cpu_frames`].
    demand: Mutex<DemandWatch>,
    /// The producer declared it will write no more
    /// ([`FrameWriter::end_of_stream`]). Later subscribers are dropped at once.
    ended: AtomicBool,
}

impl FrameChannel {
    // Single-producer: frames arrive from one capture source. We take the
    // previous frame's buffer out to reuse its allocation, fill it, fan out
    // to push subscribers WITHOUT holding the `latest` lock (so a subscriber
    // may safely call `latest()`), then store it back as the new latest. `fill`
    // gets the buffer as the last frame left it and must replace its contents.
    fn write(&self, width: u32, height: u32, pts_micros: u64, fill: impl FnOnce(&mut Vec<u8>)) {
        if width == 0 || height == 0 {
            return;
        }
        let mut buf = self
            .latest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .map(|f| f.rgba)
            .unwrap_or_default();
        fill(&mut buf);

        // Fan out (no `latest` lock held).
        {
            let view = VideoFrame {
                data: &buf,
                width,
                height,
                format: PixelFormat::Rgba8,
                pts_micros,
            };
            let mut subs = self.subscribers.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            for (_, cb) in subs.iter_mut() {
                cb(&view);
            }
        }

        *self.latest.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(OwnedFrame {
            width,
            height,
            rgba: buf,
            pts_micros,
        });
        self.generation.fetch_add(1, Ordering::Release);
        // A pull's lease lapses silently, so the frames themselves are what
        // notice: the first one written after the deadline turns demand off.
        self.sync_demand();
    }

    fn add_subscriber(&self, cb: BoxedCallback) -> u64 {
        let id = self.next_sub_id.fetch_add(1, Ordering::Relaxed);
        if self.ended.load(Ordering::Acquire) {
            // Dropping `cb` releases whatever waits on it, as `end_of_stream`
            // does for the subscribers it finds.
            return id;
        }
        {
            let mut subs = self.subscribers.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            subs.push((id, cb));
            self.subscriber_count.store(subs.len(), Ordering::Release);
        }
        self.sync_demand();
        id
    }

    fn remove_subscriber(&self, id: u64) {
        {
            let mut subs = self.subscribers.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            subs.retain(|(i, _)| *i != id);
            self.subscriber_count.store(subs.len(), Ordering::Release);
        }
        self.sync_demand();
    }

    fn wants_cpu(&self) -> bool {
        self.subscriber_count.load(Ordering::Acquire) > 0
            || clock::now_micros() < self.pull_until.load(Ordering::Acquire)
    }

    /// Tell the producer's callback about a change in demand, in order. The
    /// retained frame goes on either edge: a producer gated on demand writes
    /// nothing while idle, so that frame shows a moment long gone, and a pull must
    /// wait for a new one rather than read it as current. Falling, this also gives
    /// a frame's worth of memory back while only a native display is running.
    fn sync_demand(&self) {
        let wants = self.wants_cpu();
        let mut watch = self.demand.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if watch.reported == wants {
            return;
        }
        watch.reported = wants;
        let Some(callback) = &watch.callback else {
            return;
        };
        self.latest.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
        callback(wants);
    }

    /// A pull: extend the lease, and say whether the retained frame may be read.
    /// A producer gated on demand holds nothing current unless demand was
    /// already up; every other producer's last write is its current frame.
    fn pull(&self) -> bool {
        let was_wanted = self.wants_cpu();
        self.pull_until
            .store(clock::now_micros() + PULL_LEASE_MICROS, Ordering::Release);
        self.sync_demand();
        let gated = self
            .demand
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .callback
            .is_some();
        was_wanted || !gated
    }
}

// ---------------------------------------------------------------------------
// FrameWriter — the producer's `Send` push handle (held by the capture
// thread). Decoupled from the `!Send` `MediaStream` handle so the capture
// thread never touches the native source.
// ---------------------------------------------------------------------------

/// The producer side of a [`MediaStream`]: the capture thread pushes frames
/// through it. `Send` + cheap to clone.
#[derive(Clone)]
pub struct FrameWriter {
    channel: Arc<FrameChannel>,
    /// macOS zero-copy capture sink: present only when the pair was built via
    /// [`MediaStream::with_surface_capture`]. The GPU producer publishes each
    /// rendered IOSurface here; the stream's `native_source` is the paired
    /// [`SurfaceSource`].
    #[cfg(target_os = "macos")]
    surface: Option<apple_surface::SurfaceWriter>,
    /// Linux zero-copy capture sink: present only when the pair was built via
    /// [`MediaStream::with_surface_capture`]. The GPU producer publishes each
    /// rendered frame's exported dma-buf descriptor here; the stream's
    /// `native_source` is the paired [`dmabuf::DmaBufSource`].
    #[cfg(target_os = "linux")]
    dmabuf: Option<dmabuf::DmaBufWriter>,
    /// Web self-capture: the shared `native` slot of the paired stream, so the
    /// canvas renderer can publish its `captureStream()` as the native source.
    /// Present only when built via [`MediaStream::with_surface_capture`].
    #[cfg(target_arch = "wasm32")]
    native_slot: Option<Rc<RefCell<Option<Rc<dyn Any>>>>>,
}

impl FrameWriter {
    fn from_channel(channel: Arc<FrameChannel>) -> Self {
        FrameWriter {
            channel,
            #[cfg(target_os = "macos")]
            surface: None,
            #[cfg(target_os = "linux")]
            dmabuf: None,
            #[cfg(target_arch = "wasm32")]
            native_slot: None,
        }
    }

    /// Publish a platform native source (e.g. the web canvas's `captureStream()`
    /// `web_glue::dom::MediaStream`) as the paired stream's
    /// [`native_source`](MediaStream::native_source). No-op unless the pair was
    /// built via [`MediaStream::with_surface_capture`]. Web self-capture seam.
    #[cfg(target_arch = "wasm32")]
    pub fn publish_native_source(&self, src: Rc<dyn Any>) {
        if let Some(slot) = &self.native_slot {
            *slot.borrow_mut() = Some(src);
        }
    }

    /// Whether a native-surface consumer (a recorder) is currently tapping the
    /// zero-copy path (holds a [`NativeTap`]). A GPU producer gates its
    /// per-frame IOSurface blit + [`publish_surface`](Self::publish_surface) on
    /// this — the native analogue of [`wants_cpu_frames`](Self::wants_cpu_frames).
    /// Always false unless built via [`MediaStream::with_surface_capture`].
    pub fn wants_native(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.surface.as_ref().is_some_and(|s| s.wants_surface())
        }
        #[cfg(target_os = "linux")]
        {
            self.dmabuf.as_ref().is_some_and(|d| d.wants())
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            false
        }
    }

    /// Publish a rendered frame's exported dma-buf descriptor to the native source
    /// — the Linux GPU self-capture seam (the analogue of macOS's
    /// [`publish_surface`](Self::publish_surface)). No-op if this writer wasn't
    /// built with a dma-buf sink. The producer owns the fd (its render ring); this
    /// only records the borrowed descriptor. A consumer reads it via the stream's
    /// [`DmaBufSource`](dmabuf::DmaBufSource) and `dup(2)`s the fd on import.
    #[cfg(target_os = "linux")]
    pub fn publish_dmabuf(&self, frame: dmabuf::DmaBufFrame) {
        if let Some(d) = &self.dmabuf {
            d.publish(frame);
        }
    }

    /// Publish a rendered `IOSurface` (raw `IOSurfaceRef`) to the native source
    /// — the macOS GPU self-capture seam. No-op if this writer wasn't built with
    /// a surface sink. Publishing retains the surface for the slot; a consumer
    /// reads it via the stream's [`SurfaceSource`].
    ///
    /// # Safety
    /// `surface` must be a valid `IOSurfaceRef` (or null, ignored).
    #[cfg(target_os = "macos")]
    pub unsafe fn publish_surface(&self, surface: *const std::ffi::c_void) {
        if let Some(s) = &self.surface {
            s.publish(surface);
        }
    }

    /// Whether any consumer currently wants CPU frames: an active
    /// [`MediaStream::subscribe`], or a [`MediaStream::latest`] in the last two
    /// seconds. A producer that can also publish a zero-copy native source (the
    /// Apple `SurfaceWriter`, the Android preview) uses this to skip per-frame
    /// CPU normalization — e.g. the screen-recorder's full-frame BGRA→RGBA
    /// swizzle — when only a native-source display is attached.
    ///
    /// A poll, for a producer that wakes per frame anyway. A producer with no
    /// frame callback to poll from uses [`on_cpu_demand_change`](Self::on_cpu_demand_change).
    pub fn wants_cpu_frames(&self) -> bool {
        self.channel.wants_cpu()
    }

    /// Be told when [`wants_cpu_frames`](Self::wants_cpu_frames) changes, for a
    /// producer whose CPU path has to be switched on and off: Camera2 delivers
    /// no frame to poll from while its `ImageReader` is not a target of the
    /// repeating request. `callback` runs with the new value on whichever thread
    /// changed it (a subscriber's, a puller's, or the capture thread when a pull
    /// lapses), in order. It must return quickly and must not touch this stream.
    /// If demand is already up it fires once now.
    ///
    /// Registering also declares this producer writes only on demand, so a
    /// retained frame is never read as current after an idle spell: it is dropped
    /// when demand changes, and [`MediaStream::latest`] returns `None` until a new
    /// frame arrives.
    pub fn on_cpu_demand_change(&self, callback: impl Fn(bool) + Send + 'static) {
        let wants = self.channel.wants_cpu();
        let mut watch = self
            .channel
            .demand
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if wants {
            callback(true);
        }
        watch.reported = wants;
        watch.callback = Some(Box::new(callback));
    }

    /// Declare that this writer will write no more frames: the device is gone,
    /// not merely idle. Every subscriber is dropped, and with it anything waiting
    /// on one, so a pending [`MediaStream::screenshot`] resolves `None` instead
    /// of never. The stream's [`latest`](MediaStream::latest) still reads whatever
    /// was retained.
    pub fn end_of_stream(&self) {
        self.channel.ended.store(true, Ordering::Release);
        let dropped = {
            let mut subs = self
                .channel
                .subscribers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.channel.subscriber_count.store(0, Ordering::Release);
            std::mem::take(&mut *subs)
        };
        // Outside the lock: a subscriber's captures may run code on drop.
        drop(dropped);
        self.channel.sync_demand();
    }

    /// Push a tightly-packed top-down `RGBA8` frame, stamped with the current
    /// [`clock::now_micros`]. Frames shorter than `width * height * 4` are
    /// ignored.
    pub fn write_rgba8(&self, width: u32, height: u32, data: &[u8]) {
        self.write_rgba8_at(width, height, data, clock::now_micros());
    }

    /// Like [`write_rgba8`](Self::write_rgba8) but with an explicit capture
    /// timestamp (microseconds on the shared [`clock`] timeline). Use this
    /// when the producer has a real hardware presentation timestamp for the
    /// frame, so a muxer sees the true capture cadence rather than the moment
    /// the byte copy happened.
    pub fn write_rgba8_at(&self, width: u32, height: u32, data: &[u8], pts_micros: u64) {
        let need = match checked_len(width, height, data) {
            Some(n) => n,
            None => return,
        };
        self.channel.write(width, height, pts_micros, |buf| {
            buf.clear();
            buf.extend_from_slice(&data[..need]);
        });
    }

    /// Push a frame by writing it directly into the channel's reused buffer,
    /// stamped with the current [`clock::now_micros`]. `fill` gets exactly
    /// `width * height * 4` bytes, holding whatever the previous frame left, and
    /// must write every one as tightly-packed top-down `RGBA8`. A zero dimension
    /// is ignored and `fill` is not called.
    ///
    /// For a producer that converts pixels anyway: converting into this buffer
    /// costs one pass, where [`write_rgba8`](Self::write_rgba8) would copy the
    /// finished frame a second time. Decide whether the source frame is good
    /// before calling; the frame is published when `fill` returns.
    pub fn write_rgba8_with(&self, width: u32, height: u32, fill: impl FnOnce(&mut [u8])) {
        let need = width as usize * height as usize * 4;
        self.channel.write(width, height, clock::now_micros(), |buf| {
            // Not `clear` first: that would zero the whole frame just to overwrite it.
            buf.resize(need, 0);
            fill(buf);
        });
    }

    /// Push a tightly-packed top-down `BGRA8` frame (Apple / Windows layout);
    /// channels are swizzled to `RGBA8`. Stamped with [`clock::now_micros`].
    /// Frames shorter than `width * height * 4` are ignored.
    pub fn write_bgra8(&self, width: u32, height: u32, data: &[u8]) {
        self.write_bgra8_at(width, height, data, clock::now_micros());
    }

    /// Like [`write_bgra8`](Self::write_bgra8) but with an explicit capture
    /// timestamp (microseconds on the shared [`clock`] timeline).
    pub fn write_bgra8_at(&self, width: u32, height: u32, data: &[u8], pts_micros: u64) {
        let need = match checked_len(width, height, data) {
            Some(n) => n,
            None => return,
        };
        self.channel.write(width, height, pts_micros, |buf| {
            buf.clear();
            buf.reserve(need);
            for px in data[..need].chunks_exact(4) {
                buf.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
            }
        });
    }
}

fn checked_len(width: u32, height: u32, data: &[u8]) -> Option<usize> {
    if width == 0 || height == 0 {
        return None;
    }
    let need = width as usize * height as usize * 4;
    (data.len() >= need).then_some(need)
}

// ---------------------------------------------------------------------------
// MediaStream — the !Send, main-thread consumer handle.
// ---------------------------------------------------------------------------

struct StreamInner {
    channel: Arc<FrameChannel>,
    /// The platform's zero-copy frame source (web `MediaStream`, etc.),
    /// type-erased. Set by the producer, downcast by a same-platform
    /// display / GPU consumer. An `Rc` so a `with_surface_capture` producer
    /// half (the `FrameWriter`) can share + set it AFTER construction — e.g. the
    /// web canvas publishes its `captureStream()` once the `<canvas>` exists.
    native: Rc<RefCell<Option<Rc<dyn Any>>>>,
    /// Runs when the last clone drops — tears capture down.
    stopper: RefCell<Option<Box<dyn FnOnce()>>>,
}

impl Drop for StreamInner {
    fn drop(&mut self) {
        if let Some(stop) = self.stopper.borrow_mut().take() {
            stop();
        }
    }
}

/// A cloneable, platform-agnostic handle to a live video source. Capture
/// runs while any clone is alive; the last drop stops it.
///
/// Not `Send` — it holds the main-thread native source and capture
/// lifecycle. The `Send` push side is [`FrameWriter`], handed to the
/// capture thread separately.
#[derive(Clone)]
pub struct MediaStream {
    inner: Rc<StreamInner>,
}

/// Pointer identity — a `MediaStream` is a HANDLE to a live capture, so clones
/// of one stream are equal and two independent streams never are.
///
/// There is no meaningful VALUE equality to offer: the payload is a live frame
/// channel plus a native source, and comparing frames would be both wrong (a
/// stream is not its current frame) and unbounded. 1.0 bounds the whole `Signal`
/// handle on `PartialEq`, and holding a camera/screen stream in app state is the
/// common case, so the guarded `set` needs *an* answer — pointer identity is the
/// one that matches this type's semantics. Mirrors `idea_theme`'s `ThemeSlot`.
impl PartialEq for MediaStream {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.inner, &other.inner)
    }
}

impl MediaStream {
    /// Create a stream and its producer [`FrameWriter`]. The producer spins
    /// up capture (writing through the `FrameWriter`), optionally sets a
    /// [`native source`](MediaStream::set_native_source), and attaches a
    /// [`stopper`](MediaStream::attach_stopper).
    #[allow(clippy::new_without_default)]
    pub fn new() -> (MediaStream, FrameWriter) {
        let channel = Arc::new(FrameChannel::default());
        let stream = MediaStream {
            inner: Rc::new(StreamInner {
                channel: channel.clone(),
                native: Rc::new(RefCell::new(None)),
                stopper: RefCell::new(None),
            }),
        };
        (stream, FrameWriter::from_channel(channel))
    }

    /// Like [`new`](Self::new), but wires a zero-copy native surface sink: the
    /// returned `FrameWriter` can publish rendered IOSurfaces
    /// ([`FrameWriter::publish_surface`]) and the stream's
    /// [`native_source`](Self::native_source) is the matching [`SurfaceSource`].
    /// This is the GPU self-capture path (a canvas renders into an IOSurface and
    /// publishes it; a recorder consumes it with no CPU readback). On non-macOS
    /// it's exactly [`new`](Self::new) (no IOSurface), so callers stay portable.
    #[cfg(target_os = "macos")]
    pub fn with_surface_capture() -> (MediaStream, FrameWriter) {
        let channel = Arc::new(FrameChannel::default());
        let (surf_source, surf_writer) = apple_surface::surface_channel();
        let stream = MediaStream {
            inner: Rc::new(StreamInner {
                channel: channel.clone(),
                native: Rc::new(RefCell::new(Some(Rc::new(surf_source) as Rc<dyn Any>))),
                stopper: RefCell::new(None),
            }),
        };
        let mut writer = FrameWriter::from_channel(channel);
        writer.surface = Some(surf_writer);
        (stream, writer)
    }

    /// Web: shares the `native` slot with the `FrameWriter` so the canvas
    /// renderer can publish its `captureStream()` web `MediaStream` as the
    /// stream's native source AFTER the `<canvas>` is created
    /// ([`FrameWriter::publish_native_source`]). The recorder then records that
    /// stream directly.
    #[cfg(target_arch = "wasm32")]
    pub fn with_surface_capture() -> (MediaStream, FrameWriter) {
        let channel = Arc::new(FrameChannel::default());
        let native: Rc<RefCell<Option<Rc<dyn Any>>>> = Rc::new(RefCell::new(None));
        let stream = MediaStream {
            inner: Rc::new(StreamInner {
                channel: channel.clone(),
                native: native.clone(),
                stopper: RefCell::new(None),
            }),
        };
        let mut writer = FrameWriter::from_channel(channel);
        writer.native_slot = Some(native);
        (stream, writer)
    }

    /// Linux (desktop): wires a zero-copy dma-buf sink. The returned `FrameWriter`
    /// publishes each rendered frame's exported dma-buf descriptor
    /// ([`FrameWriter::publish_dmabuf`]) and the stream's
    /// [`native_source`](Self::native_source) is the matching
    /// [`DmaBufSource`](dmabuf::DmaBufSource). This is the GPU self-capture path (a
    /// canvas renders into a GL texture, exports it as a dma-buf, and publishes it;
    /// a recorder imports it into GStreamer with no CPU readback in the canvas).
    #[cfg(target_os = "linux")]
    pub fn with_surface_capture() -> (MediaStream, FrameWriter) {
        // dma-buf zero-copy is ON by default (opt out with `IDEALYST_CANVAS_DMABUF=0`).
        // The canvas exports a genuinely-LINEAR GBM buffer (see
        // `canvas_vello::native_capture_linux`), which `glupload` imports correctly —
        // the earlier corruption was a GPU-tiled export mislabelled as linear, now
        // fixed. When disabled, return a PLAIN stream (no native source), so
        // `native_source()` is `None` and BOTH the recorder and the canvas take the
        // GPU→CPU read-back path. The gate MUST live here (the source of the native
        // source): gating only the canvas producer while the stream still advertised
        // a `DmaBufSource` made the recorder pick the dma-buf pipeline and wait
        // forever for frames the canvas never sent — which HUNG `stop()`. Both this
        // gate and the canvas producer's read the same `IDEALYST_CANVAS_DMABUF`, so
        // they always agree.
        if !dmabuf_capture_enabled() {
            return Self::new();
        }
        let channel = Arc::new(FrameChannel::default());
        let (dmabuf_source, dmabuf_writer) = dmabuf::dmabuf_channel();
        let stream = MediaStream {
            inner: Rc::new(StreamInner {
                channel: channel.clone(),
                native: Rc::new(RefCell::new(Some(Rc::new(dmabuf_source) as Rc<dyn Any>))),
                stopper: RefCell::new(None),
            }),
        };
        let mut writer = FrameWriter::from_channel(channel);
        writer.dmabuf = Some(dmabuf_writer);
        (stream, writer)
    }

    /// Other native targets (Windows desktop): no zero-copy self-capture path yet,
    /// so identical to [`new`](Self::new). Keeps cross-platform callers (the canvas
    /// capture) compiling everywhere.
    #[cfg(all(not(target_os = "macos"), not(target_os = "linux"), not(target_arch = "wasm32")))]
    pub fn with_surface_capture() -> (MediaStream, FrameWriter) {
        Self::new()
    }

    /// Record the platform's zero-copy frame source (e.g. a web
    /// `MediaStream`). A same-platform display / GPU consumer retrieves and
    /// downcasts it via [`native_source`](Self::native_source).
    pub fn set_native_source(&self, src: Rc<dyn Any>) {
        *self.inner.native.borrow_mut() = Some(src);
    }

    /// The platform's zero-copy frame source, if the producer set one. The
    /// consuming backend downcasts the `Rc<dyn Any>` to the type it expects.
    pub fn native_source(&self) -> Option<Rc<dyn Any>> {
        self.inner.native.borrow().clone()
    }

    /// Whether [`native_source`](Self::native_source) holds frames a GPU layer can
    /// import as textures. False for no source, and for the Android
    /// `PreviewSource`, which routes frames into a display's `Surface` and holds
    /// none of its own: a compositor takes the CPU path for that stream.
    pub fn has_importable_source(&self) -> bool {
        #[cfg(any(test, target_os = "android"))]
        {
            self.native_source().is_some_and(|source| !source.is::<PreviewSource>())
        }
        #[cfg(not(any(test, target_os = "android")))]
        {
            self.native_source().is_some()
        }
    }

    /// Attach the capture teardown closure, run when the last clone drops.
    /// Replaces any previously attached stopper.
    pub fn attach_stopper(&self, stop: impl FnOnce() + 'static) {
        *self.inner.stopper.borrow_mut() = Some(Box::new(stop));
    }

    /// Subscribe to frames (push). `callback` fires with each new
    /// [`VideoFrame`] until the returned [`Subscription`] is dropped. Runs on
    /// the producer's capture thread on native/Android, the main thread on
    /// web — keep it fast; copy pixels out rather than processing in place.
    pub fn subscribe<C: FrameCallback>(&self, callback: C) -> Subscription {
        let id = self.inner.channel.add_subscriber(Box::new(callback));
        Subscription {
            channel: self.inner.channel.clone(),
            id,
        }
    }

    /// Copy the most recent frame's pixels into `buf` (pull). Returns its
    /// dimensions, or `None` if no frame has arrived. Pair with
    /// [`generation`](Self::generation) to skip re-reading an unchanged
    /// frame — the shape a GPU blit consumes.
    ///
    /// A pull counts as demand. With nobody subscribed, a producer that makes CPU
    /// frames only on demand (the camera behind a native preview) has none, so
    /// this first answers `None` and starts it; a caller polling at least every
    /// two seconds then gets frames, the first within a few frames' time. It
    /// answers from a one-off call only if demand was already up, so a single
    /// press of a shutter wants [`screenshot`](Self::screenshot), which waits.
    pub fn latest(&self, buf: &mut Vec<u8>) -> Option<(u32, u32)> {
        if !self.inner.channel.pull() {
            return None;
        }
        let slot = self.inner.channel.latest.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let frame = slot.as_ref()?;
        buf.clear();
        buf.extend_from_slice(&frame.rgba);
        Some((frame.width, frame.height))
    }

    /// The capture timestamp of the most recent frame (microseconds on the
    /// shared [`clock`] timeline), or `None` if none has arrived. Lets a pull
    /// consumer read a frame's PTS without copying its pixels.
    pub fn latest_pts(&self) -> Option<u64> {
        self.inner
            .channel
            .latest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|f| f.pts_micros)
    }

    /// A counter bumped on every frame. Compare across calls to detect a new
    /// frame without copying.
    pub fn generation(&self) -> u64 {
        self.inner.channel.generation.load(Ordering::Acquire)
    }

    /// Capture a **single still frame** as CPU `RGBA8` — a one-shot screenshot of
    /// whatever the stream is currently showing. Platform-agnostic for callers: a
    /// thumbnail generator just `await`s it and gets pixels, with no per-platform
    /// code of its own.
    ///
    /// - **Web:** reads the current frame off the canvas `captureStream()` (our
    ///   `native_source`) via a `<video>` + 2D `getImageData`. Works on a WebGPU
    ///   canvas (the capture stream taps the live surface — unlike `toBlob`, which
    ///   reads the cleared post-present buffer). Self-contained: the stream is
    ///   already live, so no repaint is needed.
    /// - **Native:** `subscribe`s for one CPU frame. A camera turns its CPU path on
    ///   for that frame (on Android its `ImageReader` joins the repeating request,
    ///   so the frame is a new one, a few frames' time later) and off again after.
    ///   The GPU renderer reads back GPU→CPU only when a consumer `wants_cpu_frames`
    ///   (which `subscribe` flips on) AND it renders. The renderer is
    ///   repaint-on-demand, so **the caller must trigger a repaint** after calling
    ///   this (e.g. `CoreCanvas::snapshot_png` bumps the canvas's repaint nonce) or
    ///   the future won't resolve. The subscribe happens on first poll, before the
    ///   caller's deferred repaint renders, so ordering is safe.
    ///
    /// `None` if no source is attached, the frame can't be read, or the producer
    /// ended ([`FrameWriter::end_of_stream`]) before one came.
    pub async fn screenshot(&self) -> Option<Screenshot> {
        #[cfg(target_arch = "wasm32")]
        {
            self.screenshot_web().await
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (tx, rx) = futures_channel::oneshot::channel::<(Vec<u8>, u32, u32)>();
            let mut tx = Some(tx);
            // Hold the subscription until a frame arrives; flipping `wants_cpu_frames`
            // makes the renderer read back GPU→CPU on its next render.
            let _sub = self.subscribe(move |f: &VideoFrame| {
                if let Some(tx) = tx.take() {
                    let _ = tx.send((f.data.to_vec(), f.width, f.height));
                }
            });
            let (data, width, height) = rx.await.ok()?;
            Some(Screenshot { data, width, height })
        }
    }

    #[cfg(target_arch = "wasm32")]
    async fn screenshot_web(&self) -> Option<Screenshot> {
        let stream = self
            .native_source()?
            .downcast::<web_glue::dom::MediaStream>()
            .ok()?;
        web_screenshot::grab(&stream).await
    }
}

/// The web leg of [`MediaStream::screenshot`]: one frame off the stream's web
/// `MediaStream`, through a detached `<video>` and a 2D canvas.
#[cfg(target_arch = "wasm32")]
mod web_screenshot {
    use web_glue::{JsFuture, JsValue};

    use crate::Screenshot;

    /// How many animation frames to wait for the first decoded frame.
    const MAX_WAIT_FRAMES: u32 = 30;
    /// `HTMLMediaElement.HAVE_CURRENT_DATA`: a current frame is drawable.
    const HAVE_CURRENT_DATA: u32 = 2;

    web_glue::import! {
        // A detached `<video>` — the readable surface, never inserted into the
        // DOM. Muted and inline, each as both property and attribute: iOS
        // Safari only autoplays a muted video, and a video that is not inline
        // waits for fullscreen playback, so it never decodes a frame and
        // `screenshot()` times out empty. WebKit has read the attributes or the
        // properties depending on version, so both are set.
        fn js_detached_video() -> u32 =
            "() => { const v = document.createElement('video'); \
               v.muted = true; v.setAttribute('muted', ''); \
               v.playsInline = true; v.setAttribute('playsinline', ''); return G.add(v); }";
        // Play stream `s` in `v`. `play()`'s rejection (autoplay policy) is
        // observed here; the wait below simply times out if nothing decodes.
        #[catch]
        fn js_play_stream(v: u32, s: u32) -> u32 =
            "(v, s) => { const e = G.get(v); e.srcObject = G.get(s); \
               const p = e.play(); if (p) p.catch(() => {}); return 1; }";
        // A Promise resolving on the next animation frame.
        fn js_next_frame() -> u32 = "() => G.add(new Promise((r) => requestAnimationFrame(r)))";
        fn js_ready_state(v: u32) -> u32 = "(v) => G.get(v).readyState >>> 0";
        fn js_video_width(v: u32) -> u32 = "(v) => G.get(v).videoWidth >>> 0";
        fn js_video_height(v: u32) -> u32 = "(v) => G.get(v).videoHeight >>> 0";
        // Draw the current frame into a w×h canvas and copy its straight RGBA8
        // into the w*h*4 bytes at `out`. 1 on success, 0 if it threw.
        fn js_read_rgba(v: u32, w: u32, h: u32, out: usize) -> u32 =
            "(v, w, h, o) => { try { const c = document.createElement('canvas'); c.width = w; c.height = h; \
               const x = c.getContext('2d'); x.drawImage(G.get(v), 0, 0); \
               G.u8().set(x.getImageData(0, 0, w, h).data, o >>> 0); return 1; } catch (_) { return 0; } }";
        // Stop the `<video>` tapping the stream (the stream itself lives on).
        fn js_release(v: u32) = "(v) => { const e = G.get(v); e.pause(); e.srcObject = null; }";
    }

    /// The configured, not-yet-playing `<video>` [`grab`] reads frames from.
    pub(crate) fn detached_video() -> JsValue {
        // SAFETY: a fresh element handle.
        unsafe { JsValue::from_raw(js_detached_video()) }
    }

    pub(crate) async fn grab(stream: &web_glue::dom::MediaStream) -> Option<Screenshot> {
        let video = detached_video();
        // SAFETY: live element and stream handles.
        unsafe { js_play_stream(video.raw(), stream.as_js().raw()) }.ok()?;

        // Wait (a few animation frames) for the first decoded frame to land.
        for _ in 0..MAX_WAIT_FRAMES {
            // SAFETY: a fresh Promise handle.
            let frame = unsafe { JsValue::from_raw(js_next_frame()) };
            let _ = JsFuture::new(&frame).await;
            // SAFETY (below): live element handle.
            if unsafe { js_video_width(video.raw()) } > 0
                && unsafe { js_ready_state(video.raw()) } >= HAVE_CURRENT_DATA
            {
                break;
            }
        }
        let (w, h) = unsafe { (js_video_width(video.raw()), js_video_height(video.raw())) };
        let result = if w == 0 || h == 0 {
            None
        } else {
            let mut data = vec![0u8; w as usize * h as usize * 4];
            // SAFETY: `data` holds exactly w*h*4 bytes.
            (unsafe { js_read_rgba(video.raw(), w, h, data.as_mut_ptr() as usize) } == 1)
                .then_some(Screenshot { data, width: w, height: h })
        };
        // SAFETY: live element handle.
        unsafe { js_release(video.raw()) };
        result
    }
}

/// The screenshot `<video>`'s configuration, in a real browser. The element is
/// internal (built inside `screenshot()`), so this lives in the crate rather
/// than `tests/web_media_stream.rs`. Desktop engines decode a non-inline video
/// anyway, so the end-to-end screenshot test cannot catch a missing
/// `playsinline`; asserting the element's state is the closest reachable check.
#[cfg(all(test, target_arch = "wasm32"))]
mod web_screenshot_tests {
    use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};
    use web_glue::js::Function;
    use web_glue::JsValue;

    wasm_bindgen_test_configure!(run_in_browser);

    fn eval_on(video: &JsValue, body: &str) -> JsValue {
        Function::new_with_args("v", body).call1(&JsValue::UNDEFINED, video).expect("probe")
    }

    /// The bug: the element was created without `playsinline`, so on iPhone
    /// Safari it never produced a frame and `screenshot()` returned `None`.
    #[wasm_bindgen_test]
    fn regression_screenshot_video_is_inline_and_muted_on_ios_safari() {
        let video = super::web_screenshot::detached_video();
        for probe in [
            "return v.playsInline === true",
            "return v.hasAttribute('playsinline')",
            "return v.muted === true",
            "return v.hasAttribute('muted')",
        ] {
            assert_eq!(eval_on(&video, probe).as_bool(), Some(true), "{probe}");
        }
    }
}

/// A one-shot CPU screenshot: tightly-packed top-down `RGBA8`, `width*height*4`
/// bytes. Returned by [`MediaStream::screenshot`].
pub struct Screenshot {
    /// Tightly-packed top-down `RGBA8` pixels (`width * height * 4` bytes).
    pub data: Vec<u8>,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
}

/// Cancels a [`subscribe`](MediaStream::subscribe) when dropped. Hold it for
/// as long as you want the callback to fire.
///
/// Do not drop a `Subscription` from inside its own callback (the channel's
/// subscriber list is locked during fan-out).
pub struct Subscription {
    channel: Arc<FrameChannel>,
    id: u64,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.channel.remove_subscriber(self.id);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::future::Future;

    // 2x1 RGBA frame.
    const W: u32 = 2;
    const H: u32 = 1;

    #[test]
    fn write_rgba8_at_carries_explicit_pts() {
        let (stream, writer) = MediaStream::new();
        let seen = Arc::new(Mutex::new(Vec::<(u32, u32, u64)>::new()));
        let _sub = {
            let seen = seen.clone();
            stream.subscribe(move |f: &VideoFrame| {
                assert_eq!(f.format, PixelFormat::Rgba8);
                assert_eq!(f.data.len(), f.byte_len());
                seen.lock().unwrap().push((f.width, f.height, f.pts_micros));
            })
        };
        writer.write_rgba8_at(W, H, &[1, 2, 3, 4, 5, 6, 7, 8], 4_242);
        assert_eq!(*seen.lock().unwrap(), vec![(W, H, 4_242)]);
    }

    /// The 1.0.1 identity impl, pinned. `Signal<T>` is bounded on
    /// `T: PartialEq` at creation and `get`, so without this a camera or
    /// screen stream could not be held in app state at all — and the
    /// orphan rule means only this crate can supply it.
    #[test]
    fn media_stream_clones_compare_equal() {
        let (stream, _writer) = MediaStream::new();
        assert!(stream == stream.clone(), "clones address the same capture");
    }

    /// Two independently opened streams are never the same stream, even
    /// after both have received byte-identical frames — the compare must
    /// read identity, not the current frame. If it collapsed them,
    /// swapping the camera for the screen-share would be swallowed by the
    /// guarded `set` and the preview would keep showing the old source.
    #[test]
    fn independently_opened_media_streams_compare_unequal() {
        let (a, wa) = MediaStream::new();
        let (b, wb) = MediaStream::new();
        assert!(a != b);
        wa.write_rgba8_at(W, H, &[1, 2, 3, 4, 5, 6, 7, 8], 0);
        wb.write_rgba8_at(W, H, &[1, 2, 3, 4, 5, 6, 7, 8], 0);
        assert!(
            a != b,
            "identical frames must not collapse two streams into one"
        );
    }

    #[test]
    fn bgra8_swizzles_to_rgba_and_stamps_monotonically() {
        let (stream, writer) = MediaStream::new();
        // B G R A  ->  R G B A
        writer.write_bgra8(1, 1, &[10, 20, 30, 40]);
        let p1 = {
            let slot = stream.inner.channel.latest.lock().unwrap();
            let f = slot.as_ref().unwrap();
            assert_eq!(f.rgba, vec![30, 20, 10, 40]);
            f.pts_micros
        };
        // Auto-stamped frames are non-decreasing on the shared clock.
        writer.write_bgra8(1, 1, &[10, 20, 30, 40]);
        let p2 = stream.inner.channel.latest.lock().unwrap().as_ref().unwrap().pts_micros;
        assert!(p2 >= p1);
    }

    #[test]
    fn write_rgba8_with_fills_in_place_and_publishes() {
        let (stream, writer) = MediaStream::new();
        let seen = Arc::new(Mutex::new(Vec::<(u32, u32, Vec<u8>)>::new()));
        let _sub = {
            let seen = seen.clone();
            stream.subscribe(move |f: &VideoFrame| {
                seen.lock().unwrap().push((f.width, f.height, f.data.to_vec()));
            })
        };
        writer.write_rgba8_with(W, H, |out| {
            assert_eq!(out.len(), 8);
            out.copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        });
        assert_eq!(stream.generation(), 1);
        assert_eq!(*seen.lock().unwrap(), vec![(W, H, vec![1, 2, 3, 4, 5, 6, 7, 8])]);
        let mut pulled = Vec::new();
        assert_eq!(stream.latest(&mut pulled), Some((W, H)));
        assert_eq!(pulled, [1, 2, 3, 4, 5, 6, 7, 8]);
    }

    /// The point of the in-place write: a steady stream keeps one allocation,
    /// and the buffer arrives unzeroed, so `fill` pays for its own writes only.
    #[test]
    fn write_rgba8_with_reuses_the_last_frame_buffer() {
        let (stream, writer) = MediaStream::new();
        let mut first = std::ptr::null();
        writer.write_rgba8_with(W, H, |out| {
            first = out.as_ptr();
            out.fill(9);
        });
        writer.write_rgba8_with(W, H, |out| {
            assert_eq!(out.as_ptr(), first, "same allocation");
            assert_eq!(out, [9; 8], "not zeroed between frames");
        });
        assert_eq!(stream.generation(), 2);
    }

    /// A quarter turn swaps width and height at the same byte length; a real
    /// size change has to shrink or grow to exactly the new frame.
    #[test]
    fn write_rgba8_with_sizes_the_buffer_to_each_frame() {
        let (stream, writer) = MediaStream::new();
        for (w, h) in [(2, 1), (1, 2), (3, 3), (1, 1)] {
            writer.write_rgba8_with(w, h, |out| {
                assert_eq!(out.len(), (w * h * 4) as usize);
                out.fill(7);
            });
            let mut pulled = Vec::new();
            assert_eq!(stream.latest(&mut pulled), Some((w, h)));
            assert_eq!(pulled, vec![7; (w * h * 4) as usize]);
        }
    }

    #[test]
    fn write_rgba8_with_ignores_an_empty_frame() {
        let (stream, writer) = MediaStream::new();
        writer.write_rgba8_with(0, 4, |_| panic!("no frame to fill"));
        writer.write_rgba8_with(4, 0, |_| panic!("no frame to fill"));
        assert_eq!(stream.generation(), 0);
    }

    /// Records what a demand-gated producer is told, in order.
    fn demand_log(writer: &FrameWriter) -> Arc<Mutex<Vec<bool>>> {
        let log = Arc::new(Mutex::new(Vec::new()));
        let sink = log.clone();
        writer.on_cpu_demand_change(move |wants| sink.lock().unwrap().push(wants));
        log
    }

    /// The whole point on a slow tablet: a stream only being displayed through
    /// its native source makes no CPU frames, and nothing has told the producer
    /// otherwise.
    #[test]
    fn regression_preview_only_stream_reports_no_cpu_demand() {
        let (_stream, writer) = MediaStream::new();
        let log = demand_log(&writer);
        assert!(!writer.wants_cpu_frames());
        assert!(log.lock().unwrap().is_empty());
    }

    #[test]
    fn demand_callback_follows_subscribers_edge_by_edge() {
        let (stream, writer) = MediaStream::new();
        let log = demand_log(&writer);
        let first = stream.subscribe(|_: &VideoFrame| {});
        let second = stream.subscribe(|_: &VideoFrame| {});
        assert_eq!(*log.lock().unwrap(), vec![true], "only the first subscriber is an edge");
        drop(first);
        assert_eq!(*log.lock().unwrap(), vec![true], "one subscriber is still demand");
        drop(second);
        assert_eq!(*log.lock().unwrap(), vec![true, false]);
        assert!(!writer.wants_cpu_frames());
    }

    #[test]
    fn registering_with_demand_already_up_fires_once() {
        let (stream, writer) = MediaStream::new();
        let _sub = stream.subscribe(|_: &VideoFrame| {});
        let log = demand_log(&writer);
        assert_eq!(*log.lock().unwrap(), vec![true]);
        writer.write_rgba8(W, H, &[0; 8]);
        assert_eq!(*log.lock().unwrap(), vec![true], "a frame is not an edge");
    }

    #[test]
    fn a_pull_with_nobody_subscribed_raises_demand_and_serves_the_next_frame() {
        let (stream, writer) = MediaStream::new();
        let log = demand_log(&writer);
        let mut buf = Vec::new();
        assert_eq!(stream.latest(&mut buf), None, "nothing to give yet");
        assert_eq!(*log.lock().unwrap(), vec![true], "the pull is the demand");
        assert!(writer.wants_cpu_frames());
        writer.write_rgba8(W, H, &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(stream.latest(&mut buf), Some((W, H)));
        assert_eq!(buf, [1, 2, 3, 4, 5, 6, 7, 8]);
    }

    /// The first frame after the deadline is what notices: there is no timer.
    #[test]
    fn an_expired_pull_lease_turns_demand_off_on_the_next_frame() {
        let (stream, writer) = MediaStream::new();
        let log = demand_log(&writer);
        let mut buf = Vec::new();
        stream.latest(&mut buf);
        stream.inner.channel.pull_until.store(0, Ordering::Release);
        assert_eq!(*log.lock().unwrap(), vec![true], "nothing has noticed yet");
        assert!(!writer.wants_cpu_frames(), "the poll sees the deadline at once");
        writer.write_rgba8(W, H, &[0; 8]);
        assert_eq!(*log.lock().unwrap(), vec![true, false]);
    }

    /// A shutter pressed long after the last tap ended must not get that tap's
    /// last frame back as if it were now.
    #[test]
    fn regression_gated_producer_never_serves_a_frame_from_before_the_idle_spell() {
        let (stream, writer) = MediaStream::new();
        let _log = demand_log(&writer);
        let tap = stream.subscribe(|_: &VideoFrame| {});
        writer.write_rgba8(W, H, &[9; 8]);
        drop(tap);
        let mut buf = Vec::new();
        assert_eq!(stream.latest(&mut buf), None, "idle, so what is retained is old");
        assert_eq!(stream.latest(&mut buf), None, "and still old until a frame comes");
        writer.write_rgba8(W, H, &[1; 8]);
        assert_eq!(stream.latest(&mut buf), Some((W, H)));
        assert_eq!(buf, [1; 8]);
    }

    /// A subscription that lands while nothing was wanted makes the same
    /// promise: nothing it sees through `latest` predates it.
    #[test]
    fn rising_demand_discards_the_retained_frame_of_a_gated_producer() {
        let (stream, writer) = MediaStream::new();
        let _log = demand_log(&writer);
        writer.write_rgba8(W, H, &[9; 8]);
        let _tap = stream.subscribe(|_: &VideoFrame| {});
        let mut buf = Vec::new();
        assert_eq!(stream.latest(&mut buf), None);
    }

    /// A producer that writes whether or not anyone wants it (a test, a
    /// synthetic stream) has no idle spell: its last write IS the current frame.
    #[test]
    fn ungated_producer_serves_a_pull_from_its_last_write() {
        let (stream, writer) = MediaStream::new();
        writer.write_rgba8(W, H, &[3; 8]);
        let mut buf = Vec::new();
        assert_eq!(stream.latest(&mut buf), Some((W, H)));
        assert!(writer.wants_cpu_frames(), "the pull still reads as demand to a poller");
    }

    /// The demand edge is what wakes a Camera2 `ImageReader`; `screenshot` has to
    /// raise it and let it fall by itself.
    #[test]
    fn screenshot_raises_demand_gets_a_frame_and_lowers_it() {
        let (stream, writer) = MediaStream::new();
        let log = demand_log(&writer);
        let mut future = Box::pin(stream.screenshot());
        let waker = std::task::Waker::noop();
        let mut cx = std::task::Context::from_waker(waker);
        assert!(future.as_mut().poll(&mut cx).is_pending());
        assert_eq!(*log.lock().unwrap(), vec![true]);
        writer.write_rgba8(W, H, &[7; 8]);
        let shot = match future.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(shot) => shot.expect("a frame"),
            std::task::Poll::Pending => panic!("the frame was written"),
        };
        assert_eq!((shot.width, shot.height), (W, H));
        assert_eq!(*log.lock().unwrap(), vec![true, false]);
    }

    /// Without `end_of_stream`, a shutter awaiting a frame from a camera that
    /// was unplugged or taken by another app would never come back.
    #[test]
    fn regression_screenshot_resolves_none_when_the_producer_ends() {
        let (stream, writer) = MediaStream::new();
        let mut future = Box::pin(stream.screenshot());
        let waker = std::task::Waker::noop();
        let mut cx = std::task::Context::from_waker(waker);
        assert!(future.as_mut().poll(&mut cx).is_pending());
        writer.end_of_stream();
        assert!(matches!(future.as_mut().poll(&mut cx), std::task::Poll::Ready(None)));
        assert!(!writer.wants_cpu_frames());
        // A subscriber arriving after the end is released at once, too.
        let mut late = Box::pin(stream.screenshot());
        assert!(matches!(late.as_mut().poll(&mut cx), std::task::Poll::Ready(None)));
    }

    #[test]
    fn short_frames_are_ignored() {
        let (stream, writer) = MediaStream::new();
        writer.write_rgba8(W, H, &[1, 2, 3]); // too short for 2x1x4
        assert_eq!(stream.generation(), 0);
    }
}

/// Whether dma-buf zero-copy self-capture is enabled. **ON by default** now that the
/// canvas exports a genuinely-linear buffer that `glupload` imports correctly; set
/// `IDEALYST_CANVAS_DMABUF=0` (or `false`) to force the GPU→CPU read-back path. Read
/// once, cached. Same flag the `canvas-vello` producer reads, so both sides agree.
#[cfg(target_os = "linux")]
fn dmabuf_capture_enabled() -> bool {
    use std::sync::OnceLock;
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("IDEALYST_CANVAS_DMABUF").map(|v| !(v == "0" || v == "false")).unwrap_or(true)
    })
}

