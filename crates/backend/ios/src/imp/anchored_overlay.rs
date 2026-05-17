//! Element-anchored overlay implementation on iOS — popovers,
//! tooltips, dropdowns, context menus. Anything that follows a
//! trigger element through scrolls / layout reflow / orientation.
//!
//! Viewport-anchored cases (modals, drawers) live in the sibling
//! [`overlay`](super::overlay) module.
//!
//! ## Layout model
//!
//! The container is a viewport-spanning `UIView` mounted into the
//! host's window. It uses the `OverlayPassthroughView` subclass so
//! touches outside the popover's frame fall through to the page
//! beneath — the typical popover UX where the page stays
//! interactive.
//!
//! Initial position comes from the trigger's measured rect at mount
//! time: the child is added as a Taffy node with `position:
//! absolute` plus inset values pointing at the trigger. After mount,
//! a `CADisplayLink` re-pins the child's origin every vsync,
//! tracking the trigger through scrolls / animations. The link is
//! scheduled in `NSRunLoopCommonModes` so it keeps firing during
//! active scroll-view tracking.

use objc2::rc::Retained;
use objc2::{msg_send, msg_send_id};
use objc2_foundation::{MainThreadMarker, NSObject, NSString};
use objc2_ui_kit::UIView;
use std::rc::Rc;

use framework_core::primitives::overlay::{
    AnchorTarget, BackdropMode, ElementAlign, ElementSide, ViewportRect,
};
use framework_core::{Length, Position, StyleRules, Tokenized};

use crate::imp::callbacks::{CallbackTarget, DisplayLinkTarget, OverlayPassthroughView};
use crate::imp::overlay_shared::{mount_in_window, schedule_main};

/// Per-anchored-overlay state stored in the backend, keyed by the
/// container view's pointer.
pub(crate) struct AnchoredOverlayEntry {
    pub(crate) container: Retained<UIView>,
    /// The anchor descriptor — used by `insert` to compute the
    /// child's initial Taffy style and by `start_anchor_tracker` to
    /// re-compute its origin each tick.
    pub(crate) target: AnchorTarget,
    pub(crate) side: ElementSide,
    pub(crate) align: ElementAlign,
    pub(crate) offset: f32,
    #[allow(dead_code)]
    pub(crate) dismiss_target: Option<Retained<NSObject>>,
    /// `CADisplayLink` re-pinning the popover's origin every vsync.
    /// `None` until the first `insert` populates the content view;
    /// `release_anchored_overlay` calls `invalidate` to tear it
    /// down.
    pub(crate) anchor_link: Option<Retained<NSObject>>,
}

pub(crate) type AnchoredOverlayInstances =
    std::collections::HashMap<usize, AnchoredOverlayEntry>;

/// Create an element-anchored overlay container in the host's
/// window. The Taffy positioning style for the (yet-to-be-inserted)
/// content child is computed via [`child_style_for_anchor`] in the
/// caller once the child exists; the tracker is started via
/// [`start_anchor_tracker`] at the same point.
pub(crate) fn create_anchored_overlay(
    mtm: MainThreadMarker,
    host_root: Option<&Retained<UIView>>,
    target: AnchorTarget,
    side: ElementSide,
    align: ElementAlign,
    offset: f32,
    backdrop: BackdropMode,
    on_dismiss: Option<Rc<dyn Fn()>>,
) -> (Retained<UIView>, AnchoredOverlayEntry) {
    // Anchored overlays almost always want a passthrough container —
    // the page beneath stays interactive. A `Dismiss` backdrop is
    // legitimate too (e.g. "select" with a scrim to dim the rest of
    // the page) and gets a plain `UIView` like viewport overlays.
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
                let target_t = CallbackTarget::new(mtm, cb);
                let sel = objc2::sel!(invoke);
                let cls = objc2::class!(UITapGestureRecognizer);
                let alloc: objc2::rc::Allocated<NSObject> = unsafe { msg_send_id![cls, alloc] };
                let tap: Retained<NSObject> = unsafe {
                    msg_send_id![alloc, initWithTarget: &*target_t, action: sel]
                };
                let _: () = unsafe { msg_send![&scrim, addGestureRecognizer: &*tap] };
                let target_obj: Retained<NSObject> = unsafe {
                    let ptr = Retained::as_ptr(&target_t) as *mut NSObject;
                    Retained::retain(ptr).unwrap()
                };
                dismiss_target = Some(target_obj);
            }
        }
    }

    if let Some(root) = host_root {
        let root_clone = root.clone();
        let container_clone = container.clone();
        schedule_main(move || mount_in_window(&root_clone, &container_clone));
    }

    let entry = AnchoredOverlayEntry {
        container: container.clone(),
        target,
        side,
        align,
        offset,
        dismiss_target,
        anchor_link: None,
    };
    (container, entry)
}

