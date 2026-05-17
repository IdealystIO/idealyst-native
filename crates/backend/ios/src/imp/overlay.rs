//! Viewport-anchored overlay implementation on iOS — modals, drawers,
//! full-screen sheets, and edge-pinned banners. Higher-level
//! components (Dialog, BottomSheet) build on top.
//!
//! Element-anchored cases (popovers, tooltips, dropdowns, context
//! menus) live in the sibling [`anchored_overlay`](super::anchored_overlay)
//! module — a different code path because they need active position
//! tracking on scroll/orientation/layout reflow.
//!
//! ## Layout model
//!
//! The overlay container is a plain `UIView` mounted into the host's
//! `UIWindow`. It's registered as a Taffy root (orphan — no parent in
//! the Taffy tree because `insert` skips it), so the layout tree's
//! viewport auto-fill resizes it to the full viewport on every layout
//! pass — including orientation flips and split-view resizes.
//!
//! Author content is positioned via Taffy too: the container's flex
//! `justify_content` / `align_items` settings — derived from the
//! `ViewportPlacement` — place the child in the right viewport
//! region.

use objc2::rc::Retained;
use objc2::{msg_send, msg_send_id};
use objc2_foundation::{MainThreadMarker, NSObject};
use objc2_ui_kit::UIView;
use std::rc::Rc;

use framework_core::primitives::overlay::{BackdropMode, ViewportPlacement};
use framework_core::{
    AlignItems, FlexDirection, JustifyContent, Length, StyleRules, Tokenized,
};

use crate::imp::callbacks::{CallbackTarget, OverlayPassthroughView};
use crate::imp::overlay_shared::{mount_in_window, schedule_main};

/// Per-overlay state stored in the backend, keyed by the container
/// view's pointer.
pub(crate) struct OverlayEntry {
    /// The top-level container view (a plain UIView added to the
    /// host's UIWindow). Dropping/removing it tears down the entire
    /// overlay subtree.
    pub(crate) container: Retained<UIView>,
    /// Retained tap target for dismiss-on-scrim-tap. `None` for
    /// backdrop modes that don't dismiss on outside-tap.
    #[allow(dead_code)]
    pub(crate) dismiss_target: Option<Retained<NSObject>>,
}

pub(crate) type OverlayInstances = std::collections::HashMap<usize, OverlayEntry>;

