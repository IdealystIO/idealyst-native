//! `position: sticky` for the GTK backend.
//!
//! CSS semantics: the element lays out normally until it would scroll
//! past an edge of its enclosing scroll container, then pins there. The
//! layout engine never sees this — Taffy places the node at its natural
//! position and the backend applies a *visual* offset on top, so the
//! surrounding flow doesn't reflow as the pin engages.
//!
//! ## Model
//!
//! GTK follows the same shape as the macOS backend (see
//! `backend-macos/src/imp/sticky.rs`): a registry of sticky nodes keyed
//! by the enclosing scroll node, driven by the scroll container's own
//! change notification rather than a per-frame poll. Here that's the
//! `GtkScrolledWindow`'s vertical `GtkAdjustment::value-changed`, which
//! fires only when the offset actually moves — strictly cheaper than
//! ticking every vsync, and it's the same signal the `on_scroll` prop
//! already rides.
//!
//! Unlike macOS this moves the node's **transform**, not its frame:
//! `IdealystView` paints children through their stored `GskTransform`
//! and doesn't cull by frame, so there's no equivalent of AppKit purging
//! a transform-pinned view's drawing.
//!
//! ## Scope
//!
//! Vertical leading (`top`) only. This is now the ONLY backend without
//! horizontal or trailing-edge pinning: iOS, macOS, wgpu and Android
//! all read per-edge thresholds via
//! `runtime_shared::sticky::StickyInsets` and pin on `left`, `right`
//! and `bottom` as well, and web has always had all four from the
//! browser. A `left`- or `right`-inset element here silently falls
//! back to relative on the horizontal axis — so a frozen COLUMN does
//! not work on GTK.
//!
//! Bringing this backend in line means widening the registry to
//! `StickyInsets`, summing frame origins on both axes in the
//! natural-position walk, threading the scrolled window's viewport
//! extent + each child's frame extent into the tick (trailing edges
//! measure the far edge), and driving the pin from the scrolled
//! window's `hadjustment` alongside its `vadjustment` — the same
//! change the other four already took. It is deliberately NOT done
//! blind: the GTK toolchain isn't available in the environment this
//! landed from, and CLAUDE.md §5 rules out shipping unverified
//! low-level backend code.
//!
//! With no enclosing scroll container the node falls back to
//! `Relative`, which is what CSS does.
//!
//! Not modelled yet: CSS also confines a sticky element to its
//! *containing block*, so it stops pinning once its parent scrolls away.
//! Deliberately left out — every other backend's v1 omits it too, and
//! adding it here alone would make Linux the odd one out (see the
//! per-backend coverage notes on `runtime_shared::Position::Sticky`).

/// How far to shift a sticky node along Y, given its natural position in
/// the scroll container's content space, the container's current scroll
/// offset, and the `top` threshold it pins at.
///
/// Returns `0.0` while the node hasn't reached the threshold — it rides
/// the content normally — and grows as the content scrolls further, so
/// the node appears parked `top` px below the viewport's top edge.
///
/// Never negative: a `top` sticky pins only when scrolling *down* past
/// it and must never drag the node above where the layout put it (CSS
/// clamps the same way).
pub fn pin_offset(content_y: f32, scroll_offset: f32, top: f32) -> f32 {
    (top + scroll_offset - content_y).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::pin_offset;

    #[test]
    fn unscrolled_content_is_not_pinned() {
        // Node sits 500px down a container that hasn't scrolled: it's far
        // below the pin line, so it rides the content untouched.
        assert_eq!(pin_offset(500.0, 0.0, 0.0), 0.0);
        assert_eq!(pin_offset(500.0, 0.0, 16.0), 0.0);
    }

    #[test]
    fn pins_once_scrolled_past_the_threshold() {
        // Scrolled 600px: the node's natural spot (500) is now 100px
        // above the viewport top, so it must be pushed back down 100px to
        // sit at the edge.
        assert_eq!(pin_offset(500.0, 600.0, 0.0), 100.0);
        // With a 16px threshold it parks 16px below the edge instead.
        assert_eq!(pin_offset(500.0, 600.0, 16.0), 116.0);
    }

    #[test]
    fn engages_exactly_at_the_threshold() {
        // Boundary: scrolled precisely to the node's position.
        assert_eq!(pin_offset(500.0, 500.0, 0.0), 0.0);
        // One pixel further and it starts pinning.
        assert_eq!(pin_offset(500.0, 501.0, 0.0), 1.0);
    }

    #[test]
    fn never_lifts_a_node_above_its_natural_position() {
        // Scrolling "up" past the node (or a node below the fold) must
        // not yield a negative offset — that would hoist it out of flow.
        assert_eq!(pin_offset(500.0, 0.0, -100.0), 0.0);
        assert_eq!(pin_offset(1000.0, 10.0, 0.0), 0.0);
    }

    #[test]
    fn tracks_scrolling_one_for_one_while_pinned() {
        // Once engaged the offset must grow exactly with the scroll, or
        // the node drifts instead of staying parked.
        let a = pin_offset(100.0, 300.0, 8.0);
        let b = pin_offset(100.0, 340.0, 8.0);
        assert_eq!(b - a, 40.0);
    }
}
