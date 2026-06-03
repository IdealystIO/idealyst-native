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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

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
}

impl FrameChannel {
    // Single-producer: frames arrive from one capture source. We take the
    // previous frame's buffer out to reuse its allocation, fill it, fan out
    // to push subscribers WITHOUT holding the `latest` lock (so a subscriber
    // may safely call `latest()`), then store it back as the new latest.
    fn write(&self, width: u32, height: u32, fill: impl FnOnce(&mut Vec<u8>)) {
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
        });
        self.generation.fetch_add(1, Ordering::Release);
    }

    fn add_subscriber(&self, cb: BoxedCallback) -> u64 {
        let id = self.next_sub_id.fetch_add(1, Ordering::Relaxed);
        self.subscribers.lock().unwrap().push((id, cb));
        id
    }

    fn remove_subscriber(&self, id: u64) {
        self.subscribers.lock().unwrap().retain(|(i, _)| *i != id);
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
    /// Push a tightly-packed top-down `RGBA8` frame. Frames shorter than
    /// `width * height * 4` are ignored.
    pub fn write_rgba8(&self, width: u32, height: u32, data: &[u8]) {
        let need = match checked_len(width, height, data) {
            Some(n) => n,
            None => return,
        };
        self.channel.write(width, height, |buf| {
            buf.extend_from_slice(&data[..need]);
        });
    }

    /// Push a tightly-packed top-down `BGRA8` frame (Apple / Windows layout);
    /// channels are swizzled to `RGBA8`. Frames shorter than
    /// `width * height * 4` are ignored.
    pub fn write_bgra8(&self, width: u32, height: u32, data: &[u8]) {
        let need = match checked_len(width, height, data) {
            Some(n) => n,
            None => return,
        };
        self.channel.write(width, height, |buf| {
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
