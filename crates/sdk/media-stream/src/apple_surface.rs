//! Zero-copy Apple frame source: a latest-frame-handle slot shared between a
//! capture producer (on its background queue) and a main-thread display
//! consumer. The handle is an opaque retained CoreFoundation type — the
//! producer and consumer on a given platform agree on which:
//!
//! - **macOS** → an `IOSurface` (`CVPixelBufferGetIOSurface`). `CALayer.contents`
//!   accepts an `IOSurface` directly, so the GPU displays it with **zero** CPU
//!   touches (no BGRA→RGBA swizzle, no `CGImage`, no per-frame upload).
//! - **iOS** → a `CMSampleBuffer`, enqueued into an `AVSampleBufferDisplayLayer`
//!   (iOS `CALayer.contents` does *not* accept an `IOSurface`). Same retain/
//!   release lifecycle; the consumer just enqueues instead of assigning.
//!
//! This module is the platform handle the
//! [`MediaStream`](crate::MediaStream)'s `native_source` carries for that
//! fast-path. The type names say "surface" but the slot is format-agnostic —
//! it retains and hands back whatever CFTypeRef the producer publishes.
//!
//! ## Threading + lifecycle
//!
//! Capture runs on a private serial queue; the display reads on the main
//! thread. The two halves share an `Arc<Mutex<SurfaceSlot>>`:
//!
//! - [`SurfaceWriter::publish`] (capture queue) `CFRetain`s the new surface,
//!   swaps it into the slot, and `CFRelease`s the one it replaced. The slot
//!   owns exactly one retain on the surface it currently holds.
//! - [`SurfaceSource::acquire`] (main thread) returns the current surface with
//!   an **extra** retain the caller owns, so it stays alive across the
//!   `setContents:` call even if `publish` swaps in a newer one concurrently.
//!   The caller `CFRelease`s it after CoreAnimation has taken its own retain.
//!
//! `CFRetain`/`CFRelease` are atomic and thread-safe, so the cross-thread
//! retain dance is sound. When both halves drop, the slot's `Drop` releases the
//! last held surface. Only CoreFoundation (libSystem) is linked — no GPU deps,
//! keeping `media-stream` thin.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRetain(cf: *const c_void) -> *const c_void;
    fn CFRelease(cf: *const c_void);
}

/// The shared latest-surface slot. Holds one retain on `surface`.
struct SurfaceSlot {
    /// `IOSurfaceRef` bits, or 0 for "no frame yet". The slot owns one retain
    /// while non-zero.
    surface: usize,
}

impl Drop for SurfaceSlot {
    fn drop(&mut self) {
        if self.surface != 0 {
            unsafe { CFRelease(self.surface as *const c_void) };
            self.surface = 0;
        }
    }
}

struct Shared {
    slot: Mutex<SurfaceSlot>,
    /// Bumped on every `publish`; a display consumer compares it to skip work
    /// when no new frame has arrived.
    generation: AtomicU64,
}

/// Producer half (capture queue, `Send`): publishes the latest captured
/// `IOSurface`. Cheap to clone.
#[derive(Clone)]
pub struct SurfaceWriter {
    shared: Arc<Shared>,
}

/// Consumer half (main thread): the display reads the current `IOSurface` to
/// set as `CALayer.contents`. Carried type-erased in
/// [`MediaStream::native_source`](crate::MediaStream::native_source).
#[derive(Clone)]
pub struct SurfaceSource {
    shared: Arc<Shared>,
}

/// Create a paired [`SurfaceSource`] (consumer) and [`SurfaceWriter`]
/// (producer) over one shared latest-surface slot.
pub fn surface_channel() -> (SurfaceSource, SurfaceWriter) {
    let shared = Arc::new(Shared {
        slot: Mutex::new(SurfaceSlot { surface: 0 }),
        generation: AtomicU64::new(0),
    });
    (
        SurfaceSource {
            shared: shared.clone(),
        },
        SurfaceWriter { shared },
    )
}

impl SurfaceWriter {
    /// Publish a freshly captured `IOSurface` (its raw `IOSurfaceRef` pointer).
    /// Retains it for the slot and releases the previously held surface. A null
    /// pointer is ignored. Safe to call from the capture queue.
    ///
    /// # Safety
    /// `surface` must be a valid `IOSurfaceRef` (or null). The producer gets it
    /// from `CVPixelBufferGetIOSurface` on a live pixel buffer; this retains it
    /// so it survives past the buffer's recycle.
    pub unsafe fn publish(&self, surface: *const c_void) {
        if surface.is_null() {
            return;
        }
        // Retain for the slot BEFORE taking the lock (CFRetain is thread-safe).
        CFRetain(surface);
        let old = {
            let mut slot = self.shared.slot.lock().unwrap();
            let old = slot.surface;
            slot.surface = surface as usize;
            old
        };
        self.shared.generation.fetch_add(1, Ordering::Release);
        // Release the displaced surface OUTSIDE the lock.
        if old != 0 {
            CFRelease(old as *const c_void);
        }
    }
}

impl SurfaceSource {
    /// A counter bumped on every [`publish`](SurfaceWriter::publish). Compare
    /// across frames to skip re-setting an unchanged surface.
    pub fn generation(&self) -> u64 {
        self.shared.generation.load(Ordering::Acquire)
    }

    /// Acquire the current `IOSurface` with an **extra** retain the caller owns.
    /// Returns null if no frame has been published. The caller sets it as
    /// `CALayer.contents` (CoreAnimation takes its own retain) and then must
    /// [`release`](SurfaceSource::release) the returned pointer.
    pub fn acquire(&self) -> *const c_void {
        let slot = self.shared.slot.lock().unwrap();
        if slot.surface == 0 {
            return std::ptr::null();
        }
        unsafe { CFRetain(slot.surface as *const c_void) }
    }

    /// Release a pointer obtained from [`acquire`](SurfaceSource::acquire),
    /// after CoreAnimation has retained it via `setContents:`.
    ///
    /// # Safety
    /// `surface` must be a non-null pointer returned by `acquire` (balances its
    /// extra retain exactly once).
    pub unsafe fn release(&self, surface: *const c_void) {
        if !surface.is_null() {
            CFRelease(surface);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A fake "CF object" with an atomic refcount, so the retain/release dance
    // can be exercised without a real IOSurface. We can't redirect the extern
    // CFRetain/CFRelease here, so this test validates the slot's generation +
    // swap bookkeeping (the pointer plumbing) rather than the CF calls — but it
    // uses a null surface to avoid touching CF entirely.
    #[test]
    fn null_publish_is_ignored_and_generation_starts_zero() {
        let (source, writer) = surface_channel();
        assert_eq!(source.generation(), 0);
        unsafe { writer.publish(std::ptr::null()) };
        assert_eq!(source.generation(), 0, "null publish must not bump generation");
        assert!(source.acquire().is_null(), "no surface → acquire is null");
    }

    #[test]
    fn channel_halves_share_one_slot() {
        let (source, writer) = surface_channel();
        // Cloning either half keeps the same shared generation counter.
        let source2 = source.clone();
        let _writer2 = writer.clone();
        assert_eq!(source.generation(), source2.generation());
    }
}