/// Create a viewport-anchored overlay container in the host's window.
/// Returns the container view; the framework's `insert` path will
/// position author content via Taffy flex inside it.
pub(crate) fn create_overlay(
    mtm: MainThreadMarker,
    host_root: Option<&Retained<UIView>>,
    placement: ViewportPlacement,
    backdrop: BackdropMode,
    on_dismiss: Option<Rc<dyn Fn()>>,
) -> (Retained<UIView>, OverlayEntry) {
    let _ = placement; // captured by `container_style_for_placement` in the caller
    // Container choice depends on backdrop mode:
    //
    // - `BackdropMode::None` → an `OverlayPassthroughView`. Its
    //   overridden `pointInside:` only claims touches that land inside
    //   the content child, so taps on the page beneath continue to
    //   work. Rare for viewport overlays (modals/drawers usually want
    //   a backdrop) but legitimate for e.g. a transparent banner.
    // - `Dismiss` / `Opaque` → a plain `UIView`. The scrim subview
    //   covers the whole container, so it naturally swallows all
    //   touches that don't reach the content.
    let needs_backdrop = !matches!(backdrop, BackdropMode::None);
    let container: Retained<UIView> = if needs_backdrop {
        unsafe { UIView::new(mtm) }
    } else {
        let v = OverlayPassthroughView::new(mtm);
        unsafe { Retained::cast::<UIView>(v) }
    };

    let mut dismiss_target: Option<Retained<NSObject>> = None;
    if needs_backdrop {
        let scrim = unsafe { UIView::new(mtm) };
        let scrim_color: Retained<NSObject> = unsafe {
            msg_send_id![
                objc2::class!(UIColor),
                colorWithRed: 0.0_f64, green: 0.0_f64, blue: 0.0_f64, alpha: 0.5_f64
            ]
        };
        let _: () = unsafe { msg_send![&scrim, setBackgroundColor: &*scrim_color] };
        let _: () = unsafe { msg_send![&scrim, setAutoresizingMask: 0x12u64] };
        let bounds: objc2_foundation::CGRect = unsafe { msg_send![&container, bounds] };
        let _: () = unsafe { msg_send![&scrim, setFrame: bounds] };
        unsafe { container.addSubview(&scrim) };

        if matches!(backdrop, BackdropMode::Dismiss) {
            if let Some(cb) = on_dismiss.clone() {
                let target = CallbackTarget::new(mtm, cb);
                let sel = objc2::sel!(invoke);
                let cls = objc2::class!(UITapGestureRecognizer);
                let alloc: objc2::rc::Allocated<NSObject> = unsafe { msg_send_id![cls, alloc] };
                let tap: Retained<NSObject> = unsafe {
                    msg_send_id![alloc, initWithTarget: &*target, action: sel]
                };
                let _: () = unsafe { msg_send![&scrim, addGestureRecognizer: &*tap] };
                let target_obj: Retained<NSObject> = unsafe {
                    let ptr = Retained::as_ptr(&target) as *mut NSObject;
                    Retained::retain(ptr).unwrap()
                };
                dismiss_target = Some(target_obj);
            }
        }
    }

    // Mount into the window on the next runloop turn — the framework
    // is mid-build and we want the children inserted first.
    if let Some(root) = host_root {
        let root_clone = root.clone();
        let container_clone = container.clone();
        schedule_main(move || mount_in_window(&root_clone, &container_clone));
    }

    let entry = OverlayEntry {
        container: container.clone(),
        dismiss_target,
    };
    (container, entry)
}

/// `StyleRules` for the overlay container that place the single
/// author-supplied child in the right viewport region via flex
/// `justify_content` / `align_items`. The container is a Taffy root
/// (viewport-filled by the layout tree); this is the only style we
/// give it.
pub(crate) fn container_style_for_placement(placement: ViewportPlacement) -> StyleRules {
    let mut rules = StyleRules::default();
    // Column flex: `justify_content` runs vertical, `align_items`
    // runs horizontal. Single-child case so the two together cover
    // every corner + center.
    rules.flex_direction = Some(FlexDirection::Column);
    match placement {
        ViewportPlacement::Center => {
            rules.justify_content = Some(JustifyContent::Center);
            rules.align_items = Some(AlignItems::Center);
            // Safety inset so an oversized child can't touch the
            // viewport edges. Author `max_width` is stricter and wins.
            let inset = Tokenized::Literal(Length::Px(16.0));
            rules.padding_top = Some(inset);
            rules.padding_right = Some(inset);
            rules.padding_bottom = Some(inset);
            rules.padding_left = Some(inset);
        }
        ViewportPlacement::Top => {
            rules.justify_content = Some(JustifyContent::FlexStart);
            rules.align_items = Some(AlignItems::Stretch);
        }
        ViewportPlacement::Bottom => {
            rules.justify_content = Some(JustifyContent::FlexEnd);
            rules.align_items = Some(AlignItems::Stretch);
        }
        ViewportPlacement::Left => {
            rules.justify_content = Some(JustifyContent::FlexStart);
            rules.align_items = Some(AlignItems::FlexStart);
        }
        ViewportPlacement::Right => {
            rules.justify_content = Some(JustifyContent::FlexStart);
            rules.align_items = Some(AlignItems::FlexEnd);
        }
        ViewportPlacement::FullScreen => {
            rules.justify_content = Some(JustifyContent::FlexStart);
            rules.align_items = Some(AlignItems::Stretch);
        }
    }
    rules
}

pub(crate) fn release_overlay(entry: OverlayEntry) {
    let container = entry.container;
    schedule_main(move || {
        unsafe { container.removeFromSuperview() };
    });
}
