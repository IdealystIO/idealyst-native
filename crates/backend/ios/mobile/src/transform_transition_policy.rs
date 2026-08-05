//! Pure decision + mapping logic for animating STATIC `transform:`
//! style changes (`transitions { transform: … }` /
//! `transform_transition`) on iOS.
//!
//! Un-gated (compiles on any host) so the regression tests run from
//! any platform — same pattern as `backend-macos`'s `portal_policy` /
//! `layout_policy`. The UIKit half (`UIView animateWithDuration:…`)
//! lives in `imp/animated.rs` and consumes these.
//!
//! Why this exists: web animates `transform` changes via CSS
//! `transition` (the `ttr` emission), but the iOS backend snapped
//! `setTransform:` on every style apply — an `AppShell` drawer that
//! slides on web teleported on iOS. Backend implementations diverge
//! in mechanism but must converge in output (CLAUDE.md §7), so iOS
//! wraps the same style change in a UIView animation block.

#![cfg_attr(not(target_os = "ios"), allow(dead_code))]

use runtime_shared::Easing;

/// `UIViewAnimationOptions` bits. Curves occupy bits 16-19
/// (`UIViewAnimationOptionCurveEaseInOut` = 0 << 16 … `Linear` =
/// 3 << 16); `AllowUserInteraction` is 1 << 1 — kept ON so the
/// scrim / content stay tappable during a 200-300ms slide, matching
/// CSS transitions (which never block hit testing).
pub(crate) const UIVIEW_ALLOW_USER_INTERACTION: usize = 1 << 1;

/// Map the framework easing to `UIViewAnimationOptions` curve bits
/// (plus `AllowUserInteraction`, see above). `Ease` and
/// `CubicBezier` approximate to UIKit's EaseInOut — UIKit's
/// block-based API has no custom-bezier option; the visual delta on
/// a sub-300ms UI slide is imperceptible. (A future exact path would
/// use `CAMediaTimingFunction` on a CABasicAnimation.)
pub(crate) fn easing_to_uiview_options(easing: Easing) -> usize {
    let curve: usize = match easing {
        Easing::EaseInOut | Easing::Ease | Easing::CubicBezier(..) => 0,
        Easing::EaseIn => 1 << 16,
        Easing::EaseOut => 2 << 16,
        Easing::Linear => 3 << 16,
    };
    curve | UIVIEW_ALLOW_USER_INTERACTION
}

/// Should this static-transform apply animate rather than snap?
///
/// - `seen_before`: a static transform was applied to this view at
///   least once already. The FIRST apply always snaps — CSS parity:
///   a transition fires on a *change*, never on the initial style
///   (else every drawer would visibly slide in from identity at
///   mount).
/// - `has_transition`: the stylesheet declares
///   `transitions { transform: … }`.
/// - `changed`: the composed matrix actually differs — identical
///   re-applies (theme toggles re-firing a cohort, reactive
///   re-styles that only flip colors) must not restart an animation.
pub(crate) fn should_animate_static_transform(
    seen_before: bool,
    has_transition: bool,
    changed: bool,
) -> bool {
    seen_before && has_transition && changed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The AppShell-drawer regression: web slides the panel via CSS
    /// `transition: transform`; iOS snapped. The policy must animate
    /// exactly the "subsequent, declared, actually-changed" apply.
    #[test]
    fn regression_drawer_slide_animates_only_declared_changes() {
        // The open/close flip: seen, declared, changed → animate.
        assert!(should_animate_static_transform(true, true, true));
        // Initial mount (drawer starts closed off-canvas): snap, or
        // every mount would play a phantom slide-in.
        assert!(!should_animate_static_transform(false, true, true));
        // No `transitions { transform }` declared: snap (CSS parity).
        assert!(!should_animate_static_transform(true, false, true));
        // Identical re-apply (e.g. theme toggle re-fires the sheet):
        // no animation restart.
        assert!(!should_animate_static_transform(true, true, false));
    }

    #[test]
    fn easing_maps_to_uikit_curve_bits() {
        // Curve bits per UIViewAnimationOptions; AllowUserInteraction
        // always present so the slide never blocks taps.
        assert_eq!(easing_to_uiview_options(Easing::EaseInOut), UIVIEW_ALLOW_USER_INTERACTION);
        assert_eq!(easing_to_uiview_options(Easing::EaseIn), (1 << 16) | UIVIEW_ALLOW_USER_INTERACTION);
        assert_eq!(easing_to_uiview_options(Easing::EaseOut), (2 << 16) | UIVIEW_ALLOW_USER_INTERACTION);
        assert_eq!(easing_to_uiview_options(Easing::Linear), (3 << 16) | UIVIEW_ALLOW_USER_INTERACTION);
    }
}
