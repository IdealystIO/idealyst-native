//! Child-clipping decision shared by the iOS and macOS backends.
//!
//! UIKit spells it `UIView.clipsToBounds`, AppKit spells it
//! `CALayer.masksToBounds`, and both mean the same thing: clip descendant
//! content to the view's (possibly rounded) bounds.
//!
//! ## Clipping follows `overflow`, NEVER `border_radius`
//!
//! This module exists because both Apple backends independently got that
//! wrong. Each one forced the clip ON from inside its corner-radius branch,
//! on the reasoning that "cornerRadius should clip child content". That is
//! not the CSS rule the framework's style vocabulary is modelled on:
//!
//!   * `border-radius` rounds the element's own background and border box.
//!     `CALayer.cornerRadius` already does exactly that, with no mask —
//!     including the CALayer border stroke, which traces the curve cleanly.
//!   * Descendant content is clipped only when `overflow` is not `visible`.
//!
//! The web backend emits a bare `border-radius` and never pairs it with
//! `overflow: hidden`, and idea-ui's `Card` documents *and tests* the same
//! contract — "the default doesn't clip: content may extend past the radius",
//! the friendlier default for overhanging popovers and menus. So the Apple
//! backends were the two odd ones out, breaking the one-author-tree /
//! same-output guarantee (Rule #7).
//!
//! ## Why it also cost real frames
//!
//! `cornerRadius` + a bounds mask on a layer **that has sublayers** forces
//! CoreAnimation off its fast path: it allocates an offscreen buffer, renders
//! the subtree into it, applies the rounded mask, then composites the result.
//! Forcing the mask on every rounded view meant every card, panel and field
//! paid that pass whether or not anything needed clipping — visible as yellow
//! corners under Simulator ▸ Debug ▸ Color Offscreen-Rendered Yellow, and as
//! GPU-bound scroll jank on a content-heavy page.
//!
//! Views that genuinely want an edge-to-edge image header to follow the corner
//! curve still opt in with `overflow: Hidden` and still pay for the mask —
//! that cost is real, requested, and correct.
//!
//! NOT OS-gated — pure `runtime_shared` logic, so it builds and unit-tests on
//! the host while iOS + macOS share one source of truth.

use runtime_shared::{Overflow, StyleRules};

/// Decide whether a view should clip its children.
///
/// * `Some(true)`  — `overflow: hidden`; install the bounds mask.
/// * `Some(false)` — `overflow: visible`; explicitly clear the mask, so a
///   reactive style change from `hidden` back to `visible` actually releases
///   it.
/// * `None`        — the author said nothing about overflow; leave the
///   platform default (unclipped) alone. **A `border_radius` must not
///   promote this to `Some(true)`** — that is the regression this module
///   exists to prevent.
pub fn clips_to_bounds(style: &StyleRules) -> Option<bool> {
    style
        .overflow
        .as_ref()
        .map(|o| matches!(o, Overflow::Hidden))
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_shared::{Length, Tokenized};

    /// A card-shaped style: all four corners rounded, nothing said about
    /// `overflow` — the exact shape that used to get force-clipped.
    fn rounded() -> StyleRules {
        let r = || Some(Tokenized::Literal(Length::Px(12.0)));
        StyleRules {
            border_top_left_radius: r(),
            border_top_right_radius: r(),
            border_bottom_left_radius: r(),
            border_bottom_right_radius: r(),
            ..Default::default()
        }
    }

    /// Regression: both Apple backends forced `clipsToBounds` /
    /// `masksToBounds` ON from inside their corner-radius branch. That
    /// over-clipped relative to CSS and the web backend (a rounded card is
    /// supposed to let children overhang unless `overflow: hidden`), it
    /// contradicted idea-ui `Card`'s documented + tested default, and it cost
    /// an offscreen render pass per rounded view on every composite — the
    /// dominant GPU cost while scrolling a card-heavy page.
    #[test]
    fn regression_border_radius_alone_does_not_clip() {
        assert_eq!(
            clips_to_bounds(&rounded()),
            None,
            "a border_radius with no `overflow` must NOT clip children — \
             clipping follows `overflow`, and forcing it here costs an \
             offscreen render pass on every rounded view",
        );
    }

    #[test]
    fn overflow_hidden_clips_with_or_without_radius() {
        let unrounded = StyleRules {
            overflow: Some(Overflow::Hidden),
            ..Default::default()
        };
        assert_eq!(clips_to_bounds(&unrounded), Some(true));

        let rounded_hidden = StyleRules {
            overflow: Some(Overflow::Hidden),
            ..rounded()
        };
        assert_eq!(
            clips_to_bounds(&rounded_hidden),
            Some(true),
            "an explicit `overflow: hidden` still clips to the rounded box — \
             this is how an edge-to-edge image header follows the corner curve",
        );
    }

    /// `Some(false)` rather than `None` matters: the caller must actively
    /// clear a mask a previous apply installed, or a reactive `hidden` →
    /// `visible` transition would stay clipped forever.
    #[test]
    fn overflow_visible_clears_the_mask() {
        let style = StyleRules {
            overflow: Some(Overflow::Visible),
            ..rounded()
        };
        assert_eq!(clips_to_bounds(&style), Some(false));
    }
}
