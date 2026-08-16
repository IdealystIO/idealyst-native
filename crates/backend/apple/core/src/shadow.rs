//! Where a view's box shadow has to be painted, shared by the iOS and macOS
//! backends.
//!
//! ## The conflict this resolves
//!
//! CSS draws `box-shadow` and `overflow: hidden` independently: `overflow`
//! clips an element's *descendants*, and never clips the element's own shadow.
//! CoreAnimation has no single-layer expression of that. `masksToBounds`
//! (AppKit) / `clipsToBounds` (UIKit) clips the layer's whole composite, and an
//! outer drop shadow is by definition painted OUTSIDE the bounds, so turning
//! the clip on erases the shadow. Swapping the bounds mask for a `layer.mask`
//! does not help either — a mask layer "defines the part of the parent layer
//! that is visible, and this also affects any shadow rendered by the layer".
//!
//! So an idea-ui `Card` with a rounded, clipped image header rendered a shadow
//! on web and a flat box on both Apple backends, from one author tree — a live
//! Rule #7 divergence, previously papered over by an "iOS caveat" in `Card`'s
//! public docs telling authors to nest two views by hand.
//!
//! ## The fix: paint the shadow on a sibling layer
//!
//! When a style asks for both, the backend synthesizes a second CALayer holding
//! *only* the shadow and inserts it into the **parent's** layer, directly
//! beneath the view's own. Nothing clips it, because it is not a descendant of
//! the masked layer. The view hierarchy is untouched — no extra `NSView` /
//! `UIView` — so child indexing, the Taffy mirror, hit-testing and robot
//! introspection all behave exactly as before.
//!
//! [`shadow_placement`] is the decision, kept here so both backends branch
//! identically and so it is unit-testable on the host (the backends' UIKit /
//! AppKit modules are `cfg`-gated to their target OS, where a `cargo test`
//! never reaches them).

use runtime_shared::StyleRules;

/// Which layer paints a view's `StyleRules.shadow`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadowPlacement {
    /// No shadow. The caller must still CLEAR any shadow a previous apply left
    /// behind — including tearing down a sibling layer — or a reactively
    /// removed shadow keeps painting.
    None,
    /// Paint on the view's own layer. The cheap, ordinary case: one layer, no
    /// extra lifecycle, and CoreAnimation keeps the shadow glued to the view
    /// through animations and transforms for free.
    OwnLayer,
    /// Paint on a synthesized sibling layer in the parent's layer, because the
    /// view's own layer is bounds-masked and would clip its shadow away.
    Sibling,
}

/// Decide where this style's shadow gets painted.
///
/// The `Sibling` branch is deliberately narrow — it costs a second layer, and
/// layers are the dominant per-frame cost when compositing a long scrolling
/// page. Only a view that asks for a shadow *and* clips its children pays it.
pub fn shadow_placement(style: &StyleRules) -> ShadowPlacement {
    if style.shadow.is_none() {
        return ShadowPlacement::None;
    }
    // `Some(false)` (an explicit `overflow: visible`) and `None` (unset) both
    // leave the layer unmasked, so the shadow is safe on the view's own layer.
    if crate::clip::clips_to_bounds(style) == Some(true) {
        ShadowPlacement::Sibling
    } else {
        ShadowPlacement::OwnLayer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_shared::{Color, Overflow, Shadow};

    fn shadow() -> Option<Shadow> {
        Some(Shadow {
            x: 0.0,
            y: 4.0,
            blur: 12.0,
            color: Color::from("rgba(0,0,0,0.25)"),
        })
    }

    #[test]
    fn no_shadow_is_none_whatever_overflow_says() {
        assert_eq!(shadow_placement(&StyleRules::default()), ShadowPlacement::None);
        assert_eq!(
            shadow_placement(&StyleRules {
                overflow: Some(Overflow::Hidden),
                ..Default::default()
            }),
            ShadowPlacement::None,
            "a clipped view with no shadow must not grow a sibling layer",
        );
    }

    /// The ordinary case must stay on one layer. Promoting every shadow to a
    /// sibling would double the layer count of a card-heavy page, and layer
    /// count is what the render server's per-frame cost scales with.
    #[test]
    fn shadow_without_clipping_stays_on_the_views_own_layer() {
        assert_eq!(
            shadow_placement(&StyleRules { shadow: shadow(), ..Default::default() }),
            ShadowPlacement::OwnLayer,
        );
        assert_eq!(
            shadow_placement(&StyleRules {
                shadow: shadow(),
                overflow: Some(Overflow::Visible),
                ..Default::default()
            }),
            ShadowPlacement::OwnLayer,
            "an explicit `overflow: visible` leaves the layer unmasked, so the \
             shadow survives on it",
        );
    }

    /// Regression: `shadow` + `overflow: hidden` on the SAME view rendered no
    /// shadow at all on iOS and macOS, while web rendered both. The bounds mask
    /// clipped the shadow away and the overflow branch ran last, so it always
    /// won. This is the combination that must route to the sibling layer.
    #[test]
    fn regression_shadow_plus_overflow_hidden_needs_a_sibling_layer() {
        assert_eq!(
            shadow_placement(&StyleRules {
                shadow: shadow(),
                overflow: Some(Overflow::Hidden),
                ..Default::default()
            }),
            ShadowPlacement::Sibling,
            "masksToBounds clips an outer drop shadow away, so the shadow has \
             to move off the masked layer entirely",
        );
    }

    /// A rounded card that says nothing about `overflow` is NOT clipped (see
    /// `clip::clips_to_bounds`), so it must not pay for a sibling layer.
    #[test]
    fn border_radius_alone_does_not_force_a_sibling_layer() {
        use runtime_shared::{Length, Tokenized};
        let r = || Some(Tokenized::Literal(Length::Px(12.0)));
        assert_eq!(
            shadow_placement(&StyleRules {
                shadow: shadow(),
                border_top_left_radius: r(),
                border_top_right_radius: r(),
                border_bottom_left_radius: r(),
                border_bottom_right_radius: r(),
                ..Default::default()
            }),
            ShadowPlacement::OwnLayer,
        );
    }
}
