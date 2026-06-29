//! Pure decision logic for the macOS backend's anchored-portal mount,
//! kept un-gated (no `target_os`) so its regression coverage runs from
//! any host. The objc-driven mount in `imp::portal` (`target_os =
//! "macos"`) is pure AppKit plumbing; the one *decision* it makes —
//! which inserted child the anchor tracker pins — is extractable and
//! host-testable, so it lives here with unit tests. Mirrors the iOS
//! `portal_policy` (same rationale as `private_layer_hittest`).
//!
//! ## Which inserted child the anchor tracker pins
//!
//! An overlay composition lowers to an `Element::Portal` whose direct
//! children are `[backdrop, content]` when a backdrop / click-away
//! catcher is requested (popover), or just `[content]` when it isn't
//! (tooltip). The framework inserts the backdrop FIRST (it must paint
//! behind the content) and the content LAST.
//!
//! For an ANCHORED portal the content child is positioned absolutely
//! against the trigger's viewport rect and a `raf_loop` re-pins it each
//! frame. Wiring the tracker to the FIRST inserted child would pin the
//! BACKDROP for a `[backdrop, content]` popover and leave the actual
//! content laid out top-left by the container's neutral flex (the
//! "popover renders in the wrong place" bug). So the LATEST inserted
//! child claims the tracker on every insert, replacing any prior one.
//! Content is always inserted last, so the final tracker pins the
//! content — and a single-child anchored portal still works (its one
//! child is both first and last). See [`anchored_insert_action`].

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
    /// typically the backdrop — was wired first): STOP the prior tracker
    /// and re-wire to THIS (later) child instead. Content is inserted
    /// last, so the final re-wire lands on the content child.
    RetrackToLatest,
}

/// Decide how to route a freshly-inserted portal child.
///
/// `anchor_is_some` — the portal entry is anchored (has an `AnchorSpec`).
/// `tracker_already_live` — the entry already started a tracker.
///
/// Invariant: re-routing to the LATEST child (rather than freezing on
/// the first) is what makes a `[backdrop, content]` anchored portal pin
/// the *content*, not the backdrop. A single-child anchored portal is
/// unaffected: its one child takes `StartTracker`.
// Consumed by `imp::insert` (macos-only) and the tests below.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A viewport (non-anchored) portal never starts a tracker — every
    /// child is a plain flex child. Covers the Modal / sheet path.
    #[test]
    fn viewport_portal_child_is_plain() {
        assert_eq!(anchored_insert_action(false, false), AnchoredInsertAction::PlainChild);
        assert_eq!(anchored_insert_action(false, true), AnchoredInsertAction::PlainChild);
    }

    /// A single-child anchored portal (tooltip): the one (content) child
    /// starts the tracker. No backdrop to confuse it.
    #[test]
    fn anchored_single_child_starts_tracker() {
        assert_eq!(anchored_insert_action(true, false), AnchoredInsertAction::StartTracker);
    }

    /// Regression: a multi-child anchored portal `[backdrop, content]`
    /// (popover with a click-away catcher). The backdrop is inserted
    /// first and starts a tracker; the content is inserted second and
    /// MUST re-track to itself (the latest child), not be ignored —
    /// otherwise the popover pins to the backdrop and the content lays
    /// out top-left (the reported "renders top-left" bug).
    #[test]
    fn regression_anchored_multichild_retracks_to_content() {
        let first = anchored_insert_action(true, /* tracker_already_live */ false);
        assert_eq!(first, AnchoredInsertAction::StartTracker);
        let second = anchored_insert_action(true, /* tracker_already_live */ true);
        assert_eq!(
            second, AnchoredInsertAction::RetrackToLatest,
            "the content child (inserted last) must claim the tracker so the \
             popover pins to the anchor, not the backdrop"
        );
    }
}
