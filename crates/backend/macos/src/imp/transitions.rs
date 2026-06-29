//! CSS-style `transitions { … }` for macOS — animated interpolation of color
//! properties when a styled value changes (the canonical case: a theme toggle
//! fading `background`/`color` over 250ms).
//!
//! iOS leans on `UIView.animateWithDuration:`, which implicitly animates the
//! property changes inside its block. AppKit has no equivalent that spans both
//! CALayer-backed colors AND `NSScrollView`'s `drawsBackground` AppKit color,
//! and layer-backed `NSView`s suppress implicit CALayer animations anyway. So
//! we drive transitions ourselves with a single raf-clock color tween that
//! works for every setter:
//!
//! - regular view background → `layer.backgroundColor`
//! - scroll-view background  → `NSScrollView`/`NSClipView` `backgroundColor`
//!   (the visible body/sidebar theme fade)
//! - label text color        → `NSTextField.textColor`
//!
//! `apply_color` is called from `apply_style` instead of snapping the color. It
//! diffs against the LAST applied value (tracked here, so no native read-back);
//! if a `Transition` is set and the value changed, it registers an animation and
//! drives it off `runtime_core::scheduling::raf_loop`. The raf is installed lazily
//! when the first transition starts and dropped (via a microtask, never from
//! inside its own tick) when the last one finishes.

use std::cell::RefCell;
use std::collections::HashMap;

use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2_app_kit::{NSColor, NSView};
use runtime_core::scheduling::RafLoop;
use runtime_core::{Easing, Transition};

use crate::imp::CGColorRef;

/// Which color setter a transition drives.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ColorProp {
    Background,
    TextColor,
}

struct Active {
    /// Keeps the target view alive for the (short) duration of the tween.
    view: Retained<NSView>,
    prop: ColorProp,
    is_scroll: bool,
    from: [f32; 4],
    to: [f32; 4],
    start_ms: u64,
    duration_ms: u32,
    easing: Easing,
}

type Key = (usize, ColorProp);

thread_local! {
    /// Last color applied per (view, prop) — the tween's `from` on the next
    /// change. Avoids reading back native colors (which may be mid-animation).
    static LAST: RefCell<HashMap<Key, [f32; 4]>> = RefCell::new(HashMap::new());
    static ACTIVE: RefCell<HashMap<Key, Active>> = RefCell::new(HashMap::new());
    static RAF: RefCell<Option<RafLoop>> = RefCell::new(None);
}

fn now_ms() -> u64 {
    runtime_core::time::now_micros() / 1000
}

/// Apply `to` to `view`'s `prop`, animating over `transition` if one is set and
/// the value actually changed (else snap). `is_scroll` selects the scroll-view
/// AppKit background setter over the CALayer one. Call this from `apply_style`
/// in place of a direct color set.
pub(crate) fn apply_color(
    view: &NSView,
    prop: ColorProp,
    is_scroll: bool,
    to: [f32; 4],
    transition: Option<&Transition>,
) {
    let key = (view as *const NSView as usize, prop);
    // `from`: the live animated value if a tween is mid-flight (so we retarget
    // smoothly), else the last applied value.
    let now = now_ms();
    let from = ACTIVE
        .with(|a| a.borrow().get(&key).map(|t| value_at(t, now)))
        .or_else(|| LAST.with(|l| l.borrow().get(&key).copied()));
    LAST.with(|l| {
        l.borrow_mut().insert(key, to);
    });

    match (transition, from) {
        (Some(t), Some(from)) if t.duration_ms > 0 && color_changed(from, to) => {
            ACTIVE.with(|a| {
                a.borrow_mut().insert(
                    key,
                    Active {
                        view: retain(view),
                        prop,
                        is_scroll,
                        from,
                        to,
                        start_ms: now,
                        duration_ms: t.duration_ms,
                        easing: t.easing,
                    },
                );
            });
            ensure_raf();
            // Paint the start frame now so there's no one-frame flash of `to`.
            set_color(view, prop, is_scroll, from);
        }
        _ => {
            ACTIVE.with(|a| {
                a.borrow_mut().remove(&key);
            });
            set_color(view, prop, is_scroll, to);
        }
    }
}

