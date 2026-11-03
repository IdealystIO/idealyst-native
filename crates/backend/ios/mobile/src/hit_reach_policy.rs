//! The robot's two UIKit geometry reads — whether a real tap at a point
//! reaches a node (`IntrospectionOps::hit_contains`) and how a node clips
//! its descendants (`IntrospectionOps::content_clip`) — as pure decisions,
//! kept un-gated so their regression coverage runs from any host. The
//! UIKit walk that feeds them is [`uikit`] (`target_os = "ios"`).
//!
//! ## A tap on a label reaches the label
//!
//! UIKit's `hitTest:withEvent:` never returns a view with
//! `userInteractionEnabled = NO`, and that is the default for `UILabel`
//! and `UIImageView` — every `text` and `image` on this backend. A finger
//! on a text lands on the nearest interactive ancestor instead (the
//! pressable or row around it), which is exactly where the touch is
//! delivered on web too: the text span is the event target there and the
//! press bubbles to the same ancestor. Asking only "is the hit view the
//! node or inside it" therefore answered `false` for every text, and the
//! robot refused `tap_native` on one as "covered" — reported first inside
//! a virtualized list, where the hit view was the row's cell.
//!
//! So when the hit lands on an ANCESTOR of the node, the tap still reaches
//! it when every view from the node up to (not including) that ancestor
//! contains the point and has `userInteractionEnabled = NO`: UIKit skipped
//! them only because they refuse touches, and the touch goes to the
//! ancestor, as it bubbles there on web. An interactive view on that path
//! that UIKit skipped anyway was skipped by `pointer_events: none` (the
//! `IdealystTouchView` hit-test override), which web also treats as "the
//! tap goes through it": not reached. A point outside a view on the path —
//! a row's content overhanging its smaller cell — is not reached either,
//! as UIKit never descends there. Reactive anchors (`IdealystAnchorView`,
//! `display: contents`) are left off the path: their hit test never
//! answers with themselves and ignores their own bounds.

use runtime_vocabulary::caps::ContentClip;

/// Where UIKit's hit test landed, relative to the node asked about.
pub(crate) enum Landing<P> {
    /// Nothing took the point, or a view outside the node's ancestry did.
    Elsewhere,
    /// The node or one of its descendants.
    InNode,
    /// A proper ancestor of the node. `P` walks the views from the node
    /// (included) up to that ancestor (excluded).
    Ancestor(P),
}

/// One view on the path from the node up to the view the hit landed on.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PathView {
    /// `pointInside:withEvent:` at the tapped point.
    pub contains_point: bool,
    /// `userInteractionEnabled`.
    pub interactive: bool,
}

/// Whether a tap whose hit test landed at `landing` reaches the node. See
/// the module docs.
pub(crate) fn reaches<P: IntoIterator<Item = PathView>>(landing: Landing<P>) -> bool {
    match landing {
        Landing::Elsewhere => false,
        Landing::InNode => true,
        Landing::Ancestor(path) => path.into_iter().all(|v| v.contains_point && !v.interactive),
    }
}

/// How a view clips its descendants: a `UIScrollView` (a `scroll_view`,
/// a virtualizer's `UICollectionView`, a `text_area`) scrolls them in; a
/// view whose layer masks to its bounds (`clipsToBounds`, what
/// `overflow: hidden` sets — `backend_apple_core::clip`) cuts them.
pub(crate) fn content_clip(is_scroll_view: bool, masks_to_bounds: bool) -> ContentClip {
    if is_scroll_view {
        ContentClip::Scroll
    } else if masks_to_bounds {
        ContentClip::Hidden
    } else {
        ContentClip::Visible
    }
}

#[cfg(target_os = "ios")]
pub(crate) mod uikit {
    use objc2::{msg_send, ClassType};
    use objc2_foundation::CGPoint;
    use objc2_ui_kit::UIView;
    use runtime_vocabulary::caps::ContentClip;

    use super::{Landing, PathView};

