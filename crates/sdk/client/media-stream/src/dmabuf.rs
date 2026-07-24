//! Zero-copy Linux frame source: a latest-frame-descriptor slot shared between a
//! GPU producer (the canvas, on the render thread) and a same-platform consumer
//! (a recorder / display GPU layer). The descriptor is a **dma-buf** handle — a
//! GPU buffer the producer exported from its render target via
//! `EGL_MESA_image_dma_buf_export` — so a GPU-rendered frame stays a GPU handle
//! through the stream and only touches the CPU if a consumer (a software encoder)
//! demands it (GStreamer `gldownload` at the encoder boundary).
//!
//! This is the Linux analog of [`apple_surface`](crate::apple_surface) (IOSurface):
//! same `MediaStream::native_source` fast-path, same "wants"/tap gating so an
//! un-recorded canvas pays nothing, same latest-slot + generation. It differs in
//! one way that matters for lifetime:
//!
//! - An IOSurface is a `CFTypeRef` with an atomic refcount; the Apple channel does
//!   a `CFRetain`/`CFRelease` dance so producer and consumer each hold a retain.
//! - A dma-buf is a plain file descriptor with **no refcount**. The producer's
//!   render ring owns the fd (one [`OwnedFd`] per ring texture, exported once and
//!   kept for the recording's lifetime). The published [`DmaBufFrame`] carries a
//!   **borrowed** `RawFd` — valid only while the producer keeps that ring texture
//!   alive. A consumer that needs the buffer past the current frame (a GStreamer
//!   `appsrc` reads asynchronously) **must `dup(2)` the fd when it imports it**;
//!   the descriptor makes no ownership promise. This mirrors how the Apple
//!   consumer re-retains in `acquire`, adapted to fds having no refcount.
//!
//! The producer's render cadence is the sync: with a ring of `POOL` textures the
//! canvas never overwrites the buffer a consumer is still reading `POOL` frames
//! later (see `canvas-vello`'s `native_capture_linux`).

use std::os::fd::RawFd;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// `DRM_FORMAT_MOD_INVALID` — Mesa reports this when a dma-buf export carries no
/// explicit format modifier (the buffer's tiling is implicit / driver-internal).
/// A consumer importing the fd must treat it as "no modifier", NOT as a real
/// modifier value, or dma-buf import negotiation fails. Measured value from the
/// export spike on Mesa; equals `(1 << 56) - 1`.
pub const DRM_FORMAT_MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;

/// A single-plane dma-buf frame descriptor — the concrete type
/// [`MediaStream::native_source`](crate::MediaStream::native_source) carries on
/// Linux for the zero-copy GPU fast-path.
///
/// `fd` is **borrowed**: it is owned by the producer's render ring and is valid
/// only while that ring texture lives (the recording's lifetime). A consumer that
/// outlives the current frame MUST `dup(2)` it before use — see the module docs.
///
/// `Copy` because every field is a plain value; copying the descriptor does NOT
/// duplicate the underlying buffer or the fd.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DmaBufFrame {
    /// Borrowed dma-buf file descriptor (see the type-level and module docs).
    pub fd: RawFd,
    /// Buffer width in pixels.
    pub width: u32,
    /// Buffer height in pixels.
    pub height: u32,
    /// DRM fourcc pixel format (e.g. `DRM_FORMAT_ABGR8888` = `0x34324241`, the
    /// export of a wgpu `Rgba8Unorm` texture — byte order R, G, B, A in memory).
    pub fourcc: u32,
    /// Row stride in bytes for plane 0.
    pub stride: i32,
    /// Byte offset of plane 0 within the buffer.
    pub offset: i32,
    /// DRM format modifier, or [`DRM_FORMAT_MOD_INVALID`] when the driver reports
    /// none (the common Mesa case — import without an explicit modifier).
    pub modifier: u64,
}

/// The shared latest-frame slot + gating. Holds a plain descriptor (no fd
/// ownership — see the module docs).
struct Shared {
    slot: Mutex<Option<DmaBufFrame>>,
    /// Bumped on every [`publish`](DmaBufWriter::publish); a consumer compares it
    /// to skip re-importing an unchanged buffer.
    generation: AtomicU64,
    /// Live count of consumers that want the producer to keep exporting + publishing
    /// dma-buf frames (each holds a [`NativeTap`]). The GPU producer reads this via
    /// [`DmaBufWriter::wants`] to skip the per-frame export when nobody records —
    /// the Linux analogue of `SurfaceWriter::wants_surface`.
    native_taps: AtomicUsize,
}