/// Forget a view's transition state when it's torn down (`clear_children` /
/// `release`), so a recycled NSView pointer can't inherit a stale `from`.
pub(crate) fn forget_view(view: &NSView) {
    let ptr = view as *const NSView as usize;
    LAST.with(|l| l.borrow_mut().retain(|k, _| k.0 != ptr));
    ACTIVE.with(|a| a.borrow_mut().retain(|k, _| k.0 != ptr));
}

fn retain(view: &NSView) -> Retained<NSView> {
    unsafe { Retained::retain(view as *const NSView as *mut NSView).unwrap() }
}

fn color_changed(a: [f32; 4], b: [f32; 4]) -> bool {
    // ~1/255 per channel — below this a fade is invisible, so snap.
    (0..4).any(|i| (a[i] - b[i]).abs() > 0.004)
}

fn value_at(t: &Active, now: u64) -> [f32; 4] {
    let elapsed = now.saturating_sub(t.start_ms);
    let p = if t.duration_ms == 0 {
        1.0
    } else {
        (elapsed as f32 / t.duration_ms as f32).clamp(0.0, 1.0)
    };
    let e = ease(t.easing, p);
    lerp(t.from, t.to, e)
}

fn ease(easing: Easing, p: f32) -> f32 {
    // Approximations are fine for color fades — the exact bezier isn't
    // perceptible. CubicBezier falls back to smoothstep.
    match easing {
        Easing::Linear => p,
        Easing::EaseIn => p * p,
        Easing::EaseOut => p * (2.0 - p),
        Easing::Ease | Easing::EaseInOut | Easing::CubicBezier(..) => p * p * (3.0 - 2.0 * p),
    }
}

/// Interpolate two colors in PREMULTIPLIED-alpha space, then un-premultiply.
///
/// A straight per-channel RGBA lerp darkens any fade that touches a transparent
/// endpoint: the framework stores `transparent` as `[0, 0, 0, 0]` (black, α=0),
/// so the midpoint of `transparent → #eef0f7` is `[0.46, 0.47, 0.49, 0.5]` — a
/// half-opaque BLACK that composites to dark gray (the reported "jumps to a dark
/// gray background before the desired color" on hover / sidebar-active fades).
///
/// Premultiplying first keeps the interpolation on the line between the two
/// PREMULTIPLIED endpoints, so a transparent endpoint contributes no color and
/// only the alpha ramps — the midpoint stays the target hue at half alpha and
/// composites to a light tint, exactly how browsers interpolate
/// `transition: background`. Opaque→opaque fades are unaffected (α stays 1, so
/// premultiplied == straight).
fn lerp(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let pa = [a[0] * a[3], a[1] * a[3], a[2] * a[3], a[3]];
    let pb = [b[0] * b[3], b[1] * b[3], b[2] * b[3], b[3]];
    let p = [
        pa[0] + (pb[0] - pa[0]) * t,
        pa[1] + (pb[1] - pa[1]) * t,
        pa[2] + (pb[2] - pa[2]) * t,
        pa[3] + (pb[3] - pa[3]) * t,
    ];
    let alpha = p[3];
    if alpha <= 0.0001 {
        // Fully transparent — RGB is irrelevant; return clear.
        [0.0, 0.0, 0.0, 0.0]
    } else {
        [p[0] / alpha, p[1] / alpha, p[2] / alpha, alpha]
    }
}

#[cfg(test)]
mod lerp_tests {
    use super::lerp;

    // Regression: the transparent→light fade must NOT pass through dark gray.
    #[test]
    fn fade_from_transparent_keeps_target_hue_not_gray() {
        let transparent = [0.0, 0.0, 0.0, 0.0];
        let surface = [0.93, 0.94, 0.97, 1.0]; // #eef0f7
        let mid = lerp(transparent, surface, 0.5);
        // RGB must be the LIGHT target hue, not the ~0.46 dark-gray a straight
        // lerp produced. Alpha ramps from 0 → 1, so it's ~0.5 at the midpoint.
        assert!(mid[0] > 0.9 && mid[1] > 0.9 && mid[2] > 0.9, "midpoint RGB stays light, got {mid:?}");
        assert!((mid[3] - 0.5).abs() < 0.01, "alpha ramps to ~0.5 at midpoint, got {}", mid[3]);
    }

