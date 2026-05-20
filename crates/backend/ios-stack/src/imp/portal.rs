//! Portal primitive on the iOS stack backend — render a subtree at a
//! window-level `UIView` that escapes the parent's layout and stacking
//! context. Used by modals, popovers, dropdowns, tooltips, sheets,
//! and any author-built floating UX.
//!
//! Backdrops are no longer a backend concern — the framework's
//! composition layer renders a backdrop primitive as a child of the
//! portal. The portal container just sees a portal with children.
//!
//! ## Layout model: absolute positioning via Auto Layout
//!
//! The portal container is a plain `UIView` pinned to the host's
//! `UIWindow` via Auto Layout (this backend doesn't use Taffy). Author
//! content is positioned via constraints derived from the
//! [`PortalTarget`]:
//!
//! - `Viewport(Center)`     — content centered in the viewport
//! - `Viewport(Top|Bottom|Left|Right)` — pinned to a viewport edge
//! - `Viewport(FullScreen)` — fills the viewport
//! - `Anchor { ... }`       — anchored to an element's rect (one-shot
//!   at mount; this backend doesn't run a live anchor tracker — if
//!   the trigger moves, the portal stays where it was placed)
//! - `Named(_)`             — reserved (falls back to FullScreen)
//!
//! Absolute positioning avoids `UIStackView`'s `UISV-canvas-connection`
//! constraints, which fight with portal positioning.

use objc2::rc::Retained;
use objc2::{msg_send, msg_send_id};
use objc2_foundation::{MainThreadMarker, NSObject};
use objc2_ui_kit::UIView;

use framework_core::primitives::portal::{
    AnchorTarget, ElementAlign, ElementSide, PortalTarget, ViewportPlacement,
};

/// Per-portal state stored in the backend, keyed by the container
/// view's pointer.
pub(crate) struct PortalEntry {
    /// The top-level container view (a plain UIView added to the
    /// host's UIWindow). Dropping/removing it tears down the entire
    /// portal subtree.
    pub(crate) container: Retained<UIView>,
    /// The portal target — used by `IosBackend::insert` to position
    /// children when the framework inserts them.
    pub(crate) target: PortalTarget,
}

/// Create a portal container in the host's window. Returns the
/// container view; the framework's `insert` path will position author
/// content via constraints inside it based on the stored target.
pub(crate) fn create_portal(
    mtm: MainThreadMarker,
    host_root: Option<&Retained<UIView>>,
    target: PortalTarget,
    trap_focus: bool,
) -> (Retained<UIView>, PortalEntry) {
    // Container: a plain full-window UIView. Author content is
    // positioned via Auto Layout constraints as subviews.
    let container = unsafe { UIView::new(mtm) };
    let _: () = unsafe {
        msg_send![&container, setTranslatesAutoresizingMaskIntoConstraints: false]
    };

    if trap_focus {
        // Accessibility focus trap — VoiceOver / Switch Control treat
        // the portal as the only accessible subtree while it's
        // mounted.
        let _: () = unsafe { msg_send![&container, setAccessibilityViewIsModal: true] };
    }

    // Mount the container into the host's window above all other
    // content. Defer to next main-queue turn so the framework's
    // insert has time to populate children first.
    if let Some(root) = host_root {
        let root_clone = root.clone();
        let container_clone = container.clone();
        schedule_main(move || mount_in_window(&root_clone, &container_clone));
    }

    let entry = PortalEntry {
        container: container.clone(),
        target,
    };
    (container, entry)
}

