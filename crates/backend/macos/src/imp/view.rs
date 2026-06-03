//! [`FlippedView`] — `NSView` subclass that overrides `isFlipped`
//! to return `true`, giving us a top-left coordinate origin, AND
//! translates AppKit mouse events into the framework's `on_touch`
//! [`TouchHandler`] — the macOS analogue of iOS's `IdealystTouchView`.
//!
//! AppKit defaults to bottom-left origin (Y-up); Taffy emits frames
//! in top-left (Y-down). Resolving the mismatch at the view level
//! (one method override) instead of per-frame-write keeps the rest
//! of the backend identical to iOS / web / Android, all of which
//! already think in top-left.
//!
//! Every `create_view` / `create_pressable` / `create_link` returns
//! a `FlippedView`. AppKit-supplied leaves (NSTextField, NSButton,
//! NSSlider, NSSwitch, NSImageView) keep their default orientation
//! since their internal layout is opaque to us — the Taffy layout
//! pass sets their `frame` directly, computed in a flipped parent's
//! coordinate space.
//!
//! ## Mouse → `on_touch`
//!
//! `mouseDown:`/`mouseDragged:`/`mouseUp:` map to `TouchPhase::Began`/
//! `Moved`/`Ended` for the single mouse pointer. A handler is installed by
//! `Backend::install_touch_handler`; views without one let the event bubble up
//! the responder chain (so a click on a child without a handler still reaches a
//! parent that has one — exactly like iOS's `touchesBegan` super-call). macOS
//! points equal dp (no density scaling, unlike Android), and `isFlipped`
//! already makes `convertPoint:fromView:nil` return top-left coords matching
//! Taffy frames / the canvas Scene.

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::{declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_app_kit::{NSEvent, NSView};
use objc2_foundation::{CGPoint, MainThreadMarker};

use runtime_core::{TouchEvent, TouchHandler, TouchId, TouchPhase, TouchPoint};

/// Stable id for the single mouse pointer (macOS has no multitouch here).
const MOUSE_TOUCH_ID: u64 = 1;

pub struct FlippedViewIvars {
    /// Installed by `Backend::install_touch_handler`; `None` for the many
    /// views (containers, layout wrappers, native-control hosts) that carry
    /// no `on_touch`. `RefCell<Option<_>>` so it can be set after creation.
    handler: RefCell<Option<TouchHandler>>,
    /// True between a `mouseDown` we accepted (handler consumed/claimed) and
    /// the matching `mouseUp`, so drag/up events only dispatch for a gesture we
    /// actually started — mirrors iOS's `active_touches` gate.
    active: Cell<bool>,
}

declare_class!(
    pub struct FlippedView;

    unsafe impl ClassType for FlippedView {
        type Super = NSView;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystFlippedView";
    }

    impl DeclaredClass for FlippedView {
        type Ivars = FlippedViewIvars;
    }

    unsafe impl FlippedView {
        #[method(isFlipped)]
        fn is_flipped(&self) -> bool {
            true
        }

        // Register the FIRST click even when the window isn't key, so a tap on
        // the canvas/toolbar acts immediately (matches mobile tap behavior).
        #[method(acceptsFirstMouse:)]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[method(mouseDown:)]
        fn mouse_down(&self, event: &NSEvent) {
            if !self.dispatch_mouse(event, TouchPhase::Began) {
                let _: () = unsafe { msg_send![super(self), mouseDown: event] };
            }
        }

        #[method(mouseDragged:)]
        fn mouse_dragged(&self, event: &NSEvent) {
            if !self.dispatch_mouse(event, TouchPhase::Moved) {
                let _: () = unsafe { msg_send![super(self), mouseDragged: event] };
            }
        }

        #[method(mouseUp:)]
        fn mouse_up(&self, event: &NSEvent) {
            if !self.dispatch_mouse(event, TouchPhase::Ended) {
                let _: () = unsafe { msg_send![super(self), mouseUp: event] };
            }
        }
    }
);

impl FlippedView {
    pub(crate) fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(FlippedViewIvars {
            handler: RefCell::new(None),
            active: Cell::new(false),
        });
        unsafe { msg_send_id![super(this), init] }
    }

    /// Install (or replace) the `on_touch` handler. Called by
    /// `Backend::install_touch_handler`.
    pub(crate) fn set_handler(&self, handler: TouchHandler) {
        *self.ivars().handler.borrow_mut() = Some(handler);
    }

    /// `true` if an `on_touch` handler has been installed — i.e. this view is
    /// an interactive control. The private-layer passthrough hit-test uses
    /// this to decide whether a click should be CAPTURED (a real control) or
    /// fall through to the app window beneath (a bare layout container). The
    /// macOS analogue of iOS's `IdealystTouchView::has_handler`.
    pub(crate) fn has_handler(&self) -> bool {
        self.ivars().handler.borrow().is_some()
    }

    /// Translate one AppKit mouse event into a `TouchEvent` and dispatch it to
    /// the installed handler. Returns `true` when the event was handled (so the
    /// caller does NOT bubble it to `super`); `false` when there's no handler
    /// or it's a drag/up we didn't start, letting the responder chain carry it
    /// to an ancestor that does have a handler.
    fn dispatch_mouse(&self, event: &NSEvent, phase: TouchPhase) -> bool {
        // Snapshot the handler so we don't hold the ivar borrow across the
        // closure (it may re-enter the backend / signals).
        let handler = match self.ivars().handler.borrow().as_ref() {
            Some(h) => h.clone(),
            None => return false,
        };
        // Drag/up only matter if we accepted the down.
        if matches!(phase, TouchPhase::Moved | TouchPhase::Ended) && !self.ivars().active.get() {
            return false;
        }

        // Window coords (AppKit bottom-left) → this view's local coords. Since
        // `FlippedView` is `isFlipped`, the result is top-left (dp), matching
        // Taffy frames and the canvas Scene — no density scaling on macOS.
        let win: CGPoint = unsafe { msg_send![event, locationInWindow] };
        let local: CGPoint =
            unsafe { msg_send![self, convertPoint: win, fromView: std::ptr::null::<NSView>()] };
        let ts: f64 = unsafe { msg_send![event, timestamp] };

        let ev = TouchEvent {
            id: TouchId(MOUSE_TOUCH_ID),
            phase,
            position: TouchPoint::new(local.x as f32, local.y as f32),
            // The on_touch surfaces that need window coords (the drawing
            // canvas) fill the window, so window-relative == local here.
            window_position: TouchPoint::new(local.x as f32, local.y as f32),
            timestamp_ns: (ts * 1_000_000_000.0) as u64,
            force: None,
        };
        let response = (handler)(&ev);

        match phase {
            TouchPhase::Began => {
                if response.consumed || response.claim {
                    self.ivars().active.set(true);
                }
            }
            TouchPhase::Ended | TouchPhase::Cancelled => self.ivars().active.set(false),
            _ => {}
        }

        // Handled (don't bubble) if the handler consumed, or we're mid-gesture
        // it already accepted. An explicit IGNORED on `Began` returns false so
        // the event still bubbles to a parent handler.
        response.consumed || self.ivars().active.get()
    }
}
