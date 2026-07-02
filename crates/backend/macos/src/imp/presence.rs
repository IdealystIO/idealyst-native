//! `presence` enter/exit animation for macOS — opacity + 2D translate +
//! uniform scale, the narrow vocabulary [`runtime_core::PresenceState`]
//! exposes (fade / slide / zoom mount-unmount).
//!
//! iOS leans on `UIView.animateWithDuration:`, which implicitly animates the
//! `alpha` / `transform` changes inside its block (see
//! `backend-ios-mobile/src/imp/animated.rs::impl_apply_presence`). AppKit has
//! no equivalent that spans `NSView.alphaValue` (not a CALayer property) AND a
//! layer-backed `NSView`'s transform — and layer-backed views suppress implicit
//! CALayer animations anyway. So we drive presence ourselves with a single
//! raf-clock tween, exactly like the color [`transitions`](crate::imp::transitions)
//! engine: this module is deliberately the same shape.
//!
//! [`apply_presence`] is called by the walker's presence arm at three points
//! (enter snap, animate-to-rest, exit). It diffs against the LAST applied state
//! (tracked here, no native read-back); with a transition set and a changed
//! value it registers a tween and drives it off `raf_loop`, else it snaps. The
//! raf is installed lazily on the first tween and dropped (via a microtask,
//! never from inside its own tick) when the last one finishes.
//!
//! Why no `AnimatedStateMap`: presence writes the layer transform + alpha
//! DIRECTLY (see [`animated::apply_presence_transform`]) rather than through the
//! per-view animated-state cache, so a concurrent `apply_style` (theme swap)
//! restyle can't clobber an in-flight enter/exit and vice versa. Presence is the
//! only thing transforming a presence child for its short mount/unmount window.

use std::cell::RefCell;
use std::collections::HashMap;

use objc2::msg_send;
use objc2::rc::Retained;
use objc2_app_kit::NSView;
use runtime_core::scheduling::RafLoop;
use runtime_core::{Easing, PresenceState};

use crate::imp::animated;

/// The resolved (identity-filled) presence values for a node. A missing
/// [`PresenceState`] field resolves to its resting identity (opacity 1,
/// translate 0, scale 1) — the same resolution iOS does in
/// `impl_apply_presence`.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct Vals {
    opacity: f32,
    tx: f32,
    ty: f32,
    scale: f32,
}

impl Vals {
    /// The resting identity — "no presence override active."
    fn rest() -> Self {
        Self { opacity: 1.0, tx: 0.0, ty: 0.0, scale: 1.0 }
    }

    fn from_state(s: &PresenceState) -> Self {
        Self {
            opacity: s.opacity.unwrap_or(1.0),
            tx: s.translate_x.unwrap_or(0.0),
            ty: s.translate_y.unwrap_or(0.0),
            scale: s.scale.unwrap_or(1.0),
        }
    }

    fn changed(self, other: Self) -> bool {
        (self.opacity - other.opacity).abs() > 0.004
            || (self.tx - other.tx).abs() > 0.05
            || (self.ty - other.ty).abs() > 0.05
            || (self.scale - other.scale).abs() > 0.001
    }
}

struct Active {
    /// Keeps the target view alive for the (short) duration of the tween.
    view: Retained<NSView>,
    from: Vals,
    to: Vals,
    start_ms: u64,
    duration_ms: u32,
    easing: Easing,
}

thread_local! {
    /// Last presence values applied per view — the tween's `from` on the next
    /// change. Avoids reading back native alpha/transform (which may be
    /// mid-animation). Identity until a node first animates.
    static LAST: RefCell<HashMap<usize, Vals>> = RefCell::new(HashMap::new());
    static ACTIVE: RefCell<HashMap<usize, Active>> = RefCell::new(HashMap::new());
    static RAF: RefCell<Option<RafLoop>> = RefCell::new(None);
}

fn now_ms() -> u64 {
    runtime_core::time::now_micros() / 1000
}

/// Apply `state` to `node`'s view, animating over `transition` if one is set and
/// the value actually changed (else snap). Call from the `apply_presence`
/// Backend method.
pub(crate) fn apply_presence(
    view: &NSView,
    state: PresenceState,
    transition: Option<(u32, Easing)>,
) {
    let key = view as *const NSView as usize;
    let to = Vals::from_state(&state);
    let now = now_ms();
    // `from`: the live tween value if one is mid-flight (so a reversal retargets
    // smoothly), else the last applied value, else identity (first apply).
    let from = ACTIVE
        .with(|a| a.borrow().get(&key).map(|t| value_at(t, now)))
        .or_else(|| LAST.with(|l| l.borrow().get(&key).copied()))
        .unwrap_or_else(Vals::rest);
    LAST.with(|l| {
        l.borrow_mut().insert(key, to);
    });

    match transition {
        Some((duration_ms, easing)) if duration_ms > 0 && from.changed(to) => {
            ACTIVE.with(|a| {
                a.borrow_mut().insert(
                    key,
                    Active {
                        view: retain(view),
                        from,
                        to,
                        start_ms: now,
                        duration_ms,
                        easing,
                    },
                );
            });
            ensure_raf();
            // Paint the start frame now so there's no one-frame flash of `to`.
            set_state(view, from);
        }
        _ => {
            ACTIVE.with(|a| {
                a.borrow_mut().remove(&key);
            });
            set_state(view, to);
        }
    }
}

