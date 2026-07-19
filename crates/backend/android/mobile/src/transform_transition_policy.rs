//! Pure decision + mapping logic for animating STATIC `transform:`
//! style changes (`transitions { transform: … }`) on Android.
//!
//! Un-gated (compiles on any host) so the regression tests run from
//! any platform — same pattern as this crate's `sticky_compute` /
//! `layout_policy`. The JNI half (`View.animate()` →
//! `ViewPropertyAnimator`) lives in `imp/style.rs` and consumes
//! these.
//!
//! Why this exists: web animates `transform` changes via CSS
//! `transition` (the `ttr` emission), but the Android backend wrote
//! `setTranslationX` etc. directly on every style apply — an
//! `AppShell` drawer that slides on web teleported on Android.
//! Backends diverge in mechanism but converge in output (CLAUDE.md
//! §7).

#![cfg_attr(not(target_os = "android"), allow(dead_code))]

use runtime_core::Easing;

/// Should this static-transform apply animate rather than snap?
/// Same rules as the iOS / macOS siblings (CSS semantics): animate
/// only a *subsequent* apply with a declared transition whose target
/// actually changed. The first apply and identical re-applies snap.
pub(crate) fn should_animate_static_transform(
    seen_before: bool,
    has_transition: bool,
    changed: bool,
) -> bool {
    seen_before && has_transition && changed
}

/// Which `android.view.animation` interpolator class backs each
/// framework easing. `Ease` / `CubicBezier` use `PathInterpolator`
/// (API 21+), constructed with cubic control points — see
/// [`easing_control_points`]; the named curves map to the classic
/// interpolator classes.
pub(crate) enum Interp {
    /// Zero-arg interpolator class, by JNI class path.
    Named(&'static str),
    /// `android/view/animation/PathInterpolator` with `(FFFF)V`
    /// cubic control points.
    Cubic(f32, f32, f32, f32),
}

pub(crate) fn easing_to_interpolator(easing: Easing) -> Interp {
    match easing {
        Easing::Linear => Interp::Named("android/view/animation/LinearInterpolator"),
        Easing::EaseIn => Interp::Named("android/view/animation/AccelerateInterpolator"),
        Easing::EaseOut => Interp::Named("android/view/animation/DecelerateInterpolator"),
        Easing::EaseInOut => {
            Interp::Named("android/view/animation/AccelerateDecelerateInterpolator")
        }
        // CSS `ease` — cubic-bezier(0.25, 0.1, 0.25, 1.0).
        Easing::Ease => Interp::Cubic(0.25, 0.1, 0.25, 1.0),
        Easing::CubicBezier(x1, y1, x2, y2) => Interp::Cubic(x1, y1, x2, y2),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The AppShell-drawer regression: web slides the panel via CSS
    /// `transition: transform`; Android snapped `setTranslationX`.
    #[test]
    fn regression_drawer_slide_animates_only_declared_changes() {
        assert!(should_animate_static_transform(true, true, true));
        // Initial mount: snap, or every drawer mounts with a phantom
        // slide-in from identity.
        assert!(!should_animate_static_transform(false, true, true));
        assert!(!should_animate_static_transform(true, false, true));
        assert!(!should_animate_static_transform(true, true, false));
    }

    #[test]
    fn easing_maps_to_platform_interpolators() {
        assert!(matches!(
            easing_to_interpolator(Easing::EaseOut),
            Interp::Named("android/view/animation/DecelerateInterpolator")
        ));
        assert!(matches!(
            easing_to_interpolator(Easing::CubicBezier(0.2, 0.0, 0.4, 1.0)),
            Interp::Cubic(0.2, 0.0, 0.4, 1.0)
        ));
        // CSS `ease` is a specific cubic, not a named class.
        assert!(matches!(easing_to_interpolator(Easing::Ease), Interp::Cubic(..)));
    }
}
