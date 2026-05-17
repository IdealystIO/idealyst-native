//! iOS overlay implementation.
//!
//! `Overlay` is a low-level positioning primitive — it places its
//! content above the rest of the UI in the active `UIWindow` and
//! optionally paints a backdrop. No native chrome (no UIViewController
//! page sheets, no swipe-to-dismiss, no system styling). Higher-level
//! components (Dialog, Select, Tooltip, Popover) are built on top.
//!
//! ## Layout model: absolute positioning
//!
//! The overlay container is a plain `UIView` (not a stack). Author
//! content is positioned ABSOLUTELY within it based on the
//! `OverlayAnchor`:
//!
//! - `Viewport(Center)` — content centered in the viewport
//! - `Viewport(Top|Bottom|Left|Right)` — pinned to an edge
//! - `Viewport(FullScreen)` — fills the viewport
//! - `Element(rect)` — anchored to a referenced element's rect (TODO)
//!
//! Absolute positioning avoids `UIStackView`'s `UISV-canvas-connection`
//! constraints, which fight with overlay positioning, and naturally
//! supports element-anchored popovers (set `topAnchor`/`leadingAnchor`
//! constants from the trigger's screen rect).

use objc2::rc::Retained;
use objc2::{msg_send, msg_send_id};
use objc2_foundation::{MainThreadMarker, NSObject};
use objc2_ui_kit::UIView;
use std::rc::Rc;