/// Container's Taffy style. Neutral flex layout — the child positions
/// itself via `position: absolute` + insets so the container just
/// needs to span the viewport (where the absolute coordinates live).
pub(crate) fn container_style() -> StyleRules {
    use framework_core::{AlignItems, FlexDirection, JustifyContent};
    let mut rules = StyleRules::default();
    rules.flex_direction = Some(FlexDirection::Column);
    rules.justify_content = Some(JustifyContent::FlexStart);
    rules.align_items = Some(AlignItems::FlexStart);
    rules
}

/// Build the absolutely-positioned style for the popover child based
/// on the trigger's current viewport rect. Used at mount time for an
/// initial frame before the display-link tracker takes over.
pub(crate) fn child_style_for_anchor(
    target: &AnchorTarget,
    side: ElementSide,
    align: ElementAlign,
    offset: f32,
) -> StyleRules {
    // If the trigger hasn't measured yet (no window, not mounted),
    // fall back to a safe top-left default. The Overlay primitive
    // typically only mounts after the trigger is on screen, so this
    // is mostly defensive.
    let rect = match target.rect() {
        Some(r) if r.width > 0.0 || r.height > 0.0 => r,
        _ => {
            let mut rules = StyleRules::default();
            rules.position = Some(Position::Absolute);
            rules.top = Some(Tokenized::Literal(Length::Px(100.0)));
            rules.left = Some(Tokenized::Literal(Length::Px(16.0)));
            return rules;
        }
    };

    let mut rules = StyleRules::default();
    rules.position = Some(Position::Absolute);
    // Initial placement uses the static (unmeasured-popover-size)
    // align — Center/End will land slightly off until the tracker's
    // first vsync. That's fine because the tracker runs the same
    // frame and corrects it.
    match side {
        ElementSide::Below => {
            rules.top = Some(Tokenized::Literal(Length::Px(rect.y + rect.height + offset)));
            rules.left = Some(Tokenized::Literal(Length::Px(align_x_unmeasured(&rect, align))));
        }
        ElementSide::Above => {
            // Without the popover's measured height we can't subtract
            // it from `rect.y` — approximate. The tracker fixes this
            // on first vsync once Taffy has sized the popover.
            rules.top = Some(Tokenized::Literal(Length::Px((rect.y - offset - 100.0).max(8.0))));
            rules.left = Some(Tokenized::Literal(Length::Px(align_x_unmeasured(&rect, align))));
        }
        ElementSide::End => {
            rules.top = Some(Tokenized::Literal(Length::Px(align_y_unmeasured(&rect, align))));
            rules.left = Some(Tokenized::Literal(Length::Px(rect.x + rect.width + offset)));
        }
        ElementSide::Start => {
            rules.top = Some(Tokenized::Literal(Length::Px(align_y_unmeasured(&rect, align))));
            rules.left = Some(Tokenized::Literal(Length::Px((rect.x - offset - 200.0).max(8.0))));
        }
    }
    rules
}

fn align_x_unmeasured(rect: &ViewportRect, align: ElementAlign) -> f32 {
    match align {
        ElementAlign::Start => rect.x,
        ElementAlign::Center => rect.x + rect.width / 2.0,
        ElementAlign::End => rect.x + rect.width,
    }
}

fn align_y_unmeasured(rect: &ViewportRect, align: ElementAlign) -> f32 {
    match align {
        ElementAlign::Start => rect.y,
        ElementAlign::Center => rect.y + rect.height / 2.0,
        ElementAlign::End => rect.y + rect.height,
    }
}