/// Position a child view inside a portal container based on the
/// portal's target. Called by `IosBackend::insert` when the parent
/// is a portal container.
pub(crate) fn apply_target_to_child(
    container: &UIView,
    child: &UIView,
    target: &PortalTarget,
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
    // can't grow beyond the screen. Author stylesheets can still
    // set smaller widths (e.g. `max_width: 520`) — those are
    // stricter and win automatically.
    let cap = |child_anchor: &Retained<NSObject>,
               container_anchor: &Retained<NSObject>,
               inset: f64| {
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

    match target {
        PortalTarget::Viewport(placement) => match placement {
            ViewportPlacement::Center => {
                activate(&v_cx, &c_cx);
                activate(&v_cy, &c_cy);
                // Safety caps: child width/height ≤ container - VIEWPORT_CENTER_INSET*2
                // padding. Author stylesheet `max_width` is stricter
                // and overrides automatically.
                cap(&v_width, &c_width, VIEWPORT_CENTER_INSET);
                cap(&v_height, &c_height, VIEWPORT_CENTER_INSET);
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
        PortalTarget::Anchor { target, side, align, offset } => {
            apply_element_anchor(
                child, target, *side, *align, *offset,
                &v_top, &v_lead, &v_trail, &v_bot, &c_top, &c_lead,
            );
            // Also cap to container width/height so an overflowing
            // popover doesn't escape the viewport.
            cap(&v_width, &c_width, ANCHOR_VIEWPORT_INSET);
            cap(&v_height, &c_height, ANCHOR_VIEWPORT_INSET);
        }
        PortalTarget::Named(_) => {
            // No registry yet — same fallback as `create_portal`:
            // fill the viewport so the child is visible.
            activate(&v_top, &c_top);
            activate(&v_bot, &c_bot);
            activate(&v_lead, &c_lead);
            activate(&v_trail, &c_trail);
        }
    }
}

/// Safety padding (in pt) around `Center`-placed content so an
/// oversized child can't touch the viewport edges. Authors who want
/// a tighter or looser margin set their own `max_width`/`margin`;
/// those win because they're stricter or stacked.
const VIEWPORT_CENTER_INSET: f64 = 16.0;

/// Per-side viewport inset (in pt) applied to anchored popovers as
/// a cap on width/height so a too-large popover can't escape the
/// visible area.
const ANCHOR_VIEWPORT_INSET: f64 = 8.0;

/// Static fallback position (in pt) used when an anchor element
/// hasn't measured yet (e.g. the portal opens before its trigger is
/// in the window hierarchy). Picks a top-leftish spot rather than
/// stacking everything at (0, 0).
const ANCHOR_FALLBACK_TOP: f64 = 100.0;
const ANCHOR_FALLBACK_LEAD: f64 = 16.0;

/// Position an anchored portal's content relative to its anchor's
/// viewport rect. Uses constraint constants computed from the rect.
///
/// One-shot at mount: if the anchor moves (scroll, layout reflow),
/// the portal doesn't track. The mobile backend (Taffy-based) runs
/// a CADisplayLink tracker; this stack-based backend doesn't.
#[allow(clippy::too_many_arguments)]
fn apply_element_anchor(
    _child: &UIView,
    target: &AnchorTarget,
    side: ElementSide,
    align: ElementAlign,
    offset: f32,
    v_top: &Retained<NSObject>,
    v_lead: &Retained<NSObject>,
    v_trail: &Retained<NSObject>,
    v_bot: &Retained<NSObject>,
    c_top: &Retained<NSObject>,
    c_lead: &Retained<NSObject>,
) {
    // Resolve the anchor's viewport rect. If the target isn't
    // mounted yet (rect returns None or zero), fall back to a safe
    // top-leftish placement.
    let rect = match target.rect() {
        Some(r) if r.width > 0.0 || r.height > 0.0 => r,
        _ => {
            let const_eq = |a: &Retained<NSObject>, b: &Retained<NSObject>, k: f64| {
                let c: Retained<NSObject> = unsafe {
                    msg_send_id![a, constraintEqualToAnchor: &**b, constant: k]
                };
                let _: () = unsafe { msg_send![&c, setActive: true] };
            };
            const_eq(v_top, c_top, ANCHOR_FALLBACK_TOP);
            const_eq(v_lead, c_lead, ANCHOR_FALLBACK_LEAD);
            return;
        }
    };

    let target_x = rect.x as f64;
    let target_y = rect.y as f64;
    let target_w = rect.width as f64;
    let target_h = rect.height as f64;
    let offset = offset as f64;

    // Compute the desired (top, left) of the popover relative to
    // the container's origin (which is window origin since the
    // container is pinned to the window). `NaN` is used as a sentinel
    // for "set bottom/trailing instead of top/leading" — needed for
    // `Above` and `Start` where we anchor against the popover's
    // far edge.
    let (popover_top, popover_left) = match side {
        ElementSide::Below => {
            let top = target_y + target_h + offset;
            let left = align_x(target_x, target_w, align);
            (top, left)
        }
        ElementSide::Above => {
            // For Above, anchor popover.bottom = target.top - offset
            // (signaled by NaN top).
            (f64::NAN, align_x(target_x, target_w, align))
        }
        ElementSide::End => {
            let top = align_y(target_y, target_h, align);
            let left = target_x + target_w + offset;
            (top, left)
        }
        ElementSide::Start => {
            // For Start, anchor popover.trailing = target.x - offset
            // (signaled by NaN left).
            (align_y(target_y, target_h, align), f64::NAN)
        }
    };

    let constraint_eq = |a: &Retained<NSObject>, b: &Retained<NSObject>, k: f64| {
        let c: Retained<NSObject> = unsafe {
            msg_send_id![a, constraintEqualToAnchor: &**b, constant: k]
        };
        let _: () = unsafe { msg_send![&c, setActive: true] };
    };

    // Horizontal positioning.
    if popover_left.is_finite() {
        constraint_eq(v_lead, c_lead, popover_left);
    } else if matches!(side, ElementSide::Start) {
        // Pin trailing = target.x - offset relative to
        // container.leadingAnchor with constant.
        let trailing_pos = target_x - offset;
        constraint_eq(v_trail, c_lead, trailing_pos);
    }

    // Vertical positioning.
    if popover_top.is_finite() {
        constraint_eq(v_top, c_top, popover_top);
    } else if matches!(side, ElementSide::Above) {
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

    for (a, b) in [
        (&c_top, &p_top),
        (&c_bot, &p_bot),
        (&c_lead, &p_lead),
        (&c_trail, &p_trail),
    ] {
        let c: Retained<NSObject> = unsafe { msg_send_id![a, constraintEqualToAnchor: &**b] };
        let _: () = unsafe { msg_send![&c, setActive: true] };
    }
}

fn mount_in_window(host_view: &UIView, container: &UIView) {
    let window: Option<Retained<UIView>> = unsafe { msg_send_id![host_view, window] };
    let Some(window) = window else {
        eprintln!("[ios-portal] host view has no window — cannot mount");
        return;
    };
    unsafe { window.addSubview(container) };
    pin_to_edges(&window, container);
}

pub(crate) fn release_portal(entry: PortalEntry) {
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

