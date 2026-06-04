//! A platform-agnostic handle to a live video source.
//!
//! `MediaStream` is the common currency between capture SDKs that *produce*
//! video (`camera`, `screen-recorder`) and the layers that *consume* it
//! (a display component, a GPU compositor). A developer wires `camera ->
//! video` and never names a platform type — the per-platform transport
//! (a web `MediaStream`, an Apple `CVPixelBuffer`, an Android
//! `SurfaceTexture`) is hidden.
//!
//! A stream exposes two access paths over one source:
//!
//! - **CPU frame tap** — [`MediaStream::subscribe`] delivers normalized
//!   `RGBA8` [`VideoFrame`]s on a callback (analysis, a
//!   lowest-common-denominator display, a fallback), and
//!   [`MediaStream::latest`] polls the most recent frame.
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
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
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
pub use apple_surface::{surface_channel, SurfaceSource, SurfaceWriter};

mod audio;
pub use audio::{
    AudioFormat, AudioFrame, AudioFrameCallback, AudioStream, AudioSubscription, AudioWriter,
};

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
}

impl FrameChannel {
    // Single-producer: frames arrive from one capture source. We take the
    // previous frame's buffer out to reuse its allocation, fill it, fan out
    // to push subscribers WITHOUT holding the `latest` lock (so a subscriber
    // may safely call `latest()`), then store it back as the new latest.
    fn write(&self, width: u32, height: u32, pts_micros: u64, fill: impl FnOnce(&mut Vec<u8>)) {
        if width == 0 || height == 0 {
            return;
        }
        let mut buf = self
            .latest
            .lock()
            .unwrap()
            .take()
            .map(|f| f.rgba)
            .unwrap_or_default();
        buf.clear();
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
            let mut subs = self.subscribers.lock().unwrap();
            for (_, cb) in subs.iter_mut() {
                cb(&view);
            }
        }

        *self.latest.lock().unwrap() = Some(OwnedFrame {
            width,
            height,
            rgba: buf,
            pts_micros,
        });
        self.generation.fetch_add(1, Ordering::Release);
    }

    fn add_subscriber(&self, cb: BoxedCallback) -> u64 {
        let id = self.next_sub_id.fetch_add(1, Ordering::Relaxed);
        let mut subs = self.subscribers.lock().unwrap();
        subs.push((id, cb));
        self.subscriber_count.store(subs.len(), Ordering::Release);
        id
    }

    fn remove_subscriber(&self, id: u64) {
        let mut subs = self.subscribers.lock().unwrap();
        subs.retain(|(i, _)| *i != id);
        self.subscriber_count.store(subs.len(), Ordering::Release);
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
}

impl FrameWriter {
    /// Whether any consumer is currently tapping CPU frames (has an active
    /// [`MediaStream::subscribe`]). A producer that can also publish a
    /// zero-copy native source (the Apple [`SurfaceWriter`]) uses this to skip
    /// per-frame CPU normalization — e.g. the screen-recorder's full-frame
    /// BGRA→RGBA swizzle — when only a native-source display is attached.
    /// Pull consumers (`latest`) that need CPU frames should `subscribe` so the
    /// channel knows to keep producing them.
    pub fn wants_cpu_frames(&self) -> bool {
        self.channel.subscriber_count.load(Ordering::Acquire) > 0
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
            buf.extend_from_slice(&data[..need]);
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
    /// display / GPU consumer.
    native: RefCell<Option<Rc<dyn Any>>>,
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
                native: RefCell::new(None),
                stopper: RefCell::new(None),
            }),
        };
        (stream, FrameWriter { channel })
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
    pub fn latest(&self, buf: &mut Vec<u8>) -> Option<(u32, u32)> {
        let slot = self.inner.channel.latest.lock().unwrap();
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
            .unwrap()
            .as_ref()
            .map(|f| f.pts_micros)
    }

    /// A counter bumped on every frame. Compare across calls to detect a new
    /// frame without copying.
    pub fn generation(&self) -> u64 {
        self.inner.channel.generation.load(Ordering::Acquire)
    }
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
    fn short_frames_are_ignored() {
        let (stream, writer) = MediaStream::new();
        writer.write_rgba8(W, H, &[1, 2, 3]); // too short for 2x1x4
        assert_eq!(stream.generation(), 0);
    }
}
