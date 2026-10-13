//! The `UIViewAnimationOptions` mask for every `+[UIView
//! animateWithDuration:delay:options:animations:completion:]` the iOS
//! backends run: stylesheet transitions (`style::animate`), presence
//! fades/slides, and transform transitions. One mapping, so the three
//! cannot drift apart again.
//!
//! **`AllowUserInteraction` is always set.** Without it UIKit makes every
//! view inside the animation block untouchable for the whole duration.
//! Button carries a 200 ms opacity transition, so a Button REBUILT by a
//! press (a pin pad's backspace the moment its live `disabled` flips)
//! ignored every tap for 200 ms after it appeared, and anything mid-fade
//! or mid-colour-change did the same. CSS transitions never block hit
//! testing, and neither may these (Rule 7). The transform-transition path
//! already set the bit; the style and presence paths did not.
//!
//! Cross-target (no UIKit calls) so the mapping is host-tested.

use runtime_shared::Easing;

/// `UIViewAnimationOptionAllowUserInteraction`.
pub const ALLOW_USER_INTERACTION: u64 = 1 << 1;

/// The curve bits (16–19) for `easing`. `Ease` and `CubicBezier` have no
/// UIKit named curve and approximate to ease-in-out; a custom curve would
/// need `CAMediaTimingFunction` on a `CABasicAnimation`.
pub fn curve_bits(easing: Easing) -> u64 {
    match easing {
        Easing::EaseInOut | Easing::Ease | Easing::CubicBezier(..) => 0 << 16,
        Easing::EaseIn => 1 << 16,
        Easing::EaseOut => 2 << 16,
        Easing::Linear => 3 << 16,
    }
}

/// The full options mask: `easing`'s curve, plus [`ALLOW_USER_INTERACTION`].
pub fn animation_options(easing: Easing) -> u64 {
    curve_bits(easing) | ALLOW_USER_INTERACTION
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Easing; 6] = [
        Easing::Linear,
        Easing::Ease,
        Easing::EaseIn,
        Easing::EaseOut,
        Easing::EaseInOut,
        Easing::CubicBezier(0.1, 0.2, 0.3, 0.4),
    ];

    /// REGRESSION (CrewForge kiosk pin pad, 2026-10-13): stylesheet and
    /// presence transitions animated without `AllowUserInteraction`, so a
    /// view mid-transition — a Button just rebuilt by a press, running its
    /// 200 ms opacity transition — ignored taps. Reproduced in the iOS
    /// simulator: a backspace tap 100–150 ms after the digit that rebuilt
    /// it never reached any view. Every easing must keep views touchable.
    #[test]
    fn regression_every_transition_keeps_views_touchable() {
        for easing in ALL {
            assert_ne!(
                animation_options(easing) & ALLOW_USER_INTERACTION,
                0,
                "{easing:?} must not block touches while it animates"
            );
        }
    }

    /// The curve bits are UIKit's stable ABI; a slip here would silently
    /// flip every transition to the wrong curve on device.
    #[test]
    fn curve_bits_match_uikit() {
        assert_eq!(curve_bits(Easing::EaseInOut), 0);
        assert_eq!(curve_bits(Easing::EaseIn), 1 << 16);
        assert_eq!(curve_bits(Easing::EaseOut), 2 << 16);
        assert_eq!(curve_bits(Easing::Linear), 3 << 16);
        assert_eq!(curve_bits(Easing::Ease), 0, "CSS default approximates ease-in-out");
        assert_eq!(curve_bits(Easing::CubicBezier(0.1, 0.2, 0.3, 0.4)), 0);
    }
}
