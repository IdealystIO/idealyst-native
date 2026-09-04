//! Pure decision logic for the iOS backend's portal mount + teardown,
//! kept un-gated (no `target_os`) so its regression coverage runs from
//! any host. The objc-driven mount/release in `imp` (`target_os =
//! "ios"`) is pure UIKit plumbing; the two *decisions* it makes are
//! extractable and host-testable, so they live here with unit tests.
//! Same rationale as `splice_policy` and `private_layer_hittest`.
//!
//! ## Decision 1 — which inserted child the anchor tracker pins
//!
//! An overlay composition lowers to an `Element::Portal` whose direct
//! children are `[backdrop, content]` when a backdrop is requested
//! (`overlay()`'s default `BackdropMode::Dismiss`), or just `[content]`
//! when it isn't (the Modal's `BackdropMode::None`). The framework
//! inserts the backdrop FIRST (it must paint behind the content) and
//! the content LAST.
//!
//! For an ANCHORED portal (popover / tooltip / menu) the content child
//! is positioned absolutely against the trigger's viewport rect and a
//! `CADisplayLink` re-pins it each vsync. The earlier code wired that
//! tracker to the FIRST inserted child and then refused to re-wire
//! ("apply tracker treatment only when the entry doesn't already have
//! one"). With a backdrop present that pinned the BACKDROP and left the
//! actual content laid out top-left by the container's neutral flex —
//! the popover rendered in the wrong place (read as "missing"). The
//! fix: re-route the LATEST inserted child through the anchor path on
//! every insert, replacing any prior tracker. Because content is always
//! inserted last, the final tracker pins the content — and a
//! single-child anchored portal still works (its one child is both
//! first and last). See [`anchored_insert_action`].
//!
//! ## Decision 2 — the teardown set
//!
//! Releasing a portal must drop the container's Taffy node AND every
//! descendant view's Taffy node from the backend's `view_to_layout` /
//! `applied_frames` / layout tree — not just `removeFromSuperview` the
//! container. The earlier `release_portal` removed only the
//! `portal_instances` entry and the container's superview link, leaving
//! the container as an orphan Taffy ROOT plus a detached descendant
//! subtree still registered in `view_to_layout`. Every subsequent
//! layout pass then re-computed the dead root and wrote frames into the
//! torn-down subtree forever (a growing leak + stale-layout source).
//! [`teardown_plan`] produces the exact, de-duplicated key set to drop;
//! the test asserts it covers the whole subtree exactly once and never
//! lists a key that wasn't registered.
//!
//! ## Decision 3 — where the anchor tracker pins the popover
//!
//! The `CADisplayLink` callback used to call `anchor_top_left` — the
//! align/side geometry WITHOUT collision handling — while web called
//! `resolve_anchored_placement`, which composes that same math with a
//! side-flip and a viewport clamp. Identical author code therefore
//! placed differently per platform: a menu, tooltip or popover near a
//! screen edge rendered off-screen on iOS and stayed on-screen on web,
//! which is exactly the divergence CLAUDE.md §7 forbids.
//!
//! [`anchor_tracker_placement`] is the tracker's placement decision,
//! host-testable because the geometry it delegates to is pure. The
//! objc callback contributes only the two measurements it feeds in
//! (the popover frame and the container bounds) — and the "is the
//! container laid out yet?" half of that is [`anchor_tracker_viewport`].

use std::collections::HashSet;

use runtime_shared::primitives::portal::{
    resolve_anchored_placement, ElementAlign, ElementSide, ViewportRect,
};

/// Gutter kept between a measured popover and every viewport edge when
/// the clamp kicks in. Matches the web backend's `EDGE_GAP`, because
/// the same author intent must not place differently per platform.
pub(crate) const ANCHOR_EDGE_GAP: f32 = 8.0;

