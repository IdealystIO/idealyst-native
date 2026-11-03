//! `IntrospectionOps::scroll_port` / `set_scroll_offset` on iOS: any node
//! whose view is a `UIScrollView` (a `scroll_view`, a virtualizer's
//! collection view). A window does not scroll: no `Host` scroller.

use objc2::msg_send;
use objc2_foundation::{CGPoint, CGRect, CGSize};
use runtime_vocabulary::caps::{NativeInjection, ScrollPort, ScrollTarget};

use super::callbacks::UIEdgeInsets;
use super::IosNode;

fn is_scroller(node: &IosNode) -> bool {
    unsafe { msg_send![node.as_view(), isKindOfClass: objc2::class!(UIScrollView)] }
}

/// The offset is `contentOffset`; the range runs to the content's far edge
/// plus the bottom/right `adjustedContentInset` (where UIKit itself stops a
/// drag), less the visible size.
pub(crate) fn scroll_port(node: &IosNode, target: ScrollTarget) -> Option<ScrollPort> {
    if target != ScrollTarget::Node || !is_scroller(node) {
        return None;
    }
    let view = node.as_view();
    let offset: CGPoint = unsafe { msg_send![view, contentOffset] };
    let size: CGSize = unsafe { msg_send![view, contentSize] };
    let bounds: CGRect = unsafe { msg_send![view, bounds] };
    let inset: UIEdgeInsets = unsafe { msg_send![view, adjustedContentInset] };
    Some(ScrollPort {
        x: offset.x as f32,
        y: offset.y as f32,
        max_x: (size.width + inset.right - bounds.size.width).max(0.0) as f32,
        max_y: (size.height + inset.bottom - bounds.size.height).max(0.0) as f32,
        port: None,
    })
}

/// `setContentOffset:` without animation — run as an injection because it
/// fires `scrollViewDidScroll:` (`on_scroll`) synchronously.
pub(crate) fn set_scroll_offset(node: &IosNode, target: ScrollTarget, x: f32, y: f32) -> Option<NativeInjection> {
    scroll_port(node, target)?;
    let node = node.clone();
    Some(Box::new(move || {
        let p = CGPoint { x: x as f64, y: y as f64 };
        let _: () = unsafe { msg_send![node.as_view(), setContentOffset: p] };
        Ok(())
    }))
}
