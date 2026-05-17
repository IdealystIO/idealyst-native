//! iOS overlay implementation.
//!
//! `Overlay` is a low-level positioning primitive — it places its
//! content above the rest of the UI in the active `UIWindow` and
//! optionally paints a backdrop. No native chrome (no UIViewController
//! page sheets, no swipe-to-dismiss, no system styling). Higher-level
//! components (Dialog, Select, Tooltip, Popover) are built on top.
//!
//! ## Layout model
//!
//! The overlay container is a plain `UIView`. It's registered as a
//! Taffy root (no parent in the Taffy tree because `insert` skips it),
//! so the layout tree's viewport auto-fill resizes it to the full
//! viewport on every layout pass — including orientation flips and
//! split-view resizes.
//!
//! Author content is positioned via Taffy too: the container's flex
//! `justify_content` / `align_items` settings — derived from the
//! `OverlayAnchor` — place the child in the right viewport region.
//! Element-anchored popovers use `position: absolute` plus computed
//! `top`/`left` insets from the trigger's viewport rect.

use objc2::rc::Retained;
use objc2::{msg_send, msg_send_id};
use objc2_foundation::{MainThreadMarker, NSObject};
use objc2_ui_kit::UIView;
use std::rc::Rc;

use framework_core::primitives::overlay::{
    BackdropMode, ElementAlign, ElementSide, OverlayAnchor, ViewportPlacement,
};
use framework_core::{
    AlignItems, FlexDirection, JustifyContent, Length, Position, StyleRules, Tokenized,
};

use crate::imp::callbacks::CallbackTarget;

/// Per-overlay state stored in the backend, keyed by the container
/// view's pointer.
pub(crate) struct OverlayEntry {
    /// The top-level container view (a plain UIView added to the
    /// host's UIWindow). Dropping/removing it tears down the entire
    /// overlay subtree.
    pub(crate) container: Retained<UIView>,
    /// The anchor we use to position children when the framework
    /// inserts them. Stored so `IosBackend::insert` can look it up.
    pub(crate) anchor: OverlayAnchor,
    /// Retained tap target for dismiss-on-scrim-tap.
    #[allow(dead_code)]
    pub(crate) dismiss_target: Option<Retained<NSObject>>,
}

