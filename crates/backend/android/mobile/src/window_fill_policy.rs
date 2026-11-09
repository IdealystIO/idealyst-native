//! Pure decisions for how a portal overlay reaches the whole window and how
//! `overflow` maps to `clipChildren`, kept un-gated so the regression
//! coverage runs from any host. The JNI half lives in
//! `imp::primitives::overlay` and `imp::style`.
//!
//! # Why a portal needs this on Android
//!
//! The host root sits between the system bars (`RustKeyboardInsets` keeps
//! them as margins), and that root is the viewport every author measures
//! against. iOS and web mount a portal on the window, so a scrim covers the
//! whole screen there. On Android the same scrim stopped at the root's edges
//! and left the status and navigation bars undimmed.
//!
//! The overlay therefore lives on the decor view, the only parent above the
//! opaque bar backgrounds, and fills the window. Its padding is the root's
//! distance from each window edge, so its content keeps viewport coordinates
//! and in-flow content stays between the bars.
//!
//! A child pinned to all four edges of a viewport-filling portal is the one
//! exception: it fills the portal's real extent, the window.

/// How far the host root sits inside the overlay's parent on each side, in
/// `[left, top, right, bottom]` order and device px. Never negative.
pub(crate) fn root_bleed_px(
    root_offset: (i32, i32),
    root_size: (i32, i32),
    parent_size: (i32, i32),
) -> [i32; 4] {
    [
        root_offset.0.max(0),
        root_offset.1.max(0),
        (parent_size.0 - root_offset.0 - root_size.0).max(0),
        (parent_size.1 - root_offset.1 - root_size.1).max(0),
    ]
}

/// Tolerance when comparing a laid-out frame to the viewport, in dp. Taffy
/// rounds frames to whole dp while the viewport keeps its fraction, so a
/// full-bleed child of a 411.43dp viewport comes back 411dp wide.
const VIEWPORT_MATCH_DP: f32 = 1.0;

/// Whether a frame, in dp and relative to its parent, covers the viewport
/// from the parent's origin.
pub(crate) fn covers_viewport(
    (x, y): (f32, f32),
    (width, height): (f32, f32),
    (vw, vh): (f32, f32),
) -> bool {
    x.abs() < VIEWPORT_MATCH_DP
        && y.abs() < VIEWPORT_MATCH_DP
        && (width - vw).abs() <= VIEWPORT_MATCH_DP
        && (height - vh).abs() <= VIEWPORT_MATCH_DP
}

/// The px frame `[x, y, width, height]` of a window-filling child whose
/// parent sits at the viewport origin: back out past the root's offset and
/// span the whole overlay parent.
pub(crate) fn window_fill_frame_px(bleed: [i32; 4], parent_size: (i32, i32)) -> [i32; 4] {
    [-bleed[0], -bleed[1], parent_size.0, parent_size.1]
}

/// The `clipChildren` an explicit `overflow` asks for: `Some(false)` for
/// `Visible`, `Some(true)` for `Hidden`. With no `overflow` stated, a view
/// that had it overridden returns to Android's default of clipping, and any
/// other view is left alone.
///
/// Android's clip is off by one level from `overflow`: a parent's
/// `clipChildren` clips each CHILD to that child's own bounds, so a view's
/// content is only free to overhang it when the view's PARENT turns the clip
/// off. An author chain that must not clip declares `Visible` on every
/// wrapper above the moving view.
pub(crate) fn clip_children_for_overflow(
    overflow: Option<runtime_shared::Overflow>,
    overridden: bool,
) -> Option<bool> {
    use runtime_shared::Overflow;
    match overflow {
        Some(Overflow::Visible) => Some(false),
        Some(Overflow::Hidden) => Some(true),
        None => overridden.then_some(true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_shared::Overflow;

    /// Pixel 6 Pro: 1440x3120, a 145px status bar and an 84px navigation
    /// bar. The scrim stopped at both.
    #[test]
    fn regression_scrim_reaches_past_both_system_bars() {
        let bleed = root_bleed_px((0, 145), (1440, 2891), (1440, 3120));
        assert_eq!(bleed, [0, 145, 0, 84]);
        assert_eq!(window_fill_frame_px(bleed, (1440, 3120)), [0, -145, 1440, 3120]);
    }

    /// 411dp at density 3.5 rounds to 1439px, which left a one-pixel column
    /// of undimmed screen at the right edge. The fill frame is the parent's
    /// px size, so no dp rounding reaches it.
    #[test]
    fn regression_scrim_has_no_rounding_gap_at_a_fractional_density() {
        assert_eq!(window_fill_frame_px([0, 145, 0, 84], (1440, 3120))[2], 1440);
        assert!(covers_viewport((0.0, 0.0), (411.0, 826.0), (411.43, 826.0)));
    }

    /// A root with no system-bar margins (fullscreen, pinned) has nothing to
    /// bleed past, so the fill is the plain viewport.
    #[test]
    fn a_root_filling_the_window_bleeds_nothing() {
        let bleed = root_bleed_px((0, 0), (1440, 3120), (1440, 3120));
        assert_eq!(bleed, [0; 4]);
        assert_eq!(window_fill_frame_px(bleed, (1440, 3120)), [0, 0, 1440, 3120]);
    }

    /// A card, or anything else smaller than the viewport, stays in viewport
    /// coordinates.
    #[test]
    fn only_a_viewport_sized_frame_at_the_origin_counts() {
        let vp = (411.43, 826.0);
        assert!(!covers_viewport((16.0, 300.0), (379.0, 126.0), vp));
        assert!(!covers_viewport((0.0, 0.0), (411.0, 400.0), vp));
        assert!(!covers_viewport((0.0, 40.0), (411.0, 826.0), vp));
    }

    /// `overflow: visible` on a Modal's wrappers is what lets the card slide
    /// as a whole. Android ignored it, so the wrappers' default clip held the
    /// card's bottom edge still and the contents appeared to move inside it.
    #[test]
    fn regression_modal_card_slide_clip_overflow_visible_turns_the_clip_off() {
        assert_eq!(clip_children_for_overflow(Some(Overflow::Visible), false), Some(false));
    }

    #[test]
    fn overflow_hidden_clips_and_an_unstated_overflow_leaves_the_default() {
        assert_eq!(clip_children_for_overflow(Some(Overflow::Hidden), false), Some(true));
        assert_eq!(clip_children_for_overflow(None, false), None);
    }

    /// A reactive style that drops `overflow: visible` must not leave the
    /// view unclipped.
    #[test]
    fn dropping_a_stated_overflow_restores_the_default_clip() {
        assert_eq!(clip_children_for_overflow(None, true), Some(true));
    }
}
