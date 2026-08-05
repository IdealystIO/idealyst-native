//! Pure decision + mapping logic for animating STATIC `transform:`
//! style changes (`transitions { transform: … }`) on macOS.
//!
//! Un-gated (compiles on any host) so the regression tests run from
//! any platform — same pattern as this crate's `portal_policy` /
//! `layout_policy`. The AppKit half (`CABasicAnimation` on the
//! layer) lives in `imp/animated.rs` and consumes these.
//!
//! Why this exists: web animates `transform` changes via CSS
//! `transition` (the `ttr` emission), but the macOS backend snapped
//! the layer matrix on every style apply — an `AppShell` drawer that
//! slides on web teleported on macOS. Backends diverge in mechanism
//! but converge in output (CLAUDE.md §7).

#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use runtime_shared::Easing;

/// Should this static-transform apply animate rather than snap?
/// Mirrors `backend-ios-mobile::transform_transition_policy` — the
/// rules are CSS's: animate only a *subsequent* apply (`seen_before`)
/// with a declared transition whose composed value actually
/// `changed`. The first apply and identical re-applies (theme
/// toggles re-firing a cohort) snap.
pub(crate) fn should_animate_static_transform(
    seen_before: bool,
    has_transition: bool,
    changed: bool,
) -> bool {
    seen_before && has_transition && changed
}

/// Map the framework easing to a `CAMediaTimingFunction` name — the
/// documented values behind `kCAMediaTimingFunction*`. `Ease` maps to
/// "default" (Core Animation's ease, closest to CSS `ease`);
/// `CubicBezier` approximates to ease-in-ease-out — the
/// `functionWithControlPoints::::` exact path needs a 4-anonymous-arg
/// selector `msg_send!` can't express cleanly; revisit if an author
/// bezier ever visibly diverges on a sub-300ms UI slide.
pub(crate) fn easing_to_ca_timing_name(easing: Easing) -> &'static str {
    match easing {
        Easing::Linear => "linear",
        Easing::EaseIn => "easeIn",
        Easing::EaseOut => "easeOut",
        Easing::EaseInOut | Easing::CubicBezier(..) => "easeInEaseOut",
        Easing::Ease => "default",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The AppShell-drawer regression: web slides the panel via CSS
    /// `transition: transform`; macOS snapped the layer matrix.
    #[test]
    fn regression_drawer_slide_animates_only_declared_changes() {
        assert!(should_animate_static_transform(true, true, true));
        // Initial mount: snap, or every drawer mounts with a phantom
        // slide-in from identity.
        assert!(!should_animate_static_transform(false, true, true));
        // No transition declared / no actual change: snap.
        assert!(!should_animate_static_transform(true, false, true));
        assert!(!should_animate_static_transform(true, true, false));
    }

    #[test]
    fn easing_maps_to_documented_ca_names() {
        assert_eq!(easing_to_ca_timing_name(Easing::Linear), "linear");
        assert_eq!(easing_to_ca_timing_name(Easing::EaseIn), "easeIn");
        assert_eq!(easing_to_ca_timing_name(Easing::EaseOut), "easeOut");
        assert_eq!(easing_to_ca_timing_name(Easing::EaseInOut), "easeInEaseOut");
        assert_eq!(easing_to_ca_timing_name(Easing::Ease), "default");
    }
}