    #[test]
    fn opaque_to_opaque_is_plain_lerp() {
        // Both endpoints opaque (α=1) → premultiplied == straight, no change.
        let a = [0.0, 0.0, 0.0, 1.0];
        let b = [1.0, 0.5, 0.0, 1.0];
        let mid = lerp(a, b, 0.5);
        assert!((mid[0] - 0.5).abs() < 1e-4 && (mid[1] - 0.25).abs() < 1e-4 && (mid[2]).abs() < 1e-4);
        assert!((mid[3] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn endpoints_are_exact() {
        let a = [0.1, 0.2, 0.3, 0.4];
        let b = [0.9, 0.8, 0.7, 1.0];
        let lo = lerp(a, b, 0.0);
        let hi = lerp(a, b, 1.0);
        for i in 0..4 {
            assert!((lo[i] - a[i]).abs() < 1e-4, "t=0 returns `a`");
            assert!((hi[i] - b[i]).abs() < 1e-4, "t=1 returns `b`");
        }
    }
}

fn nscolor(c: [f32; 4]) -> Retained<NSColor> {
    unsafe {
        NSColor::colorWithSRGBRed_green_blue_alpha(
            c[0] as f64,
            c[1] as f64,
            c[2] as f64,
            c[3] as f64,
        )
    }
}

/// Apply an (already-interpolated) color to the native setter for `prop`.
fn set_color(view: &NSView, prop: ColorProp, is_scroll: bool, c: [f32; 4]) {
    match prop {
        ColorProp::Background if is_scroll => {
            let ns = nscolor(c);
            unsafe {
                let _: () = msg_send![view, setDrawsBackground: true];
                let _: () = msg_send![view, setBackgroundColor: &*ns];
                let clip: *mut NSObject = msg_send![view, contentView];
                if !clip.is_null() {
                    let _: () = msg_send![clip, setDrawsBackground: true];
                    let _: () = msg_send![clip, setBackgroundColor: &*ns];
                }
            }
        }
        ColorProp::Background => {
            let ns = nscolor(c);
            unsafe {
                let _: () = msg_send![view, setWantsLayer: true];
                let layer: *mut NSObject = msg_send![view, layer];
                if !layer.is_null() {
                    let cg: CGColorRef = msg_send![&*ns, CGColor];
                    if !cg.0.is_null() {
                        let _: () = msg_send![layer, setBackgroundColor: cg];
                    }
                }
            }
        }
        ColorProp::TextColor => {
            let ns = nscolor(c);
            let _: () = unsafe { msg_send![view, setTextColor: &*ns] };
        }
    }
}

fn ensure_raf() {
    RAF.with(|r| {
        if r.borrow().is_none() {
            let handle = runtime_core::scheduling::raf_loop(tick);
            *r.borrow_mut() = Some(handle);
        }
    });
}

fn tick() {
    let now = now_ms();
    // Snapshot the work so `set_color` (and the raf drop) run without an
    // ACTIVE borrow held.
    let mut frames: Vec<(Retained<NSView>, ColorProp, bool, [f32; 4])> = Vec::new();
    let mut finished: Vec<Key> = Vec::new();
    ACTIVE.with(|a| {
        for (key, t) in a.borrow().iter() {
            let c = value_at(t, now);
            frames.push((t.view.clone(), t.prop, t.is_scroll, c));
            let done = now.saturating_sub(t.start_ms) >= t.duration_ms as u64;
            if done {
                finished.push(*key);
            }
        }
    });
    for (v, p, s, c) in frames {
        set_color(&v, p, s, c);
    }
    if !finished.is_empty() {
        ACTIVE.with(|a| {
            let mut a = a.borrow_mut();
            for k in finished {
                a.remove(&k);
            }
        });
    }
    if ACTIVE.with(|a| a.borrow().is_empty()) {
        // Drop the raf OUTSIDE this tick — dropping the `RafLoop` (which owns
        // this closure) from within would free the running closure. Re-check
        // emptiness in the microtask in case a new transition started.
        runtime_core::schedule_microtask(|| {
            if ACTIVE.with(|a| a.borrow().is_empty()) {
                RAF.with(|r| *r.borrow_mut() = None);
            }
        });
    }
}
