//! Real AppKit input for the robot (`IntrospectionOps::hit_contains` /
//! `native_pointer`).
//!
//! Points arrive in `absolute_frame`'s space: the window's flipped content
//! view, top-left origin. `convertPoint:toView:nil` takes them to window
//! base coordinates (bottom-left), which is both an `NSEvent`'s
//! `locationInWindow` and what `-[contentView hitTest:]` expects (the
//! point in the content view's superview, the frame view, whose space is
//! the window's).
//!
//! Events are POSTED (`-[NSApplication postEvent:atStart:NO]`), not sent:
//! a view's `mouseDown:` may run a tracking loop that pulls the matching
//! `mouseUp:` off the queue (NSButton, NSSlider do), so `sendEvent:` of the
//! down would block until an up arrives. Posting both lets the run loop
//! deliver them in order through `-[NSApplication sendEvent:]` — local
//! monitors (the app pointer sink), the window, hit testing, the view —
//! exactly as a user's clicks.
//!
//! Hover is the exception. AppKit raises `mouseEntered:` / `mouseExited:`
//! from its own tracking-area bookkeeping, driven by the window server's
//! cursor — a posted mouse-moved event does not move it, so posting alone
//! never enters a view. A `Hover` step therefore does the bookkeeping:
//! it hit-tests the point in the live window, collects every view on that
//! chain whose enter/exit tracking area contains the point, and sends
//! `mouseExited:` to the owners it left and `mouseEntered:` to the ones it
//! entered — the events the tracking manager would send — then posts the
//! mouse-moved event for `mouseMoved:` handlers and the app pointer sink.
//! The hovered set is remembered between steps, as the cursor is.

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{class, msg_send, msg_send_id};
use objc2_app_kit::NSView;
use objc2_foundation::CGPoint;
use runtime_vocabulary::caps::{NativeInjection, PointerStep, PointerStepPhase};

use crate::imp::MacosNode;

// NSEventType values.
const LEFT_MOUSE_DOWN: usize = 1;
const LEFT_MOUSE_UP: usize = 2;
const LEFT_MOUSE_DRAGGED: usize = 6;
const MOUSE_MOVED: usize = 5;
const MOUSE_ENTERED: usize = 8;
const MOUSE_EXITED: usize = 9;

// NSTrackingAreaOptions bits.
const TRACKING_ENTERED_AND_EXITED: usize = 0x01;
const TRACKING_IN_VISIBLE_RECT: usize = 0x200;

thread_local! {
    /// Tracking areas the robot's pointer is inside, with their owners: what
    /// the next `Hover` step exits when it moves elsewhere.
    static HOVERED: RefCell<Vec<(Retained<AnyObject>, Retained<AnyObject>)>> = const { RefCell::new(Vec::new()) };
}

/// `(owner, area)` for every enter/exit tracking area containing the
/// window-base `point`, along the hit view's superview chain.
fn tracking_areas_at(content: *mut NSView, point: CGPoint) -> Vec<(Retained<AnyObject>, Retained<AnyObject>)> {
    let mut out = Vec::new();
    let mut view: *mut NSView = unsafe { msg_send![content, hitTest: point] };
    let nil: *mut NSView = std::ptr::null_mut();
    while !view.is_null() {
        let areas: *mut AnyObject = unsafe { msg_send![view, trackingAreas] };
        let count: usize = if areas.is_null() { 0 } else { unsafe { msg_send![areas, count] } };
        for i in 0..count {
            let area: *mut AnyObject = unsafe { msg_send![areas, objectAtIndex: i] };
            let opts: usize = unsafe { msg_send![area, options] };
            if opts & TRACKING_ENTERED_AND_EXITED == 0 {
                continue;
            }
            let local: CGPoint = unsafe { msg_send![view, convertPoint: point, fromView: nil] };
            let r: objc2_foundation::CGRect = if opts & TRACKING_IN_VISIBLE_RECT != 0 {
                unsafe { msg_send![view, visibleRect] }
            } else {
                unsafe { msg_send![area, rect] }
            };
            let inside = local.x >= r.origin.x
                && local.y >= r.origin.y
                && local.x < r.origin.x + r.size.width
                && local.y < r.origin.y + r.size.height;
            let owner: *mut AnyObject = unsafe { msg_send![area, owner] };
            if inside && !owner.is_null() {
                if let (Some(o), Some(a)) = unsafe { (Retained::retain(owner), Retained::retain(area)) } {
                    out.push((o, a));
                }
            }
        }
        view = unsafe { msg_send![view, superview] };
    }
    out
}

