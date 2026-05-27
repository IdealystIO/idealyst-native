//! Viewport size — the host window / root view's logical size in
//! device-independent pixels.
//!
//! ## Reactive signal
//!
//! [`viewport_size()`] returns a `Signal<ViewportSize>` author code can
//! `.get()` inside an effect or derived. The value updates whenever
//! the active backend reports a change (window resize, orientation
//! flip, browser zoom, virtual keyboard taking screen real estate on
//! some platforms — backend-specific).
//!
//! Backends call [`set_viewport_size`] from whichever native hook
//! their platform exposes (UIKit `layoutSubviews`, AppKit per-tick
//! NSView bounds sample, Android `OnLayoutChangeListener`, web
//! `resize` event, wgpu `WindowEvent::Resized`, etc.). On platforms
//! that don't yet wire it up the signal stays at
//! [`ViewportSize::ZERO`] — degrades gracefully.
//!
//! ## What this is *not*
//!
//! - Not the safe-area insets. Use [`crate::safe_area_insets`] for
//!   that. A 393×852 iPhone viewport still has a status-bar inset on
//!   top.
//! - Not the renderable surface size (framebuffer pixels). The unit
//!   here is the same dp/CSS-pixel space `StyleRules` and Taffy use.
//!   Hosts that need physical pixels (e.g., wgpu surface
//!   configuration) own that conversion separately.
//! - Not a layout authority. The framework's layout pass still reads
//!   the host root's actual bounds; this signal is for *author*-code
//!   reactivity (breakpoint hooks, responsive containers).

use crate::reactive::Signal;
use std::cell::OnceCell;

/// Logical viewport dimensions in device-independent pixels. Both
/// fields are non-negative; backends clamp negative values to zero
/// before pushing.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct ViewportSize {
    pub width: f32,
    pub height: f32,
}

impl ViewportSize {
    pub const ZERO: Self = Self { width: 0.0, height: 0.0 };

    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

// ---------------------------------------------------------------------------
// Reactive viewport-size signal
// ---------------------------------------------------------------------------

thread_local! {
    /// The framework's authoritative viewport-size signal. Lazily
    /// initialized to `ViewportSize::ZERO` on first access; backends
    /// overwrite it as the platform reports changes.
    ///
    /// Thread-local because `Signal` is reactive-arena-backed and the
    /// reactive runtime is single-threaded (UI thread).
    static VIEWPORT: OnceCell<Signal<ViewportSize>> = const { OnceCell::new() };
}

fn viewport_signal() -> Signal<ViewportSize> {
    VIEWPORT.with(|cell| *cell.get_or_init(|| Signal::new(ViewportSize::ZERO)))
}

/// The reactive viewport-size signal. Read via `.get()` inside an
/// effect / derived to subscribe. The value updates whenever the
/// active backend reports a size change.
///
/// On platforms without a backend-side observer hooked up the value
/// stays at `ViewportSize::ZERO` — degrades gracefully.
pub fn viewport_size() -> Signal<ViewportSize> {
    viewport_signal()
}

/// Backend entry point: push a new value into the global viewport
/// signal. Called on the UI thread by each platform's observer.
/// Idempotent — the signal compares by equality, so repeated calls
/// with the same value don't re-fire dependents. Negative components
/// are clamped to zero.
pub fn set_viewport_size(size: ViewportSize) {
    let clamped = ViewportSize {
        width: size.width.max(0.0),
        height: size.height.max(0.0),
    };
    let sig = viewport_signal();
    if sig.get() != clamped {
        sig.set(clamped);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_size_zero_is_all_zeros() {
        let z = ViewportSize::ZERO;
        assert_eq!(z.width, 0.0);
        assert_eq!(z.height, 0.0);
    }

    #[test]
    fn viewport_size_default_matches_zero() {
        assert_eq!(ViewportSize::default(), ViewportSize::ZERO);
    }

    #[test]
    fn viewport_size_new_keeps_fields() {
        let v = ViewportSize::new(393.0, 852.0);
        assert_eq!(v.width, 393.0);
        assert_eq!(v.height, 852.0);
    }

    #[test]
    fn set_viewport_size_updates_the_signal() {
        let v = ViewportSize::new(1280.0, 800.0);
        set_viewport_size(v);
        assert_eq!(viewport_size().get(), v);

        // Idempotent same-value call.
        set_viewport_size(v);
        assert_eq!(viewport_size().get(), v);
    }

    #[test]
    fn set_viewport_size_clamps_negative_components_to_zero() {
        set_viewport_size(ViewportSize::new(-1.0, 600.0));
        let got = viewport_size().get();
        assert_eq!(got.width, 0.0);
        assert_eq!(got.height, 600.0);

        set_viewport_size(ViewportSize::new(800.0, -7.5));
        let got = viewport_size().get();
        assert_eq!(got.width, 800.0);
        assert_eq!(got.height, 0.0);
    }

    #[test]
    fn signal_handle_is_stable_across_reads() {
        let a = viewport_size();
        let b = viewport_size();
        // Same idempotent init → reads return the same value
        // regardless of whether earlier tests on this thread already
        // wrote to the signal.
        assert_eq!(a.get(), b.get());
    }
}