use framework_core::primitives::overlay::{
    BackdropMode, ElementAlign, ElementAnchor, ElementSide, OverlayAnchor, ViewportPlacement,
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
    // Container: a plain full-window UIView. Author content is
    // positioned absolutely as a subview.
    let container = unsafe { UIView::new(mtm) };
    let _: () = unsafe {
        msg_send![&container, setTranslatesAutoresizingMaskIntoConstraints: false]
    };

    // Backdrop (optional). Dismiss + Opaque get a semi-transparent
    // scrim filling the container. None omits it entirely.
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
        let _: () = unsafe {
            msg_send![&scrim, setTranslatesAutoresizingMaskIntoConstraints: false]
        };
        unsafe { container.addSubview(&scrim) };
        pin_to_edges(&container, &scrim);

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

/// Position a child view absolutely inside an overlay container,
/// based on the overlay's anchor. Called by `IosBackend::insert` when
/// the parent is an overlay container.
pub(crate) fn apply_anchor_to_child(
    container: &UIView,
    child: &UIView,
    anchor: &OverlayAnchor,
) {
    let _: () = unsafe {
        msg_send![child, setTranslatesAutoresizingMaskIntoConstraints: false]
    };

    let c_top: Retained<NSObject> = unsafe { msg_send_id![container, topAnchor] };
    let c_bot: Retained<NSObject> = unsafe { msg_send_id![container, bottomAnchor] };
    let c_lead: Retained<NSObject> = unsafe { msg_send_id![container, leadingAnchor] };
    let c_trail: Retained<NSObject> = unsafe { msg_send_id![container, trailingAnchor] };
    let c_cx: Retained<NSObject> = unsafe { msg_send_id![container, centerXAnchor] };
    let c_cy: Retained<NSObject> = unsafe { msg_send_id![container, centerYAnchor] };

    let v_top: Retained<NSObject> = unsafe { msg_send_id![child, topAnchor] };
    let v_bot: Retained<NSObject> = unsafe { msg_send_id![child, bottomAnchor] };
    let v_lead: Retained<NSObject> = unsafe { msg_send_id![child, leadingAnchor] };
    let v_trail: Retained<NSObject> = unsafe { msg_send_id![child, trailingAnchor] };
    let v_cx: Retained<NSObject> = unsafe { msg_send_id![child, centerXAnchor] };
    let v_cy: Retained<NSObject> = unsafe { msg_send_id![child, centerYAnchor] };

    let activate = |a: &Retained<NSObject>, b: &Retained<NSObject>| {
        let c: Retained<NSObject> = unsafe { msg_send_id![a, constraintEqualToAnchor: &**b] };
        let _: () = unsafe { msg_send![&c, setActive: true] };
    };
    // For viewport-anchored modes that don't pin both edges, add a
    // less-than-or-equal width/height to the container so content
    // can't grow beyond the screen. Author stylesheets can still set
    // smaller widths (e.g. `max_width: 520`) — those are stricter and
    // win automatically.
    let cap = |child_anchor: &Retained<NSObject>, container_anchor: &Retained<NSObject>, inset: f64| {
        let c: Retained<NSObject> = unsafe {
            msg_send_id![
                child_anchor,
                constraintLessThanOrEqualToAnchor: &**container_anchor,
                constant: -inset
            ]
        };
        let _: () = unsafe { msg_send![&c, setActive: true] };
    };

    let v_width: Retained<NSObject> = unsafe { msg_send_id![child, widthAnchor] };
    let v_height: Retained<NSObject> = unsafe { msg_send_id![child, heightAnchor] };
    let c_width: Retained<NSObject> = unsafe { msg_send_id![container, widthAnchor] };
    let c_height: Retained<NSObject> = unsafe { msg_send_id![container, heightAnchor] };

    match anchor {
        OverlayAnchor::Viewport(placement) => match placement {
            ViewportPlacement::Center => {
                activate(&v_cx, &c_cx);
                activate(&v_cy, &c_cy);
                // Safety caps: child width/height ≤ container − 32px
                // total padding. Author stylesheet `max_width` is
                // stricter and overrides automatically.
                cap(&v_width, &c_width, 32.0);
                cap(&v_height, &c_height, 32.0);
            }
            ViewportPlacement::Top => {
                activate(&v_top, &c_top);
                activate(&v_lead, &c_lead);
                activate(&v_trail, &c_trail);
            }
            ViewportPlacement::Bottom => {
                activate(&v_bot, &c_bot);
                activate(&v_lead, &c_lead);
                activate(&v_trail, &c_trail);
            }
            ViewportPlacement::Left => {
                activate(&v_lead, &c_lead);
                activate(&v_top, &c_top);
                activate(&v_bot, &c_bot);
            }
            ViewportPlacement::Right => {
                activate(&v_trail, &c_trail);
                activate(&v_top, &c_top);
                activate(&v_bot, &c_bot);
            }
            ViewportPlacement::FullScreen => {
                activate(&v_top, &c_top);
                activate(&v_bot, &c_bot);
                activate(&v_lead, &c_lead);
                activate(&v_trail, &c_trail);
            }
        },
        OverlayAnchor::Element(elem) => {
            apply_element_anchor(container, child, elem, &v_top, &v_lead, &v_trail, &v_bot,
                                  &c_top, &c_lead, &v_cx, &v_cy, &c_width, &c_height);
            // Also cap to container width/height so an overflowing
            // popover doesn't escape the viewport.
            cap(&v_width, &c_width, 8.0);
            cap(&v_height, &c_height, 8.0);
        }
    }
}

/// Position a popover-style overlay relative to its anchor element's
/// viewport rect. Uses constraint constants computed from the rect.
///
/// Note: this is a one-shot positioning at mount. If the anchor moves
/// (scroll, layout reflow), the popover doesn't track — a future
/// version would observe the anchor's bounds and update constants on
/// change.
#[allow(clippy::too_many_arguments)]
fn apply_element_anchor(
    _container: &UIView,
    _child: &UIView,
    elem: &ElementAnchor,
    v_top: &Retained<NSObject>,
    v_lead: &Retained<NSObject>,
    v_trail: &Retained<NSObject>,
    v_bot: &Retained<NSObject>,
    c_top: &Retained<NSObject>,
    c_lead: &Retained<NSObject>,
    _v_cx: &Retained<NSObject>,
    _v_cy: &Retained<NSObject>,
    _c_width: &Retained<NSObject>,
    _c_height: &Retained<NSObject>,
) {
    // Resolve the anchor's viewport rect. If the target isn't mounted
    // yet (rect returns None or zero), fall back to centered.
    let rect = match elem.target.rect() {
        Some(r) if r.width > 0.0 || r.height > 0.0 => r,
        _ => {
            // Best-effort placement until the trigger measures.
            // Future: subscribe to mount + re-apply.
            let const_eq = |a: &Retained<NSObject>, b: &Retained<NSObject>, k: f64| {
                let c: Retained<NSObject> = unsafe {
                    msg_send_id![a, constraintEqualToAnchor: &**b, constant: k]
                };
                let _: () = unsafe { msg_send![&c, setActive: true] };
            };
            const_eq(v_top, c_top, 100.0);
            const_eq(v_lead, c_lead, 16.0);
            return;
        }
    };

    let target_x = rect.x as f64;
    let target_y = rect.y as f64;
    let target_w = rect.width as f64;
    let target_h = rect.height as f64;
    let offset = elem.offset as f64;

    // Compute the desired (top, left) of the popover relative to the
    // container's origin (which is window origin since the container
    // is pinned to the window).
    let (popover_top, popover_left) = match elem.side {
        ElementSide::Below => {
            let top = target_y + target_h + offset;
            let left = align_x(target_x, target_w, elem.align);
            (top, left)
        }
        ElementSide::Above => {
            // For Above, we set bottom relative to target instead —
            // since we set top here, use top = target_y - popover_height,
            // but we don't know popover height yet. Approximate by
            // using a bottom-anchored constraint instead.
            // Simplification: set top to target_y - estimate; the
            // child's own height will push from below. This isn't
            // perfect but works when popover height is known via
            // intrinsic content.
            //
            // Better approach: bind popover.bottom = target.top - offset
            // using a constraint, which we'll do below as a special
            // case. Return sentinel to skip the top-anchor branch.
            (f64::NAN, align_x(target_x, target_w, elem.align))
        }
        ElementSide::End => {
            let top = align_y(target_y, target_h, elem.align);
            let left = target_x + target_w + offset;
            (top, left)
        }
        ElementSide::Start => {
            (align_y(target_y, target_h, elem.align), f64::NAN)
        }
    };

    let constraint_eq = |a: &Retained<NSObject>, b: &Retained<NSObject>, k: f64| {
        let c: Retained<NSObject> = unsafe {
            msg_send_id![a, constraintEqualToAnchor: &**b, constant: k]
        };
        let _: () = unsafe { msg_send![&c, setActive: true] };
    };

    // Handle horizontal positioning.
    if popover_left.is_finite() {
        constraint_eq(v_lead, c_lead, popover_left);
    } else if matches!(elem.side, ElementSide::Start) {
        // Start side: pin trailing = target.x - offset, using
        // constraint relative to container.leadingAnchor with constant.
        let trailing_pos = target_x - offset;
        constraint_eq(v_trail, c_lead, trailing_pos);
    }

    // Handle vertical positioning.
    if popover_top.is_finite() {
        constraint_eq(v_top, c_top, popover_top);
    } else if matches!(elem.side, ElementSide::Above) {
        let bottom_pos = target_y - offset;
        constraint_eq(v_bot, c_top, bottom_pos);
    }
}

fn align_x(target_x: f64, target_w: f64, align: ElementAlign) -> f64 {
    match align {
        ElementAlign::Start => target_x,
        ElementAlign::Center => target_x + target_w / 2.0,
        ElementAlign::End => target_x + target_w,
    }
}

fn align_y(target_y: f64, target_h: f64, align: ElementAlign) -> f64 {
    match align {
        ElementAlign::Start => target_y,
        ElementAlign::Center => target_y + target_h / 2.0,
        ElementAlign::End => target_y + target_h,
    }
}

fn pin_to_edges(parent: &UIView, child: &UIView) {
    let p_top: Retained<NSObject> = unsafe { msg_send_id![parent, topAnchor] };
    let p_bot: Retained<NSObject> = unsafe { msg_send_id![parent, bottomAnchor] };
    let p_lead: Retained<NSObject> = unsafe { msg_send_id![parent, leadingAnchor] };
    let p_trail: Retained<NSObject> = unsafe { msg_send_id![parent, trailingAnchor] };
    let c_top: Retained<NSObject> = unsafe { msg_send_id![child, topAnchor] };
    let c_bot: Retained<NSObject> = unsafe { msg_send_id![child, bottomAnchor] };
    let c_lead: Retained<NSObject> = unsafe { msg_send_id![child, leadingAnchor] };
    let c_trail: Retained<NSObject> = unsafe { msg_send_id![child, trailingAnchor] };

    for (a, b) in [(&c_top, &p_top), (&c_bot, &p_bot), (&c_lead, &p_lead), (&c_trail, &p_trail)] {
        let c: Retained<NSObject> = unsafe { msg_send_id![a, constraintEqualToAnchor: &**b] };
        let _: () = unsafe { msg_send![&c, setActive: true] };
    }
}

fn mount_in_window(host_view: &UIView, container: &UIView) {
    let window: Option<Retained<UIView>> = unsafe { msg_send_id![host_view, window] };
    let Some(window) = window else {
        eprintln!("[ios-overlay] host view has no window — cannot mount");
        return;
    };
    unsafe { window.addSubview(container) };
    pin_to_edges(&window, container);
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