/// Exit what the pointer left and enter what it reached at `point`.
fn deliver_hover(window: &AnyObject, content: *mut NSView, point: CGPoint) -> Result<(), String> {
    let now = tracking_areas_at(content, point);
    let before = HOVERED.with(|h| std::mem::take(&mut *h.borrow_mut()));
    let same = |a: &AnyObject, b: &AnyObject| std::ptr::eq(a, b);
    let number: isize = unsafe { msg_send![window, windowNumber] };
    let send = |kind: usize, owner: &AnyObject, area: &AnyObject| -> Result<(), String> {
        let ctx: *mut AnyObject = std::ptr::null_mut();
        let event: Option<Retained<AnyObject>> = unsafe {
            msg_send_id![
                class!(NSEvent),
                enterExitEventWithType: kind,
                location: point,
                modifierFlags: 0usize,
                timestamp: 0.0f64,
                windowNumber: number,
                context: ctx,
                eventNumber: 0isize,
                trackingNumber: area as *const AnyObject as isize,
                userData: std::ptr::null_mut::<std::ffi::c_void>(),
            ]
        };
        let event = event.ok_or("NSEvent refused the enter/exit event")?;
        if kind == MOUSE_ENTERED {
            let _: () = unsafe { msg_send![owner, mouseEntered: &*event] };
        } else {
            let _: () = unsafe { msg_send![owner, mouseExited: &*event] };
        }
        Ok(())
    };
    for (owner, area) in &before {
        if !now.iter().any(|(_, a)| same(a, area)) {
            send(MOUSE_EXITED, owner, area)?;
        }
    }
    for (owner, area) in &now {
        if !before.iter().any(|(_, a)| same(a, area)) {
            send(MOUSE_ENTERED, owner, area)?;
        }
    }
    HOVERED.with(|h| *h.borrow_mut() = now);
    Ok(())
}

/// The node's window and its content view, when it is in one.
fn window_of(view: &NSView) -> Option<(Retained<AnyObject>, *mut NSView)> {
    let window: Option<Retained<AnyObject>> = unsafe { msg_send_id![view, window] };
    let window = window?;
    let content: *mut NSView = unsafe { msg_send![&*window, contentView] };
    (!content.is_null()).then_some((window, content))
}

/// A content-view point (top-left) in window base coordinates.
fn to_window_base(content: *mut NSView, x: f32, y: f32) -> CGPoint {
    let nil: *mut NSView = std::ptr::null_mut();
    unsafe { msg_send![content, convertPoint: CGPoint { x: x as f64, y: y as f64 }, toView: nil] }
}

pub(crate) fn hit_contains(node: &MacosNode, x: f32, y: f32) -> Option<bool> {
    let view = node.as_view();
    let (_window, content) = window_of(view)?;
    let point = to_window_base(content, x, y);
    let hit: *mut NSView = unsafe { msg_send![content, hitTest: point] };
    if hit.is_null() {
        return Some(false);
    }
    Some(unsafe { msg_send![hit, isDescendantOf: view] })
}

pub(crate) fn native_pointer(node: &MacosNode, steps: &[PointerStep]) -> Option<NativeInjection> {
    let (window, content) = window_of(node.as_view())?;
    // Converted now, while the layout the robot measured is current.
    let events: Vec<(usize, CGPoint)> = steps
        .iter()
        .map(|s| {
            let kind = match s.phase {
                PointerStepPhase::Down => LEFT_MOUSE_DOWN,
                PointerStepPhase::Move => LEFT_MOUSE_DRAGGED,
                PointerStepPhase::Up => LEFT_MOUSE_UP,
                PointerStepPhase::Hover => MOUSE_MOVED,
            };
            (kind, to_window_base(content, s.x, s.y))
        })
        .collect();
    Some(Box::new(move || {
        let number: isize = unsafe { msg_send![&*window, windowNumber] };
        let info: *mut AnyObject = unsafe { msg_send![class!(NSProcessInfo), processInfo] };
        let app: *mut AnyObject = unsafe { msg_send![class!(NSApplication), sharedApplication] };
        let ctx: *mut AnyObject = std::ptr::null_mut();
        for (kind, location) in events {
            if kind == MOUSE_MOVED {
                deliver_hover(&window, content, location)?;
            }
            let now: f64 = unsafe { msg_send![info, systemUptime] };
            let event: Option<Retained<AnyObject>> = unsafe {
                msg_send_id![
                    class!(NSEvent),
                    mouseEventWithType: kind,
                    location: location,
                    modifierFlags: 0usize,
                    timestamp: now,
                    windowNumber: number,
                    context: ctx,
                    eventNumber: 0isize,
                    clickCount: 1isize,
                    pressure: if kind == LEFT_MOUSE_UP || kind == MOUSE_MOVED { 0.0f32 } else { 1.0f32 },
                ]
            };
            let event = event.ok_or("NSEvent refused the mouse event")?;
            let _: () = unsafe { msg_send![app, postEvent: &*event, atStart: false] };
        }
        Ok(())
    }))
}