/// What `insert(portal_parent, child)` should do with `child` for an
/// anchored portal, given whether the entry already has a live tracker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AnchoredInsertAction {
    /// Not an anchored portal — `child` flows into the container under
    /// its flex style with no absolute-position treatment or tracker.
    /// (Viewport portals: Modal, sheets, fullscreen overlays.)
    PlainChild,
    /// Anchored portal, and `child` is the first content child to be
    /// wired: apply the absolute-position style and START a new tracker.
    StartTracker,
    /// Anchored portal that ALREADY has a tracker (an earlier child —
    /// typically the backdrop — was wired first): INVALIDATE the prior
    /// tracker and re-wire to THIS (later) child instead. Content is
    /// inserted last, so the final re-wire lands on the content child.
    RetrackToLatest,
}

/// Decide how to route a freshly-inserted portal child.
///
/// `anchor_is_some` — the portal entry is anchored (has an `AnchorSpec`).
/// `tracker_already_live` — the entry already started a `CADisplayLink`.
///
/// Invariant: re-routing to the LATEST child (rather than freezing on
/// the first) is what makes a `[backdrop, content]` anchored portal pin
/// the *content*, not the backdrop. A single-child anchored portal is
/// unaffected: its one child takes `StartTracker`.
// Consumed by `imp::insert` (ios-only) and the tests below.
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub(crate) fn anchored_insert_action(
    anchor_is_some: bool,
    tracker_already_live: bool,
) -> AnchoredInsertAction {
    if !anchor_is_some {
        AnchoredInsertAction::PlainChild
    } else if !tracker_already_live {
        AnchoredInsertAction::StartTracker
    } else {
        AnchoredInsertAction::RetrackToLatest
    }
}

/// Build the de-duplicated set of view-pointer keys whose Taffy node +
/// `view_to_layout` + `applied_frames` entries must be dropped when a
/// portal is released: the container plus every descendant view key
/// reachable through the UIKit subtree.
///
/// `container_key` — the portal container's view pointer.
/// `descendant_keys` — every descendant view pointer, in any order
///   (the objc caller collects these by walking `container.subviews`
///   recursively before detaching anything).
///
/// Returns the keys in a stable order: container first, then
/// descendants in encounter order, each appearing exactly once. The
/// caller removes each exactly once, so a descendant that also happens
/// to equal the container key (it can't in practice, but the dedup
/// guards a future caller mistake) is never freed twice.
// Consumed by `imp::release_portal` (ios-only) and the tests below.
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub(crate) fn teardown_plan(
    container_key: usize,
    descendant_keys: &[usize],
) -> Vec<usize> {
    let mut seen: HashSet<usize> = HashSet::with_capacity(descendant_keys.len() + 1);
    let mut plan: Vec<usize> = Vec::with_capacity(descendant_keys.len() + 1);
    // Container first so the orphan root stops being recomputed before
    // its (already-detached) children are dropped — order is cosmetic
    // for correctness (the whole set is dropped in one pass) but keeps
    // logs readable.
    if seen.insert(container_key) {
        plan.push(container_key);
    }
    for &k in descendant_keys {
        if seen.insert(k) {
            plan.push(k);
        }
    }
    plan
}

/// The viewport the anchor tracker clamps against, from the portal
/// container's `bounds.size`.
///
/// `None` means "not laid out yet — skip this tick". Clamping against
/// a zero viewport pins every popover into the top-left corner, which
/// looks exactly like the off-screen bug this replaced, so the guard
/// has to reject rather than substitute a default.
// Consumed by `imp::start_anchor_tracker` (ios-only) and the tests below.
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub(crate) fn anchor_tracker_viewport(width: f32, height: f32) -> Option<(f32, f32)> {
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    Some((width, height))
}