    /// `IntrospectionOps::hit_contains` for `view` at window point `(x, y)`.
    pub(crate) fn hit_contains(view: &UIView, x: f32, y: f32) -> Option<bool> {
        let window: *mut UIView = unsafe { msg_send![view, window] };
        if window.is_null() {
            return None;
        }
        let point = CGPoint { x: x as f64, y: y as f64 };
        let nil: *mut objc2::runtime::AnyObject = std::ptr::null_mut();
        let hit: *mut UIView = unsafe { msg_send![window, hitTest: point, withEvent: nil] };
        let landing = if hit.is_null() {
            Landing::Elsewhere
        } else if unsafe { msg_send![hit, isDescendantOfView: view] } {
            Landing::InNode
        } else if unsafe { msg_send![view, isDescendantOfView: hit] } {
            let anchor = crate::imp::touch::IdealystAnchorView::class();
            let mut path = Vec::new();
            let mut cur: *const UIView = view;
            while !cur.is_null() && cur != hit as *const UIView {
                let is_anchor: bool = unsafe { msg_send![cur, isKindOfClass: anchor] };
                if !is_anchor {
                    let local: CGPoint = unsafe { msg_send![window, convertPoint: point, toView: cur] };
                    path.push(PathView {
                        contains_point: unsafe { msg_send![cur, pointInside: local, withEvent: nil] },
                        interactive: unsafe { msg_send![cur, isUserInteractionEnabled] },
                    });
                }
                cur = unsafe { msg_send![cur, superview] };
            }
            Landing::Ancestor(path)
        } else {
            Landing::Elsewhere
        };
        Some(super::reaches(landing))
    }

    /// `IntrospectionOps::content_clip` for `view`.
    pub(crate) fn content_clip(view: &UIView) -> ContentClip {
        let scroller: bool = unsafe { msg_send![view, isKindOfClass: objc2::class!(UIScrollView)] };
        let masks: bool = unsafe { msg_send![view, clipsToBounds] };
        super::content_clip(scroller, masks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LABEL: PathView = PathView { contains_point: true, interactive: false };
    const PRESSABLE: PathView = PathView { contains_point: true, interactive: true };

    /// Regression: a `text` inside a pressable (a list row's title) was
    /// refused by `tap_native` as covered — UIKit's hit test returns the
    /// pressable, never the non-interactive label, and the old check asked
    /// only whether the hit view was inside the label.
    #[test]
    fn regression_a_tap_on_a_label_reaches_it_through_its_interactive_ancestor() {
        assert!(reaches(Landing::Ancestor([LABEL])));
        // An image and the label around it, both inert.
        assert!(reaches(Landing::Ancestor([LABEL, LABEL])));
    }

    #[test]
    fn a_hit_inside_the_node_reaches_it_and_one_elsewhere_does_not() {
        assert!(reaches(Landing::<[PathView; 0]>::InNode));
        assert!(!reaches(Landing::<[PathView; 0]>::Elsewhere));
    }

    /// Interactive views UIKit skipped anyway were skipped by
    /// `pointer_events: none`: the tap goes through them, as on web.
    #[test]
    fn a_path_through_an_interactive_view_the_hit_test_skipped_is_not_reached() {
        assert!(!reaches(Landing::Ancestor([PRESSABLE])));
        assert!(!reaches(Landing::Ancestor([LABEL, PRESSABLE])));
    }

    /// The node overhangs a view on its path (a row taller than its cell):
    /// UIKit never descends there, so the tap lands on the ancestor and
    /// the node's own recognizers never see it.
    #[test]
    fn a_point_outside_a_view_on_the_path_is_not_reached() {
        assert!(!reaches(Landing::Ancestor([LABEL, PathView { contains_point: false, interactive: false }])));
    }

    #[test]
    fn scrollers_scroll_masks_cut_the_rest_draws_through() {
        assert_eq!(content_clip(true, true), ContentClip::Scroll);
        assert_eq!(content_clip(false, true), ContentClip::Hidden);
        assert_eq!(content_clip(false, false), ContentClip::Visible);
    }
}
