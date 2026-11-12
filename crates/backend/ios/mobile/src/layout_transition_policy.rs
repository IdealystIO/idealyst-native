//! Pure decision logic for animating a LAYOUT change (`transitions {
//! left: … }`, `width`, `top`, a margin or a padding) on iOS.
//!
//! Un-gated (compiles on any host) so the regression tests run from any
//! platform — the pattern of `transform_transition_policy`. The UIKit
//! half (the frame write inside `UIView animateWithDuration:…`) lives in
//! `imp/mod.rs`'s frame pass and consumes these.
//!
//! Why this exists: web animates a `left` or `width` change through CSS
//! `transition`, but the iOS frame pass wrote every Taffy frame as a
//! snap — a drawer that slides on `left` on the web teleported on iOS
//! while its scrim, on an `opacity` transition, faded as designed.
//! Backends diverge in mechanism but converge in output, so a view
//! whose style declares a layout transition takes its next frames
//! inside a UIView animation of that duration and curve.

#![cfg_attr(not(target_os = "ios"), allow(dead_code))]

use runtime_shared::{StyleRules, Transition};

/// The transition a view's frame changes animate with: the longest one
/// its style declares on a layout property (a position inset, a size,
/// a margin or a padding). `None` when it declares none, and the frame
/// pass snaps as before.
///
/// The longest, not the first: a sheet that transitions `width: 240ms`
/// and `margin_left: 150ms` moves as one box on the web, each property
/// on its own clock; one frame write cannot split them, so it takes the
/// outer clock and the box arrives together.
pub(crate) fn layout_transition(style: &StyleRules) -> Option<Transition> {
    [
        style.left_transition,
        style.top_transition,
        style.right_transition,
        style.bottom_transition,
        style.width_transition,
        style.height_transition,
        style.min_width_transition,
        style.min_height_transition,
        style.max_width_transition,
        style.max_height_transition,
        style.margin_top_transition,
        style.margin_right_transition,
        style.margin_bottom_transition,
        style.margin_left_transition,
        style.padding_top_transition,
        style.padding_right_transition,
        style.padding_bottom_transition,
        style.padding_left_transition,
    ]
    .into_iter()
    .flatten()
    .max_by_key(|t| t.duration_ms)
}

/// Should this frame write animate rather than snap?
///
/// - `seen_before`: the view has been framed at least once. The FIRST
///   frame always snaps — CSS parity: a transition fires on a change,
///   never on the initial layout (else every drawer would slide in
///   from the origin at mount).
/// - `keyboard_animating`: the frame pass is already inside the
///   keyboard's own animation block, which carries every frame on the
///   keyboard's clock; a nested block would fight it.
pub(crate) fn should_animate_frame(seen_before: bool, keyboard_animating: bool) -> bool {
    seen_before && !keyboard_animating
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_shared::Easing;

    fn t(ms: u32) -> Option<Transition> {
        Some(Transition::new(ms, Easing::EaseOut))
    }

    #[test]
    fn a_style_with_no_layout_transition_snaps() {
        let mut s = StyleRules::default();
        s.opacity_transition = t(240);
        s.transform_transition = t(240);
        assert_eq!(layout_transition(&s), None, "opacity and transform are not layout");
    }

    #[test]
    fn a_left_transition_arms_the_frame_pass() {
        let mut s = StyleRules::default();
        s.left_transition = t(240);
        assert_eq!(layout_transition(&s), t(240));
    }

    #[test]
    fn the_longest_layout_transition_wins() {
        let mut s = StyleRules::default();
        s.width_transition = t(240);
        s.margin_left_transition = t(150);
        s.padding_top_transition = t(400);
        assert_eq!(layout_transition(&s), t(400));
    }

    #[test]
    fn the_first_frame_and_the_keyboards_frames_snap() {
        assert!(!should_animate_frame(false, false), "a view's first frame never animates");
        assert!(!should_animate_frame(true, true), "the keyboard's block owns its frames");
        assert!(should_animate_frame(true, false));
    }
}