/// Start a `CADisplayLink` that re-pins `popover` to the trigger
/// every vsync. Returns the retained link; the caller stores it on
/// the entry so `release_anchored_overlay` can `invalidate` it.
///
/// The link runs in `NSRunLoopCommonModes` so it keeps firing while
/// a UIScrollView ancestor is in active tracking (otherwise the
/// runloop switches into `UITrackingRunLoopMode` and a default-mode
/// link freezes mid-scroll). The mode constant is passed by pointer
/// — CFRunLoop's common-modes set keys on pointer identity of the
/// well-known mode strings; an equal-but-distinct `NSString` is
/// silently accepted but never matched.
pub(crate) fn start_anchor_tracker(
    mtm: MainThreadMarker,
    popover: Retained<UIView>,
    target: AnchorTarget,
    side: ElementSide,
    align: ElementAlign,
    offset: f32,
) -> Retained<NSObject> {
    let popover_for_cb = popover;

    let cb: Rc<dyn Fn()> = Rc::new(move || {
        let Some(trigger) = target.rect() else { return };
        if trigger.width <= 0.0 && trigger.height <= 0.0 {
            return;
        }
        let pop_frame: objc2_foundation::CGRect = unsafe { msg_send![&*popover_for_cb, frame] };
        let pop_w = pop_frame.size.width as f32;
        let pop_h = pop_frame.size.height as f32;
        // Taffy hasn't sized the popover yet — `Above` / `Start`
        // origin math depends on popover size, defer.
        if pop_w <= 0.0 || pop_h <= 0.0 {
            return;
        }

        let (top, left) = compute_anchored_origin(side, align, offset, &trigger, pop_w, pop_h);
        let cur_top = pop_frame.origin.y as f32;
        let cur_left = pop_frame.origin.x as f32;
        // Compare against the *live* frame rather than a stored "last
        // we wrote" value — the framework's layout pass writes
        // setBounds+setCenter on every registered view between ticks,
        // so reading the current frame is what makes us self-correct.
        if (cur_top - top).abs() < 0.5 && (cur_left - left).abs() < 0.5 {
            return;
        }

        let new_frame = objc2_foundation::CGRect {
            origin: objc2_foundation::CGPoint {
                x: left as f64,
                y: top as f64,
            },
            size: pop_frame.size,
        };
        let _: () = unsafe { msg_send![&*popover_for_cb, setFrame: new_frame] };
    });

    let dl_target = DisplayLinkTarget::new(mtm, cb);
    let sel = objc2::sel!(tick:);

    let display_link: Retained<NSObject> = unsafe {
        msg_send_id![
            objc2::class!(CADisplayLink),
            displayLinkWithTarget: &*dl_target,
            selector: sel
        ]
    };

    extern "C" {
        static NSRunLoopCommonModes: *const NSString;
    }
    let run_loop: Retained<NSObject> = unsafe {
        msg_send_id![objc2::class!(NSRunLoop), mainRunLoop]
    };
    let common_modes: &NSString = unsafe { &*NSRunLoopCommonModes };
    let _: () = unsafe {
        msg_send![&*display_link, addToRunLoop: &*run_loop, forMode: common_modes]
    };
    // CADisplayLink retains its target while scheduled; release our
    // local Retained handle — the link's strong reference keeps it
    // alive until `invalidate`.
    let _ = dl_target;

    display_link
}

/// Compute the popover's top-left origin in viewport coordinates,
/// given the trigger's current rect and the popover's measured size.
/// More accurate than the static `align_x_unmeasured` /
/// `align_y_unmeasured` because it accounts for popover dimensions
/// in `Center` / `End` alignment.
fn compute_anchored_origin(
    side: ElementSide,
    align: ElementAlign,
    offset: f32,
    trigger: &ViewportRect,
    pop_w: f32,
    pop_h: f32,
) -> (f32, f32) {
    let tx = trigger.x;
    let ty = trigger.y;
    let tw = trigger.width;
    let th = trigger.height;
    let perp_x = match align {
        ElementAlign::Start => tx,
        ElementAlign::Center => tx + (tw - pop_w) / 2.0,
        ElementAlign::End => tx + tw - pop_w,
    };
    let perp_y = match align {
        ElementAlign::Start => ty,
        ElementAlign::Center => ty + (th - pop_h) / 2.0,
        ElementAlign::End => ty + th - pop_h,
    };
    match side {
        ElementSide::Below => (ty + th + offset, perp_x),
        ElementSide::Above => (ty - pop_h - offset, perp_x),
        ElementSide::End => (perp_y, tx + tw + offset),
        ElementSide::Start => (perp_y, tx - pop_w - offset),
    }
}

pub(crate) fn release_anchored_overlay(entry: AnchoredOverlayEntry) {
    // Stop the tracker first — once invalidated, the display link
    // drops its strong reference to the target/closure, which in
    // turn releases the captured popover view. Doing this before
    // removing the container avoids one final tick firing into a
    // half-torn-down hierarchy.
    if let Some(link) = entry.anchor_link {
        let _: () = unsafe { msg_send![&*link, invalidate] };
    }
    let container = entry.container;
    schedule_main(move || {
        unsafe { container.removeFromSuperview() };
    });
}
