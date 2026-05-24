//! Portal primitive on iOS — render a subtree at a window-level
//! `UIView` that escapes the parent's layout, clipping, and stacking
//! context. Used by modals, popovers, dropdowns, tooltips, sheets,
//! and any author-built floating UX.
//!
//! Backdrops are no longer a backend concern — the framework's
//! composition layer renders a backdrop primitive as a child of the
//! portal. The container is always passthrough-hit-tested: taps
//! outside actual child views fall through to the page beneath.
//!
//! ## Layout model
//!
//! For [`PortalTarget::Viewport`]: the container is a window-rooted
//! `UIView` registered as a Taffy root (orphan — `insert` skips
//! parent-tree wiring for portals). The layout tree's viewport
//! auto-fill resizes it to the full viewport on every layout pass
//! (orientation flips, split-view resizes, etc.). The
//! placement-derived flex style places the portal's content child in
//! the requested viewport region.
//!
//! For [`PortalTarget::Anchor`]: same window-rooted container, but
//! the content child is positioned absolutely against the anchor's
//! viewport rect. A `CADisplayLink` re-pins the child's origin every
//! vsync, tracking the anchor through scrolls / animations /
//! orientation changes.
//!
//! ## Trap focus
//!
//! `trap_focus = true` sets `accessibilityViewIsModal = YES` on the
//! container so VoiceOver / Switch Control treat the portal's
//! contents as the only interactive subtree until the portal closes.

use objc2::msg_send;
use objc2::msg_send_id;
use objc2::rc::Retained;
use objc2_foundation::{MainThreadMarker, NSObject, NSString};
use objc2_ui_kit::UIView;
use std::rc::Rc;

use runtime_core::primitives::portal::{
    AnchorTarget, ElementAlign, ElementSide, ViewportPlacement, ViewportRect,
};
use runtime_core::{
    AlignItems, FlexDirection, JustifyContent, Length, Position, StyleRules, Tokenized,
};

use crate::imp::callbacks::{DisplayLinkTarget, OverlayPassthroughView};

/// Per-portal state stored in the backend, keyed by the container
/// view's pointer.
pub(crate) struct PortalEntry {
    /// The top-level container view (a passthrough `UIView` added to
    /// the host's `UIWindow`). Dropping/removing it tears down the
    /// entire portal subtree.
    pub(crate) container: Retained<UIView>,
    /// `Some(...)` for anchored portals — the descriptor used to
    /// compute the child's initial Taffy style on `insert` and to
    /// re-pin the child each vsync. `None` for viewport portals.
    pub(crate) anchor: Option<AnchorSpec>,
    /// Live `CADisplayLink` for anchored portals. `None` for viewport
    /// portals or until the first `insert` populates the content
    /// view. Invalidated in `release_portal`.
    pub(crate) anchor_link: Option<Retained<NSObject>>,
}

pub(crate) struct AnchorSpec {
    pub(crate) target: AnchorTarget,
    pub(crate) side: ElementSide,
    pub(crate) align: ElementAlign,
    pub(crate) offset: f32,
}

pub(crate) type PortalInstances = std::collections::HashMap<usize, PortalEntry>;

/// Create a portal container in the host's window. The framework's
/// `insert` path will mount the portal's children (backdrop +
/// content) into the container; for anchored portals the first
/// non-backdrop child gets the absolute-position style + tracker.
pub(crate) fn create_portal(
    mtm: MainThreadMarker,
    host_root: Option<&Retained<UIView>>,
    anchor: Option<AnchorSpec>,
    trap_focus: bool,
) -> (Retained<UIView>, PortalEntry) {
    // Always passthrough — the composition layer in runtime-core
    // renders a backdrop as a child primitive when needed; that
    // backdrop sizes itself to fill the viewport and consumes taps
    // through its own gesture recognizer. Outside the backdrop /
    // content, taps should fall through to the page beneath.
    let container: Retained<UIView> = {
        let v = OverlayPassthroughView::new(mtm);
        unsafe { Retained::cast::<UIView>(v) }
    };

    if trap_focus {
        // Accessibility-focus trap: VoiceOver / Switch Control treat
        // the portal as the only accessible subtree while it's
        // mounted. Visual focus (keyboard) on iOS is best-effort —
        // there's no public API equivalent for confining tab order.
        let _: () = unsafe { msg_send![&container, setAccessibilityViewIsModal: true] };
    }

    // Mount into the window on the next runloop turn — the framework
    // is mid-build and we want the children inserted first.
    if let Some(root) = host_root {
        let root_clone = root.clone();
        let container_clone = container.clone();
        schedule_main(move || mount_in_window(&root_clone, &container_clone));
    }

    let entry = PortalEntry {
        container: container.clone(),
        anchor,
        anchor_link: None,
    };
    (container, entry)
}

