//! `IntrospectionOps::content_clip` on Android, as a decision over what the
//! JNI read collects: whether the view can scroll now
//! (`canScrollVertically` / `canScrollHorizontally`, either direction) and
//! `ViewGroup.getClipChildren()`.
//!
//! A view that can scroll cuts its children at its viewport, but scrolling
//! brings them in ([`ContentClip::Scroll`]). Any other `ViewGroup` clips
//! its children to its bounds unless `clipChildren` was turned off —
//! Android's default is ON, so on this backend a container's overhanging
//! child really is cut ([`ContentClip::Hidden`]). A scroller whose content
//! fits is the same: nothing outside it can be scrolled to. A plain `View`
//! has no children to clip.

use runtime_vocabulary::caps::ContentClip;

/// `can_scroll`: the view can scroll on some axis in some direction.
/// `clip_children`: `getClipChildren()`, `None` for a view that is not a
/// `ViewGroup`.
pub(crate) fn content_clip(can_scroll: bool, clip_children: Option<bool>) -> ContentClip {
    if can_scroll {
        ContentClip::Scroll
    } else if clip_children == Some(true) {
        ContentClip::Hidden
    } else {
        ContentClip::Visible
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scroller_with_somewhere_to_scroll_scrolls() {
        assert_eq!(content_clip(true, Some(true)), ContentClip::Scroll);
    }

    #[test]
    fn a_view_group_clips_unless_told_not_to_and_a_leaf_never_does() {
        assert_eq!(content_clip(false, Some(true)), ContentClip::Hidden);
        assert_eq!(content_clip(false, Some(false)), ContentClip::Visible);
        assert_eq!(content_clip(false, None), ContentClip::Visible);
    }
}