/// Producer half (render thread): publishes the latest exported dma-buf frame.
/// Cheap to clone.
#[derive(Clone)]
pub struct DmaBufWriter {
    shared: Arc<Shared>,
}

/// Consumer half: reads the current dma-buf frame to import into a GStreamer
/// pipeline (or a display GPU layer). Carried type-erased in
/// [`MediaStream::native_source`](crate::MediaStream::native_source).
#[derive(Clone)]
pub struct DmaBufSource {
    shared: Arc<Shared>,
}

/// Create a paired [`DmaBufSource`] (consumer) and [`DmaBufWriter`] (producer)
/// over one shared latest-frame slot.
pub fn dmabuf_channel() -> (DmaBufSource, DmaBufWriter) {
    let shared = Arc::new(Shared {
        slot: Mutex::new(None),
        generation: AtomicU64::new(0),
        native_taps: AtomicUsize::new(0),
    });
    (
        DmaBufSource {
            shared: shared.clone(),
        },
        DmaBufWriter { shared },
    )
}

impl DmaBufWriter {
    /// Whether any consumer currently wants dma-buf frames (holds a [`NativeTap`]).
    /// The GPU producer gates its per-frame export + [`publish`](Self::publish) on
    /// this: do the GPU capture work only while something records.
    pub fn wants(&self) -> bool {
        self.shared.native_taps.load(Ordering::Acquire) > 0
    }

    /// Publish the latest exported dma-buf [`DmaBufFrame`] descriptor. The producer
    /// keeps the fd alive (its ring texture); this only records the borrowed
    /// descriptor + bumps the generation. Safe to call from the render thread.
    pub fn publish(&self, frame: DmaBufFrame) {
        *self.shared.slot.lock().unwrap() = Some(frame);
        self.shared.generation.fetch_add(1, Ordering::Release);
    }
}

impl DmaBufSource {
    /// A counter bumped on every [`publish`](DmaBufWriter::publish). Compare across
    /// frames to skip re-importing an unchanged buffer.
    pub fn generation(&self) -> u64 {
        self.shared.generation.load(Ordering::Acquire)
    }

    /// The current dma-buf frame descriptor, or `None` if none has been published.
    /// The returned `fd` is **borrowed** (owned by the producer's ring); a consumer
    /// that outlives the current frame must `dup(2)` it — see the module docs.
    pub fn acquire(&self) -> Option<DmaBufFrame> {
        *self.shared.slot.lock().unwrap()
    }

    /// Register interest in dma-buf frames. While the returned [`NativeTap`] is
    /// alive, [`DmaBufWriter::wants`] is true, so the producer exports + publishes
    /// each frame. A recorder holds this for the recording's lifetime; dropping it
    /// lets the GPU producer stop the per-frame export work.
    pub fn register_tap(&self) -> NativeTap {
        self.shared.native_taps.fetch_add(1, Ordering::AcqRel);
        NativeTap {
            shared: self.shared.clone(),
        }
    }
}

/// A consumer's "keep exporting dma-buf frames" guard (see
/// [`DmaBufSource::register_tap`]). Decrements the live tap count on drop.
pub struct NativeTap {
    shared: Arc<Shared>,
}

impl Drop for NativeTap {
    fn drop(&mut self) {
        self.shared.native_taps.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_starts_zero_and_bumps_on_publish() {
        let (source, writer) = dmabuf_channel();
        assert_eq!(source.generation(), 0);
        assert!(source.acquire().is_none(), "no frame yet → acquire is None");

        let frame = DmaBufFrame {
            fd: 42,
            width: 64,
            height: 48,
            fourcc: 0x3432_4241,
            stride: 256,
            offset: 0,
            modifier: DRM_FORMAT_MOD_INVALID,
        };
        writer.publish(frame);
        assert_eq!(source.generation(), 1, "publish bumps generation");
        assert_eq!(source.acquire(), Some(frame), "acquire returns the published frame");
    }

    #[test]
    fn wants_tracks_tap_lifetime() {
        let (source, writer) = dmabuf_channel();
        assert!(!writer.wants(), "no tap → producer does no export work");
        let tap = source.register_tap();
        assert!(writer.wants(), "an active tap makes the producer export");
        let tap2 = source.register_tap();
        drop(tap);
        assert!(writer.wants(), "still wanted while a second tap is alive");
        drop(tap2);
        assert!(!writer.wants(), "last tap dropped → producer stops exporting");
    }

    #[test]
    fn channel_halves_share_one_slot() {
        let (source, writer) = dmabuf_channel();
        let source2 = source.clone();
        let _writer2 = writer.clone();
        assert_eq!(source.generation(), source2.generation());
    }
}