/// Where the tracker pins the popover's top-left this vsync, as
/// `(top, left)`.
///
/// This is the SAME resolver the web backend calls, deliberately: one
/// definition of side-flip + measured position + viewport clamp lives
/// in `runtime_shared`, and every backend routes through it so the
/// platforms cannot drift (CLAUDE.md §7). The resolved side is dropped
/// here because iOS, like web, positions by origin alone.
// Consumed by `imp::start_anchor_tracker` (ios-only) and the tests below.
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub(crate) fn anchor_tracker_placement(
    trigger: ViewportRect,
    content: (f32, f32),
    viewport: (f32, f32),
    side: ElementSide,
    align: ElementAlign,
    offset: f32,
) -> (f32, f32) {
    let placement =
        resolve_anchored_placement(trigger, content, viewport, side, align, offset, ANCHOR_EDGE_GAP);
    (placement.y, placement.x)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A viewport (non-anchored) portal never starts a tracker — every
    /// child is a plain flex child. Covers the Modal / sheet path.
    #[test]
    fn viewport_portal_child_is_plain() {
        assert_eq!(
            anchored_insert_action(false, false),
            AnchoredInsertAction::PlainChild
        );
        assert_eq!(
            anchored_insert_action(false, true),
            AnchoredInsertAction::PlainChild
        );
    }

    /// A single-child anchored portal: the one (content) child starts
    /// the tracker. No backdrop to confuse it.
    #[test]
    fn anchored_single_child_starts_tracker() {
        assert_eq!(
            anchored_insert_action(true, false),
            AnchoredInsertAction::StartTracker
        );
    }

    /// Regression: a multi-child anchored portal `[backdrop, content]`.
    /// The backdrop is inserted first and starts a tracker; the content
    /// is inserted second and MUST re-track to itself (the latest
    /// child), not be ignored. The pre-fix code returned "skip" for the
    /// second child, leaving the popover pinned to the backdrop and the
    /// content laid out top-left (the "empty / missing content" bug).
    /// Regression: the tracker called `anchor_top_left` — align/side
    /// math with NO side-flip and NO viewport clamp — while web called
    /// `resolve_anchored_placement`, which has both. A menu whose
    /// trigger sits near the right edge therefore rendered off-screen
    /// on iOS and on-screen on web from identical author code.
    ///
    /// The assertion is written so it FAILS against the old call:
    /// `align = Start` without a clamp returns `left == trigger.x`
    /// exactly, so a left origin anywhere else is only reachable
    /// through the clamp. Mirrors the on-device check (an iPhone 17 Pro
    /// Max simulator, a row's menu 32pt from the right edge).
    #[test]
    fn regression_anchor_tracker_clamps_a_popover_into_the_viewport() {
        // 200pt-wide popover under a trigger 32pt from a 440pt-wide
        // screen's right edge: unclamped it starts at x=376 and runs
        // 136pt past the edge.
        let trigger = ViewportRect { x: 376.0, y: 100.0, width: 32.0, height: 32.0 };
        let (_, left) = anchor_tracker_placement(
            trigger,
            (200.0, 120.0),
            (440.0, 956.0),
            ElementSide::Below,
            ElementAlign::Start,
            0.0,
        );

        assert!(
            left < trigger.x,
            "the clamp must pull the popover back from the right edge; \
             unclamped `align = Start` would return left == {} exactly, got {left}",
            trigger.x,
        );
        assert!(
            left + 200.0 <= 440.0 - ANCHOR_EDGE_GAP + f32::EPSILON,
            "the whole popover must sit inside the viewport with the edge gap, \
             got right edge {} against {}",
            left + 200.0,
            440.0 - ANCHOR_EDGE_GAP,
        );
        assert!(left >= ANCHOR_EDGE_GAP - f32::EPSILON, "and not past the left edge either");
    }

    /// A popover with room on every side is placed by the plain
    /// align/side geometry — the clamp must not perturb the common
    /// case, or every centered menu would drift.
    #[test]
    fn anchor_tracker_leaves_a_popover_with_room_alone() {
        let trigger = ViewportRect { x: 100.0, y: 100.0, width: 32.0, height: 32.0 };
        let (top, left) = anchor_tracker_placement(
            trigger,
            (200.0, 120.0),
            (440.0, 956.0),
            ElementSide::Below,
            ElementAlign::Start,
            0.0,
        );
        assert_eq!(left, 100.0, "start-aligned with room: left edge meets the trigger's");
        assert_eq!(top, 132.0, "below with room: top meets the trigger's bottom");
    }

    /// The container's bounds ARE the viewport (it is window-rooted and
    /// the layout tree auto-fills it), but on the first ticks after
    /// mount it has not been laid out. Clamping against `(0, 0)` would
    /// stack every popover in the top-left corner, so a zero-sized
    /// container skips the tick instead of falling back to a default.
    #[test]
    fn an_unlaid_container_yields_no_viewport() {
        assert_eq!(anchor_tracker_viewport(0.0, 0.0), None);
        assert_eq!(anchor_tracker_viewport(440.0, 0.0), None);
        assert_eq!(anchor_tracker_viewport(0.0, 956.0), None);
        assert_eq!(anchor_tracker_viewport(-1.0, 956.0), None);
        assert_eq!(anchor_tracker_viewport(440.0, 956.0), Some((440.0, 956.0)));
    }

    /// The gutter is shared with web's `EDGE_GAP` on purpose: one
    /// author intent, one placement, every backend (CLAUDE.md §7).
    #[test]
    fn the_edge_gap_matches_the_web_backend() {
        assert_eq!(ANCHOR_EDGE_GAP, 8.0);
    }

    #[test]
    fn regression_anchored_multichild_retracks_to_content() {
        // First child (backdrop): no tracker yet → start one.
        let first = anchored_insert_action(true, /* tracker_already_live */ false);
        assert_eq!(first, AnchoredInsertAction::StartTracker);
        // Second child (content): tracker now live → re-track to THIS
        // child, replacing the backdrop tracker.
        let second = anchored_insert_action(true, /* tracker_already_live */ true);
        assert_eq!(
            second, AnchoredInsertAction::RetrackToLatest,
            "the content child (inserted last) must claim the tracker so \
             the popover pins to the anchor, not the backdrop"
        );
    }

    /// The teardown plan lists the container plus every descendant, each
    /// exactly once, in container-first order.
    #[test]
    fn teardown_plan_covers_whole_subtree_once() {
        // container=1, center=2, backdrop=3, card=4 (the Modal shape).
        let plan = teardown_plan(1, &[2, 3, 4]);
        assert_eq!(plan, vec![1, 2, 3, 4]);
    }

    /// Regression: the teardown plan must never list the same key twice,
    /// even if the descendant list contains a duplicate (a defensive
    /// guard against a future caller double-collecting a view). A
    /// double-free of a Taffy slot / `view_to_layout` entry would panic
    /// or corrupt the tree.
    #[test]
    fn regression_teardown_plan_dedups_keys() {
        // Descendant list with a duplicate (3) and the container key (1)
        // erroneously included.
        let plan = teardown_plan(1, &[2, 3, 3, 1, 4]);
        assert_eq!(
            plan,
            vec![1, 2, 3, 4],
            "each key appears exactly once so no Taffy node is freed twice"
        );
    }

    /// An empty portal (no descendants) still drops its container — the
    /// orphan root must not be left behind to be recomputed forever.
    #[test]
    fn teardown_plan_drops_lone_container() {
        assert_eq!(teardown_plan(7, &[]), vec![7]);
    }

    /// Regression: the "modal re-opens with an empty card" symptom. The
    /// card's view pointer (here `4`) MUST be in the teardown set, so the
    /// release path clears its stale `applied_frames` entry. The
    /// allocator recycles freed pointers; if the card's pointer is left
    /// in `applied_frames`, the next view reusing that pointer matches
    /// the cached frame in the layout pass's short-circuit and is never
    /// re-laid-out (stays 0×0 → "empty"). Asserting the card key is
    /// covered is the host-reachable proxy for that UIKit-only failure.
    #[test]
    fn regression_card_key_covered_so_stale_frame_cleared() {
        // Modal subtree: container=1, center=2, backdrop=3, card=4.
        let card_key = 4usize;
        let plan = teardown_plan(1, &[2, 3, card_key]);
        assert!(
            plan.contains(&card_key),
            "the card's pointer must be in the teardown set so its stale \
             applied_frames entry is dropped; otherwise a recycled pointer \
             inherits the dead frame and the re-opened modal's card renders \
             empty (0×0, skipped by the layout pass short-circuit)"
        );
    }
}