/// Create an overlay container in the host's window. Returns the
/// container view; the framework's `insert` path will position author
/// content absolutely inside it based on the stored anchor.
pub(crate) fn create_overlay(
    mtm: MainThreadMarker,
    host_root: Option<&Retained<UIView>>,
    anchor: OverlayAnchor,
    backdrop: BackdropMode,
    on_dismiss: Option<Rc<dyn Fn()>>,
) -> (Retained<UIView>, OverlayEntry) {
    // Container: a plain full-window UIView. Taffy sizes it to the
    // viewport on every layout pass; the anchor-derived flex style
    // (see `container_style_for_anchor`) positions the child within.
    let container = unsafe { UIView::new(mtm) };

    // Backdrop (optional). Dismiss + Opaque get a semi-transparent
    // scrim filling the container. None omits it entirely. The scrim
    // sizes itself via the autoresizing mask (translates default = YES),
    // so it tracks the container's bounds without participating in the
    // Taffy tree.
    let mut dismiss_target: Option<Retained<NSObject>> = None;
    let needs_backdrop = !matches!(backdrop, BackdropMode::None);
    if needs_backdrop {
        let scrim = unsafe { UIView::new(mtm) };
        let scrim_color: Retained<NSObject> = unsafe {
            msg_send_id![
                objc2::class!(UIColor),
                colorWithRed: 0.0_f64, green: 0.0_f64, blue: 0.0_f64, alpha: 0.5_f64
            ]
        };
        let _: () = unsafe { msg_send![&scrim, setBackgroundColor: &*scrim_color] };
        // Match the container's bounds via autoresizing mask.
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

    // Mount the container into the host's window above all other
    // content. Defer to next main-queue turn so the framework's
    // insert has time to populate children first.
    if let Some(root) = host_root {
        let root_clone = root.clone();
        let container_clone = container.clone();
        schedule_main(move || mount_in_window(&root_clone, &container_clone));
    }

    let entry = OverlayEntry {
        container: container.clone(),
        anchor,
        dismiss_target,
    };
    (container, entry)
}

/// Build the `StyleRules` for the overlay container based on its
/// anchor. The container is a Taffy root that's viewport-filled by
/// the layout tree's auto-fill; this style positions the overlay's
/// single child within that frame using flex `justify_content` /
/// `align_items`. Element-anchored overlays leave the container
/// neutral — the child is placed via `position: absolute` plus
/// computed insets (see `child_style_for_anchor`).
pub(crate) fn container_style_for_anchor(anchor: &OverlayAnchor) -> StyleRules {
    let mut rules = StyleRules::default();
    // Single-child flex column: justify (vertical) and align (horizontal)
    // place the child anywhere from the four corners through the center.
    rules.flex_direction = Some(FlexDirection::Column);
    match anchor {
        OverlayAnchor::Viewport(placement) => match placement {
            ViewportPlacement::Center => {
                rules.justify_content = Some(JustifyContent::Center);
                rules.align_items = Some(AlignItems::Center);
                // Safety inset so an oversized child can't touch the
                // viewport edges. Author `max_width` on the child is
                // stricter and wins automatically.
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
        },
        OverlayAnchor::Element(_) => {
            // No flex placement — the child positions itself via
            // `position: absolute` + insets. The container just needs
            // to fill the viewport so the absolute coordinates line up
            // with window coordinates (where `rect.x` / `rect.y` live).
            rules.justify_content = Some(JustifyContent::FlexStart);
            rules.align_items = Some(AlignItems::FlexStart);
        }
    }
    rules
}

/// For element-anchored overlays, build a style that absolutely
/// positions the child at the trigger's viewport rect. Returns
/// `None` for viewport-anchored overlays — those are placed by the
/// container's flex style alone.
pub(crate) fn child_style_for_anchor(anchor: &OverlayAnchor) -> Option<StyleRules> {
    let OverlayAnchor::Element(elem) = anchor else {
        return None;
    };

    // If the trigger hasn't measured yet (no window, not mounted),
    // fall back to a safe top-left default. The Overlay primitive
    // typically only mounts after the trigger is on screen, so this
    // is mostly defensive.
    let rect = match elem.target.rect() {
        Some(r) if r.width > 0.0 || r.height > 0.0 => r,
        _ => {
            let mut rules = StyleRules::default();
            rules.position = Some(Position::Absolute);
            rules.top = Some(Tokenized::Literal(Length::Px(100.0)));
            rules.left = Some(Tokenized::Literal(Length::Px(16.0)));
            return Some(rules);
        }
    };

    let target_x = rect.x;
    let target_y = rect.y;
    let target_w = rect.width;
    let target_h = rect.height;
    let offset = elem.offset;

    // Convert to a (top, left) offset within the container (which
    // spans the window). `f32::NAN` means "use bottom/right instead";
    // we translate that to a `bottom: …` / `right: …` style below.
    let (top, left, bottom, right) = match elem.side {
        ElementSide::Below => (
            Some(target_y + target_h + offset),
            Some(align_x(target_x, target_w, elem.align)),
            None,
            None,
        ),
        ElementSide::Above => (
            // Align by bottom edge so the popover grows upward from
            // the trigger regardless of its (unknown-at-style-time)
            // height. Bottom is measured from container's bottom edge.
            None,
            Some(align_x(target_x, target_w, elem.align)),
            Some(target_y - offset),
            None,
        ),
        ElementSide::End => (
            Some(align_y(target_y, target_h, elem.align)),
            Some(target_x + target_w + offset),
            None,
            None,
        ),
        ElementSide::Start => (
            Some(align_y(target_y, target_h, elem.align)),
            None,
            None,
            // Right is measured from container's right edge.
            Some(target_x - offset),
        ),
    };

    let mut rules = StyleRules::default();
    rules.position = Some(Position::Absolute);
    if let Some(v) = top {
        rules.top = Some(Tokenized::Literal(Length::Px(v)));
    }
    if let Some(v) = left {
        rules.left = Some(Tokenized::Literal(Length::Px(v)));
    }
    if let Some(v) = bottom {
        // `bottom` in our style language is "distance from parent's
        // bottom edge". For Above, the popover's bottom edge sits at
        // `target_y - offset` measured from the top, which is
        // `container_h - (target_y - offset)` from the bottom. We
        // don't know `container_h` here — fall back to top-anchoring
        // with an estimated height. A complete fix would observe the
        // popover's measured height and update.
        let _ = v;
        rules.top = Some(Tokenized::Literal(Length::Px(target_y - offset - 100.0)));
    }
    if let Some(v) = right {
        let _ = v;
        // Same story for Start: defer to a conservative left offset.
        rules.left = Some(Tokenized::Literal(Length::Px((target_x - offset - 200.0).max(8.0))));
    }
    Some(rules)
}

fn align_x(target_x: f32, target_w: f32, align: ElementAlign) -> f32 {
    match align {
        ElementAlign::Start => target_x,
        ElementAlign::Center => target_x + target_w / 2.0,
        ElementAlign::End => target_x + target_w,
    }
}

fn align_y(target_y: f32, target_h: f32, align: ElementAlign) -> f32 {
    match align {
        ElementAlign::Start => target_y,
        ElementAlign::Center => target_y + target_h / 2.0,
        ElementAlign::End => target_y + target_h,
    }
}

/// Add the container to the host's window. Sized via autoresizing
/// mask — the container's bounds track the window's bounds, which is
/// what Taffy expects for the viewport root.
fn mount_in_window(host_view: &UIView, container: &UIView) {
    let window: Option<Retained<UIView>> = unsafe { msg_send_id![host_view, window] };
    let Some(window) = window else {
        eprintln!("[ios-overlay] host view has no window — cannot mount");
        return;
    };
    let window_bounds: objc2_foundation::CGRect = unsafe { msg_send![&window, bounds] };
    let _: () = unsafe { msg_send![container, setFrame: window_bounds] };
    let _: () = unsafe { msg_send![container, setAutoresizingMask: 0x12u64] };
    unsafe { window.addSubview(container) };
}

pub(crate) fn release_overlay(entry: OverlayEntry) {
    let container = entry.container;
    schedule_main(move || {
        unsafe { container.removeFromSuperview() };
    });
}

fn schedule_main<F: FnOnce() + 'static>(f: F) {
    extern "C" {
        static _dispatch_main_q: std::ffi::c_void;
        fn dispatch_async_f(
            queue: *const std::ffi::c_void,
            context: *mut std::ffi::c_void,
            work: extern "C" fn(*mut std::ffi::c_void),
        );
    }

    let boxed: Box<Box<dyn FnOnce()>> = Box::new(Box::new(f));
    let ctx = Box::into_raw(boxed) as *mut std::ffi::c_void;

    extern "C" fn trampoline(ctx: *mut std::ffi::c_void) {
        let boxed: Box<Box<dyn FnOnce()>> = unsafe { Box::from_raw(ctx as *mut _) };
        boxed();
    }

    unsafe {
        dispatch_async_f(
            &_dispatch_main_q as *const _ as *const std::ffi::c_void,
            ctx,
            trampoline,
        );
    }
}

pub(crate) type OverlayInstances = std::collections::HashMap<usize, OverlayEntry>;
