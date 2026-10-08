//! Pure placement decisions for the Android element-anchored portal
//! (`anchored_overlay` → popovers, menus, tooltips, date pickers), kept
//! un-gated so the regression coverage runs from any host
//! (`cargo test -p backend-android-mobile`). The JNI half lives in
//! `imp::primitives::overlay`; it only feeds these functions measurements
//! and applies the result. Same rationale as `layout_policy`.
//!
//! # The bug this replaced
//!
//! The anchored portal used to be an `android.widget.PopupWindow` placed
//! ONCE, at creation, by `anchor_top_left(trigger, side, align, offset,
//! (0, 0))` — content size zero, because nothing had been measured yet. So
//! on Android, alone among the backends:
//!
//! - a `Below` menu opened from a trigger near the bottom of the screen ran
//!   off the bottom edge instead of flipping above (no collision flip),
//! - nothing was clamped into the viewport (no `ANCHOR_EDGE_GAP` gutter),
//! - `Center`/`End` alignment and `Above`/`Start` sides were off by the
//!   content size (they subtract it),
//! - the popup never moved again — not when the content resized (a menu
//!   search filtering rows) and not when the anchor moved (scroll).
//!
//! It also passed `dp` values to `PopupWindow.showAtLocation`, which takes
//! physical pixels.
//!
//! # The model now (mirrors iOS / macOS / web)
//!
//! The anchored portal is a full-bleed overlay `FrameLayout` in the Activity
//! root — the same in-window mechanism the viewport portal uses (no second
//! window; see `project_android_portal_is_dialog_smell`). Its Taffy root is
//! laid out in viewport space with [`anchored_container_rules`] so the
//! content child sizes to its content. Every layout pass, and every frame
//! while the portal is open, the content child's top-left is re-resolved
//! through ONE long-lived [`AnchorTracker`] wrapping the shared
//! [`AnchoredPlacer`] — sticky side, flip only when the content stops
//! fitting, clamp with the shared [`ANCHOR_EDGE_GAP`]. The first placement
//! equals the stateless `resolve_anchored_placement`, exactly like every
//! other backend (CLAUDE.md §7).

use runtime_shared::primitives::portal::{
    AnchoredPlacer, ElementAlign, ElementSide, ViewportRect, ANCHOR_EDGE_GAP,
};
use runtime_shared::{AlignItems, FlexDirection, JustifyContent, StyleRules};

/// Movement below which a re-place is skipped. The per-frame tracker reads
/// the trigger through `getLocationOnScreen` (integer px) and divides by
/// density, so an unmoved trigger can wobble by a fraction of a dp; writing
/// `LayoutParams` for that would request a layout every frame for nothing.
pub(crate) const REPLACE_EPSILON_DP: f32 = 0.5;

/// Taffy style for an anchored portal's full-bleed overlay root: a column
/// that neither stretches nor centers its children, so the content child's
/// Taffy frame is its CONTENT size (the size the placer needs) rather than
/// the viewport width. Matches iOS `container_style_for_anchor`. Only axes
/// the portal's own style left unset are filled in, so an author/framework
/// portal style (e.g. `pointer_events`) is preserved.
// Consumed by `imp` (android-only) and the tests below.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn anchored_container_rules(base: &StyleRules) -> StyleRules {
    let mut rules = base.clone();
    rules.flex_direction.get_or_insert(FlexDirection::Column);
    rules.justify_content.get_or_insert(JustifyContent::FlexStart);
    rules.align_items.get_or_insert(AlignItems::FlexStart);
    rules
}

/// Convert a view's on-screen rect (`getLocationOnScreen` + size, physical
/// px) into the viewport's coordinate space: dp, origin at the top-left of
/// the Activity root the app tree (and every portal overlay) is mounted in.
/// This is the `ViewportRect` contract every backend's `rect()` honors —
/// the anchored placer compares it against the overlay's dp frame and the
/// root's dp size, so all three must share one space.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn screen_px_to_viewport_dp(
    view_px: ViewportRect,
    root_origin_px: (f32, f32),
    density: f32,
) -> ViewportRect {
    let d = if density > 0.0 { density } else { 1.0 };
    ViewportRect {
        x: (view_px.x - root_origin_px.0) / d,
        y: (view_px.y - root_origin_px.1) / d,
        width: view_px.width / d,
        height: view_px.height / d,
    }
}

