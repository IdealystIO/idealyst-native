//! `keyboard_avoiding_view` decisions for the Android backend, kept
//! un-gated so the regression coverage runs from any host — same rationale
//! as `sticky_compute` / `layout_policy`. The JNI half lives in
//! `imp/soft_keyboard.rs`, the per-frame half in `RustKeyboardAvoider.kt`.
//!
//! # Why the work is split this way
//!
//! The frames must come from the system's own IME animation
//! (`WindowInsetsAnimationCompat`), and the per-frame work must be cheap.
//! A layout pass per frame is neither: it re-runs Taffy and re-applies
//! frames over JNI 15-20 times per keyboard move. Instead:
//!
//! - **`Translate`** needs no layout at all: Kotlin sets the view's
//!   `translationY` from the live IME inset each frame.
//! - **`Padding`** lays out ONCE per keyboard move, and Kotlin animates the
//!   difference with `translationY` on just the views that moved (relative
//!   to their parent — a translation carries the subtree with it). A
//!   translation can move views but not resize them, so the size change is
//!   placed where the keyboard hides it — [`padding_plan`].

/// How a `Padding` avoider gets from `from` to `to` dp of keyboard padding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PaddingPlan {
    /// Nothing to do.
    Unchanged,
    /// Apply the final layout now, no animation (`animated = false`, or a
    /// change the system didn't animate).
    Instant,
    /// The content area GROWS (keyboard closing): lay out at the final,
    /// larger size immediately, start every moved view at its OLD position
    /// via `translationY = -dy`, and animate the offsets to 0. The region
    /// that grows is behind the still-visible keyboard.
    GrowNow,
    /// The content area SHRINKS (keyboard opening): keep the current, larger
    /// layout for the whole animation, animate every moved view toward its
    /// final position via `translationY = dy × progress`, and apply the
    /// smaller layout when the animation ends — by then the region that
    /// shrank is behind the keyboard.
    ShrinkAtEnd,
}

/// The rule: during the animation, always lay out at whichever of the two
/// sizes is larger, so a size change is never visible.
pub(crate) fn padding_plan(from: f32, to: f32, animated: bool) -> PaddingPlan {
    if from == to {
        PaddingPlan::Unchanged
    } else if !animated {
        PaddingPlan::Instant
    } else if to < from {
        PaddingPlan::GrowNow
    } else {
        PaddingPlan::ShrinkAtEnd
    }
}

/// Views that move between two layouts, as `(key, dy)`: the change of each
/// view's parent-relative y (Taffy frames are parent-relative, so this is
/// exactly the translation that view needs on top of whatever its parent
/// gets). Views that didn't move are left out — usually all but a handful.
pub(crate) fn moved_views<K: Copy + PartialEq>(
    before: &[(K, f32)],
    after: &[(K, f32)],
) -> Vec<(K, f32)> {
    after
        .iter()
        .filter_map(|(k, y_after)| {
            let (_, y_before) = before.iter().find(|(kb, _)| kb == k)?;
            let dy = y_after - y_before;
            (dy.abs() >= SUB_PIXEL).then_some((*k, dy))
        })
        .collect()
}

/// A moved view's parent-relative top before and after the move, in device
/// px, rounded EXACTLY as the layout pass writes frames
/// (`(y * density).round()` into `RustLayoutApply`), so each equals what
/// `View.getTop()` reports once that layout is in effect. The Kotlin avoider
/// positions views by `desired − getTop()`, so a rounding mismatch here
/// would leave a view off by a pixel — or, with the old "assume the layout
/// already landed" approach, by a whole move (the close-animation jump).
pub(crate) fn tops_px(new_y: f32, dy: f32, density: f32) -> (i32, i32) {
    let px = |dp: f32| (dp * density).round() as i32;
    (px(new_y - dy), px(new_y))
}

/// Moves smaller than this (dp) are rounding noise, not motion.
const SUB_PIXEL: f32 = 0.5;

/// The author-facing timing for an IME move: the system animation's real
/// duration with the IME interpolator's curve.
pub(crate) fn keyboard_transition(duration_ms: i64) -> runtime_shared::Transition {
    runtime_shared::Transition::new(
        duration_ms.clamp(0, u32::MAX as i64) as u32,
        runtime_shared::keyboard::ANDROID_IME_EASING,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression for the visible-size-change problem: an opening keyboard
    /// must NOT shrink the content up front (a blank band would open above
    /// the still-low composer), and a closing one must not wait to grow it
    /// (the list would end above a gap while the keyboard slides away).
    #[test]
    fn regression_size_changes_happen_behind_the_keyboard() {
        assert_eq!(padding_plan(0.0, 300.0, true), PaddingPlan::ShrinkAtEnd, "opening");
        assert_eq!(padding_plan(300.0, 0.0, true), PaddingPlan::GrowNow, "closing");
        assert_eq!(padding_plan(300.0, 250.0, true), PaddingPlan::GrowNow, "keyboard got shorter");
        assert_eq!(padding_plan(0.0, 300.0, false), PaddingPlan::Instant);
        assert_eq!(padding_plan(120.0, 120.0, true), PaddingPlan::Unchanged);
    }

    /// Only views that moved relative to their parent get a translation: a
    /// composer at the bottom of a column moves; the list above it (top
    /// anchored, only shorter) and the composer's own children don't.
    #[test]
    fn only_moved_views_are_translated() {
        let before = [("list", 0.0), ("composer", 760.0), ("field", 8.0)];
        let after = [("list", 0.0), ("composer", 424.0), ("field", 8.0)];
        assert_eq!(moved_views(&before, &after), vec![("composer", -336.0)]);
    }

    #[test]
    fn sub_pixel_noise_is_not_motion_and_new_views_are_ignored() {
        let before = [("a", 10.0)];
        let after = [("a", 10.3), ("b", 50.0)];
        assert!(moved_views(&before, &after).is_empty());
    }

    /// Regression for the close-animation jump: Kotlin compares these tops
    /// with `View.getTop()` to know whether the new layout has landed yet, so
    /// both must round exactly like the frame applier does.
    #[test]
    fn regression_tops_match_the_frame_appliers_rounding() {
        let density = 3.5;
        // A composer moving from y = 424.3 dp (keyboard up) to 760.6 dp.
        let (old, new) = tops_px(760.6, 336.3, density);
        assert_eq!(new, (760.6f32 * density).round() as i32);
        assert_eq!(old, ((760.6f32 - 336.3) * density).round() as i32);
        // Opening (moving up): old is the larger top.
        let (old, new) = tops_px(424.3, -336.3, density);
        assert!(old > new);
    }

    #[test]
    fn transition_uses_the_ime_curve_and_real_duration() {
        let t = keyboard_transition(285);
        assert_eq!(t.duration_ms, 285);
        assert_eq!(t.easing, runtime_shared::keyboard::ANDROID_IME_EASING);
        assert_eq!(keyboard_transition(-5).duration_ms, 0);
    }
}