/// Forget a view's presence tween state when it's torn down (`clear_children` /
/// `remove_child`), so a recycled NSView pointer can't inherit a stale `from`.
pub(crate) fn forget_view(view: &NSView) {
    let ptr = view as *const NSView as usize;
    LAST.with(|l| l.borrow_mut().remove(&ptr));
    ACTIVE.with(|a| a.borrow_mut().remove(&ptr));
}

fn retain(view: &NSView) -> Retained<NSView> {
    unsafe { Retained::retain(view as *const NSView as *mut NSView).unwrap() }
}

fn value_at(t: &Active, now: u64) -> Vals {
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
    // Approximations are fine for the short presence fades/slides — the exact
    // bezier isn't perceptible. CubicBezier falls back to smoothstep. Matches
    // the color-transition engine's `ease`.
    match easing {
        Easing::Linear => p,
        Easing::EaseIn => p * p,
        Easing::EaseOut => p * (2.0 - p),
        Easing::Ease | Easing::EaseInOut | Easing::CubicBezier(..) => p * p * (3.0 - 2.0 * p),
    }
}

fn lerp(a: Vals, b: Vals, t: f32) -> Vals {
    Vals {
        opacity: a.opacity + (b.opacity - a.opacity) * t,
        tx: a.tx + (b.tx - a.tx) * t,
        ty: a.ty + (b.ty - a.ty) * t,
        scale: a.scale + (b.scale - a.scale) * t,
    }
}

/// Write a (possibly-interpolated) presence value to the native setters:
/// `alphaValue` for opacity (cascades through subviews, CALayer-independent) and
/// the layer transform for translate + scale (via [`animated::apply_presence_transform`]).
fn set_state(view: &NSView, v: Vals) {
    let _: () = unsafe { msg_send![view, setAlphaValue: v.opacity as f64] };
    animated::apply_presence_transform(view, v.tx as f64, v.ty as f64, v.scale as f64);
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
    // Snapshot the work so `set_state` (and the raf drop) run without an ACTIVE
    // borrow held.
    let mut frames: Vec<(Retained<NSView>, Vals)> = Vec::new();
    let mut finished: Vec<usize> = Vec::new();
    ACTIVE.with(|a| {
        for (key, t) in a.borrow().iter() {
            frames.push((t.view.clone(), value_at(t, now)));
            if now.saturating_sub(t.start_ms) >= t.duration_ms as u64 {
                finished.push(*key);
            }
        }
    });
    for (v, vals) in frames {
        set_state(&v, vals);
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
        // emptiness in the microtask in case a new tween started.
        runtime_core::schedule_microtask(|| {
            if ACTIVE.with(|a| a.borrow().is_empty()) {
                RAF.with(|r| *r.borrow_mut() = None);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    //! Pure tween math — no AppKit. The native writes (`set_state`) need a live
    //! NSView so they're exercised by the robot screenshot pass, not here.
    use super::*;

    #[test]
    fn missing_fields_resolve_to_identity() {
        let v = Vals::from_state(&PresenceState::rest());
        assert_eq!(v.opacity, 1.0);
        assert_eq!(v.tx, 0.0);
        assert_eq!(v.ty, 0.0);
        assert_eq!(v.scale, 1.0);
    }

    #[test]
    fn from_state_reads_set_fields() {
        let s = PresenceState::default().opacity(0.0).translate_y(-8.0);
        let v = Vals::from_state(&s);
        assert_eq!(v.opacity, 0.0);
        assert_eq!(v.ty, -8.0);
        // Unset fields stay at identity.
        assert_eq!(v.tx, 0.0);
        assert_eq!(v.scale, 1.0);
    }

    #[test]
    fn lerp_endpoints_are_exact() {
        let a = Vals { opacity: 0.0, tx: 0.0, ty: -8.0, scale: 0.9 };
        let b = Vals::rest();
        let lo = lerp(a, b, 0.0);
        let hi = lerp(a, b, 1.0);
        assert_eq!(lo.opacity, a.opacity);
        assert_eq!(lo.ty, a.ty);
        assert_eq!(hi.opacity, b.opacity);
        assert_eq!(hi.ty, b.ty);
    }

    #[test]
    fn lerp_midpoint_interpolates() {
        let a = Vals { opacity: 0.0, tx: 0.0, ty: -8.0, scale: 1.0 };
        let b = Vals::rest();
        let mid = lerp(a, b, 0.5);
        assert!((mid.opacity - 0.5).abs() < 1e-4);
        assert!((mid.ty - (-4.0)).abs() < 1e-4);
    }

    #[test]
    fn enter_state_differs_from_rest() {
        // The toast enter/exit state (opacity 0, slide) must register as a
        // change vs rest, else the tween would snap and never animate.
        let enter = Vals::from_state(&PresenceState::default().opacity(0.0).translate_y(-8.0));
        assert!(enter.changed(Vals::rest()));
    }

    #[test]
    fn identical_states_do_not_change() {
        assert!(!Vals::rest().changed(Vals::rest()));
    }

    #[test]
    fn ease_endpoints() {
        for e in [Easing::Linear, Easing::EaseIn, Easing::EaseOut, Easing::EaseInOut] {
            assert!((ease(e, 0.0)).abs() < 1e-6, "{e:?} at 0");
            assert!((ease(e, 1.0) - 1.0).abs() < 1e-6, "{e:?} at 1");
        }
    }
}