/// Per-portal placement state: lives exactly as long as the anchored
/// portal (so the placer's settled side survives every re-place) and
/// remembers the last top-left it handed out.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AnchorTracker {
    placer: AnchoredPlacer,
    applied: Option<(f32, f32)>,
}

impl AnchorTracker {
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) fn new(side: ElementSide, align: ElementAlign, offset: f32) -> Self {
        Self { placer: AnchoredPlacer::new(side, align, offset, ANCHOR_EDGE_GAP), applied: None }
    }

    /// Resolve the content child's top-left `(x, y)` in viewport dp.
    ///
    /// `None` when there is nothing meaningful to place against yet: no
    /// trigger rect (its ref isn't filled / it isn't laid out — a zero rect
    /// is the "not measured" sentinel of `AnchorableHandle::rect`), an
    /// unmeasured content child, or an unlaid-out viewport. The caller then
    /// leaves the content where it is, like iOS's tracker skipping a tick.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) fn place(
        &mut self,
        trigger: Option<ViewportRect>,
        content: (f32, f32),
        viewport: (f32, f32),
    ) -> Option<(f32, f32)> {
        let trigger = trigger.filter(|t| t.width > 0.0 || t.height > 0.0)?;
        if content.0 <= 0.0 || content.1 <= 0.0 || viewport.0 <= 0.0 || viewport.1 <= 0.0 {
            return None;
        }
        let p = self.placer.place(trigger, content, viewport);
        self.applied = Some((p.x, p.y));
        Some((p.x, p.y))
    }

    /// [`Self::place`], but `None` when the result is within
    /// [`REPLACE_EPSILON_DP`] of the last placement — the per-frame
    /// tracker's "did the anchor actually move?" gate.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) fn replace_if_moved(
        &mut self,
        trigger: Option<ViewportRect>,
        content: (f32, f32),
        viewport: (f32, f32),
    ) -> Option<(f32, f32)> {
        let before = self.applied;
        let now = self.place(trigger, content, viewport)?;
        match before {
            Some((x, y))
                if (x - now.0).abs() < REPLACE_EPSILON_DP
                    && (y - now.1).abs() < REPLACE_EPSILON_DP =>
            {
                None
            }
            _ => Some(now),
        }
    }

    /// The side the placer has settled on, if it has placed at all.
    #[cfg(test)]
    pub(crate) fn settled_side(&self) -> Option<ElementSide> {
        self.placer.settled_side()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_shared::primitives::portal::{anchor_top_left, resolve_anchored_placement};

    /// A phone-sized viewport in dp.
    const VP: (f32, f32) = (400.0, 800.0);

    /// A trigger near the bottom of the screen (a toolbar "more" button).
    const LOW_TRIGGER: ViewportRect = ViewportRect { x: 300.0, y: 720.0, width: 80.0, height: 40.0 };

    #[test]
    fn regression_android_anchored_popup_never_flips() {
        // Before: the PopupWindow was placed once with content (0, 0), i.e.
        // `anchor_top_left(.., (0, 0))` — a Below menu from a bottom trigger
        // started at y = 764 and its 200dp of rows ran off the 800dp screen.
        let content = (160.0, 200.0);
        let (old_top, old_left) =
            anchor_top_left(LOW_TRIGGER, ElementSide::Below, ElementAlign::Start, 4.0, (0.0, 0.0));
        assert!(old_top + content.1 > VP.1, "the old placement overflowed the bottom edge");
        assert!(old_left + content.0 > VP.0, "…and the right edge (no clamp)");

        // Now: measured content, flipped above, clamped inside the gutter.
        let mut tracker = AnchorTracker::new(ElementSide::Below, ElementAlign::Start, 4.0);
        let (x, y) = tracker.place(Some(LOW_TRIGGER), content, VP).expect("placed");
        assert_eq!(tracker.settled_side(), Some(ElementSide::Above));
        assert_eq!(y, LOW_TRIGGER.y - 4.0 - content.1, "bottom edge sits `offset` above the trigger");
        assert_eq!(x, VP.0 - ANCHOR_EDGE_GAP - content.0, "clamped inside the right gutter");

        // First placement is exactly the stateless shared resolver — the
        // same numbers web / iOS / macOS / Linux produce for this intent.
        let shared = resolve_anchored_placement(
            LOW_TRIGGER, content, VP, ElementSide::Below, ElementAlign::Start, 4.0, ANCHOR_EDGE_GAP,
        );
        assert_eq!((x, y), (shared.x, shared.y));
    }

    #[test]
    fn regression_android_anchored_popup_detaches_when_content_shrinks() {
        // A flipped-above menu whose rows get filtered down must keep its
        // BOTTOM edge on the trigger (stay Above), not jump below it and not
        // keep a stale top that floats away from the trigger.
        let mut tracker = AnchorTracker::new(ElementSide::Below, ElementAlign::Start, 4.0);
        tracker.place(Some(LOW_TRIGGER), (160.0, 200.0), VP).unwrap();
        let (_, y) = tracker.place(Some(LOW_TRIGGER), (160.0, 40.0), VP).unwrap();
        assert_eq!(tracker.settled_side(), Some(ElementSide::Above));
        assert_eq!(y + 40.0, LOW_TRIGGER.y - 4.0, "still attached: bottom edge on the trigger");
    }

    #[test]
    fn replace_if_moved_only_reports_real_movement() {
        let mut tracker = AnchorTracker::new(ElementSide::Below, ElementAlign::Start, 0.0);
        let trigger = ViewportRect { x: 10.0, y: 10.0, width: 50.0, height: 20.0 };
        assert_eq!(tracker.replace_if_moved(Some(trigger), (100.0, 50.0), VP), Some((10.0, 30.0)));
        // Sub-epsilon wobble from integer px → dp: no write.
        let wobble = ViewportRect { y: 10.2, ..trigger };
        assert_eq!(tracker.replace_if_moved(Some(wobble), (100.0, 50.0), VP), None);
        // The anchor scrolled: re-place.
        let scrolled = ViewportRect { y: 110.0, ..trigger };
        assert_eq!(tracker.replace_if_moved(Some(scrolled), (100.0, 50.0), VP), Some((10.0, 130.0)));
    }

    #[test]
    fn unmeasured_inputs_leave_the_content_alone() {
        let mut tracker = AnchorTracker::new(ElementSide::Below, ElementAlign::Start, 0.0);
        let t = ViewportRect { x: 10.0, y: 10.0, width: 50.0, height: 20.0 };
        assert_eq!(tracker.place(None, (100.0, 50.0), VP), None, "anchor ref not filled");
        assert_eq!(tracker.place(Some(ViewportRect::default()), (100.0, 50.0), VP), None);
        assert_eq!(tracker.place(Some(t), (0.0, 0.0), VP), None, "content not laid out");
        assert_eq!(tracker.place(Some(t), (100.0, 50.0), (0.0, 0.0)), None, "viewport not laid out");
        assert_eq!(tracker.settled_side(), None, "nothing settled from unmeasured input");
    }

    #[test]
    fn screen_px_converts_to_root_relative_dp() {
        let px = ViewportRect { x: 330.0, y: 600.0, width: 240.0, height: 120.0 };
        let r = screen_px_to_viewport_dp(px, (30.0, 0.0), 3.0);
        assert_eq!(r, ViewportRect { x: 100.0, y: 200.0, width: 80.0, height: 40.0 });
        // A zero density (failed read) degrades to px rather than inf.
        assert_eq!(screen_px_to_viewport_dp(px, (0.0, 0.0), 0.0), px);
    }

    #[test]
    fn container_rules_fill_only_unset_axes() {
        let r = anchored_container_rules(&StyleRules::default());
        assert_eq!(r.flex_direction, Some(FlexDirection::Column));
        assert_eq!(r.justify_content, Some(JustifyContent::FlexStart));
        assert_eq!(r.align_items, Some(AlignItems::FlexStart));
        let base = StyleRules {
            pointer_events: Some(runtime_shared::PointerEvents::None),
            align_items: Some(AlignItems::Center),
            ..Default::default()
        };
        let r = anchored_container_rules(&base);
        assert_eq!(r.pointer_events, Some(runtime_shared::PointerEvents::None));
        assert_eq!(r.align_items, Some(AlignItems::Center), "explicit portal style wins");
    }
}