/// `StyleRules` for a viewport-placement portal container. Places the
/// (single) author-supplied content child in the requested viewport
/// region via flex `justify_content` / `align_items`.
pub(crate) fn container_style_for_placement(placement: ViewportPlacement) -> StyleRules {
    let mut rules = StyleRules::default();
    rules.flex_direction = Some(FlexDirection::Column);
    match placement {
        ViewportPlacement::Center => {
            rules.justify_content = Some(JustifyContent::Center);
            rules.align_items = Some(AlignItems::Center);
            // Safety inset so an oversized child can't touch the
            // viewport edges. Author `max_width` is stricter and wins.
            let inset = Tokenized::Literal(Length::Px(VIEWPORT_CENTER_INSET));
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

/// `StyleRules` for an anchored-portal container. Neutral flex layout
/// — the content child positions itself via `position: absolute` +
/// insets so the container just needs to span the viewport (where
/// the absolute coordinates live).
pub(crate) fn container_style_for_anchor() -> StyleRules {
    let mut rules = StyleRules::default();
    rules.flex_direction = Some(FlexDirection::Column);
    rules.justify_content = Some(JustifyContent::FlexStart);
    rules.align_items = Some(AlignItems::FlexStart);
    rules
}

/// Safety padding around `Center`-placed content so an oversized
/// child can't touch the viewport edges. Authors who want a tighter
/// or looser margin set their own `max_width`/`margin`; those win
/// because they're stricter or stacked.
const VIEWPORT_CENTER_INSET: f32 = 16.0;

/// Fallback "popover height" used in the static-pre-measurement
/// path. We don't know the popover's measured size before Taffy
/// lays it out; this is the initial guess for `Above`, corrected on
/// the tracker's first vsync.
const ANCHOR_POPOVER_HEIGHT_FALLBACK: f32 = 100.0;

/// Same idea for the `Start` (right-to-left) case.
const ANCHOR_POPOVER_WIDTH_FALLBACK: f32 = 200.0;

/// Minimum distance from the viewport top/left that the unmeasured
/// `Above`/`Start` fallbacks will clamp to — keeps the popover from
/// rendering off-screen before the tracker corrects it.
const ANCHOR_FALLBACK_MIN_INSET: f32 = 8.0;

/// Build the absolutely-positioned style for an anchored portal's
/// content child based on the anchor's current viewport rect. Used
/// at mount time for an initial frame before the display-link
/// tracker takes over.
pub(crate) fn child_style_for_anchor(anchor: &AnchorSpec) -> StyleRules {
    // If the trigger hasn't measured yet (no window, not mounted),
    // fall back to a safe top-left default. The composition typically
    // only mounts after the trigger is on screen, so this is mostly
    // defensive.
    let rect = match anchor.target.rect() {
        Some(r) if r.width > 0.0 || r.height > 0.0 => r,
        _ => {
            let mut rules = StyleRules::default();
            rules.position = Some(Position::Absolute);
            rules.top = Some(Tokenized::Literal(Length::Px(ANCHOR_POPOVER_HEIGHT_FALLBACK)));
            rules.left = Some(Tokenized::Literal(Length::Px(VIEWPORT_CENTER_INSET)));
            return rules;
        }
    };

    let mut rules = StyleRules::default();
    rules.position = Some(Position::Absolute);
    // Initial placement uses the static (unmeasured-popover-size)
    // align — Center/End will land slightly off until the tracker's
    // first vsync. That's fine because the tracker runs the same
    // frame and corrects it.
    match anchor.side {
        ElementSide::Below => {
            rules.top = Some(Tokenized::Literal(Length::Px(rect.y + rect.height + anchor.offset)));
            rules.left = Some(Tokenized::Literal(Length::Px(align_x_unmeasured(&rect, anchor.align))));
        }
        ElementSide::Above => {
            // Without the popover's measured height we can't subtract
            // it from `rect.y` — approximate with the fallback. The
            // tracker fixes this on first vsync once Taffy has sized
            // the popover.
            let top = (rect.y - anchor.offset - ANCHOR_POPOVER_HEIGHT_FALLBACK)
                .max(ANCHOR_FALLBACK_MIN_INSET);
            rules.top = Some(Tokenized::Literal(Length::Px(top)));
            rules.left = Some(Tokenized::Literal(Length::Px(align_x_unmeasured(&rect, anchor.align))));
        }
        ElementSide::End => {
            rules.top = Some(Tokenized::Literal(Length::Px(align_y_unmeasured(&rect, anchor.align))));
            rules.left = Some(Tokenized::Literal(Length::Px(rect.x + rect.width + anchor.offset)));
        }
        ElementSide::Start => {
            rules.top = Some(Tokenized::Literal(Length::Px(align_y_unmeasured(&rect, anchor.align))));
            let left = (rect.x - anchor.offset - ANCHOR_POPOVER_WIDTH_FALLBACK)
                .max(ANCHOR_FALLBACK_MIN_INSET);
            rules.left = Some(Tokenized::Literal(Length::Px(left)));
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

/// Start a `CADisplayLink` that re-pins `popover` to the anchor
/// every vsync. Returns the retained link; the caller stores it on
/// the entry so `release_portal` can `invalidate` it.
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
        if (cur_top - top).abs() < ANCHOR_TRACKER_EPSILON
            && (cur_left - left).abs() < ANCHOR_TRACKER_EPSILON
        {
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
    let run_loop: Retained<NSObject> =
        unsafe { msg_send_id![objc2::class!(NSRunLoop), mainRunLoop] };
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

/// Sub-pixel threshold below which the tracker treats the popover as
/// already in the right place. Avoids per-frame `setFrame:` churn
/// when the anchor isn't moving — both `cur_*` and the computed
/// origin can drift by tiny amounts due to UIKit's geometry rounding.
const ANCHOR_TRACKER_EPSILON: f32 = 0.5;

/// Compute the popover's top-left origin in viewport coordinates,
/// given the anchor's current rect and the popover's measured size.
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

/// Tear down a portal's UIKit state. Stops the anchor tracker (if
/// any) first so a final tick can't fire into a half-torn-down
/// hierarchy, then removes the container from its window.
pub(crate) fn release_portal(entry: PortalEntry) {
    if let Some(link) = entry.anchor_link {
        let _: () = unsafe { msg_send![&*link, invalidate] };
    }
    let container = entry.container;
    schedule_main(move || {
        unsafe { container.removeFromSuperview() };
    });
}

/// Add a backend-owned portal container view to the host's window.
/// Sized via autoresizing mask so its bounds track the window's
/// bounds — Taffy uses this as the viewport-root frame for the
/// portal's content tree.
pub(crate) fn mount_in_window(host_view: &UIView, container: &UIView) {
    let window: Option<Retained<UIView>> = unsafe { msg_send_id![host_view, window] };
    let Some(window) = window else {
        eprintln!("[ios-portal] host view has no window — cannot mount");
        return;
    };
    let window_bounds: objc2_foundation::CGRect = unsafe { msg_send![&window, bounds] };
    let _: () = unsafe { msg_send![container, setFrame: window_bounds] };
    // flexibleWidth (2) | flexibleHeight (16).
    let _: () = unsafe { msg_send![container, setAutoresizingMask: 0x12u64] };
    unsafe { window.addSubview(container) };
}

/// Dispatch a Rust closure on the main queue's next runloop turn.
/// Used to defer container mounting / unmounting outside of the
/// framework's current `backend.borrow_mut()` window.
pub(crate) fn schedule_main<F: FnOnce() + 'static>(f: F) {
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
        // SAFETY (libdispatch handoff): ctx came from the matching
        // `Box::into_raw` above; libdispatch promises to call this
        // exactly once. The box reclaim transfers ownership back to
        // Rust before the user closure runs.
        let boxed: Box<Box<dyn FnOnce()>> = unsafe { Box::from_raw(ctx as *mut _) };
        // Wrap the user closure in `catch_unwind` — libdispatch is C
        // code and a panic propagating into it is undefined behavior.
        // The closure is `FnOnce`; we take it out of the box explicitly
        // so the closure body can move captures freely.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            boxed();
        }));
        if let Err(payload) = result {
            let msg = if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else if let Some(s) = payload.downcast_ref::<&'static str>() {
                (*s).to_string()
            } else {
                "<non-string panic payload>".to_string()
            };
            eprintln!("[backend-ios::portal] schedule_main closure panicked: {msg}");
        }
    }

    unsafe {
        dispatch_async_f(
            &_dispatch_main_q as *const _ as *const std::ffi::c_void,
            ctx,
            trampoline,
        );
    }
}
