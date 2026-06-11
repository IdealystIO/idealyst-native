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
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::{declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_app_kit::{
    NSCursor, NSEvent, NSTextField, NSTextFieldCell, NSTrackingArea, NSTrackingAreaOptions, NSView,
};
use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker, NSString};
use std::cell::Cell as StdCell;

use runtime_core::{
    set_pointer_modifiers, PointerModifiers, StateBits, TouchEvent, TouchHandler, TouchId,
    TouchPhase, TouchPoint, WheelEvent, WheelHandler, WheelKind,
};

/// Stable id for the single mouse pointer (macOS has no multitouch here).
const MOUSE_TOUCH_ID: u64 = 1;

pub struct FlippedViewIvars {
    /// Installed by `Backend::install_touch_handler`; `None` for the many
    /// views (containers, layout wrappers, native-control hosts) that carry
    /// no `on_touch`. `RefCell<Option<_>>` so it can be set after creation.
    handler: RefCell<Option<TouchHandler>>,
    /// Installed by `Backend::install_wheel_handler`; `None` for views with no
    /// `on_wheel`. Drives the desktop zoom/scroll channel: `magnify:` →
    /// `WheelKind::Zoom`, `scrollWheel:` → `WheelKind::Scroll`.
    wheel_handler: RefCell<Option<WheelHandler>>,
    /// True between a `mouseDown` we accepted (handler consumed/claimed) and
    /// the matching `mouseUp`, so drag/up events only dispatch for a gesture we
    /// actually started — mirrors iOS's `active_touches` gate.
    active: Cell<bool>,
    /// The hover cursor for this view, mapped from `StyleRules::cursor`.
    /// `None` = no custom cursor (the OS default arrow). Installed as a
    /// cursor rect over the view's bounds in `resetCursorRects`; AppKit
    /// rebuilds those rects when we invalidate or the geometry changes.
    cursor: RefCell<Option<Retained<NSCursor>>>,
    /// Interaction-state setter installed by `Backend::attach_states` for
    /// nodes whose stylesheet declares `hovered`/`pressed` overlays. `None`
    /// for the many non-interactive views. We call it with
    /// `(StateBits::HOVERED, on)` from the tracking area's enter/exit and
    /// `(StateBits::PRESSED, on)` from mouseDown/Up; the framework
    /// re-resolves + re-applies the node's style.
    state_setter: RefCell<Option<Rc<dyn Fn(StateBits, bool)>>>,
    /// The live hover-tracking area, retained so `updateTrackingAreas` can
    /// remove the stale one before installing a fresh one. `None` until a
    /// `state_setter` is attached (only interactive views track hover).
    tracking_area: RefCell<Option<Retained<NSTrackingArea>>>,
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
            // macOS Ctrl-click is the system "secondary click". It can be
            // delivered as a left `mouseDown:` carrying the Control modifier while
            // the matching release arrives as `rightMouseUp:` (which we don't
            // observe) — so a touch begun here would never get its `Ended`, leaving
            // a dragged element stuck to the cursor. Treat Ctrl-click as a
            // secondary press: don't begin a touch (let super show any context
            // menu). `Began` is the only gate; the unaccepted state means the
            // following drag/up events are ignored too.
            const FLAG_CONTROL: usize = 1 << 18;
            let flags: usize = unsafe { msg_send![event, modifierFlags] };
            if flags & FLAG_CONTROL != 0 {
                let _: () = unsafe { msg_send![super(self), mouseDown: event] };
                return;
            }
            // Pressed-state feedback (no-op for views without a state setter).
            // Independent of touch dispatch so a styled button dims on press
            // whether or not it also carries an `on_touch` handler.
            self.flip_state(StateBits::PRESSED, true);
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
            self.flip_state(StateBits::PRESSED, false);
            if !self.dispatch_mouse(event, TouchPhase::Ended) {
                let _: () = unsafe { msg_send![super(self), mouseUp: event] };
            }
        }

        // Trackpad pinch → zoom. AppKit delivers `magnify:` with an
        // incremental `magnification` fraction (e.g. +0.02 per frame as the
        // fingers spread). We translate it to a normalized `WheelEvent` of
        // `WheelKind::Zoom`. Bubble to super when unhandled so a parent
        // magnifiable view still works.
        #[method(magnifyWithEvent:)]
        fn magnify_with_event(&self, event: &NSEvent) {
            if !self.dispatch_wheel(event, true) {
                let _: () = unsafe { msg_send![super(self), magnifyWithEvent: event] };
            }
        }

        // Two-finger trackpad scroll or mouse wheel → `WheelKind::Scroll`.
        #[method(scrollWheel:)]
        fn scroll_wheel(&self, event: &NSEvent) {
            if !self.dispatch_wheel(event, false) {
                let _: () = unsafe { msg_send![super(self), scrollWheel: event] };
            }
        }

        // Hover enter/exit, delivered via the tracking area installed in
        // `updateTrackingAreas`. Drives the `HOVERED` style state so a button
        // dims on hover on macOS, matching web's `:hover`.
        #[method(mouseEntered:)]
        fn mouse_entered(&self, _event: &NSEvent) {
            self.flip_state(StateBits::HOVERED, true);
        }

        #[method(mouseExited:)]
        fn mouse_exited(&self, _event: &NSEvent) {
            self.flip_state(StateBits::HOVERED, false);
        }

        // AppKit calls this when the view enters a window and on every
        // geometry change. We (re)build the hover tracking area here so it
        // always matches the current bounds. Only interactive views (those
        // with a `state_setter`) get one; everything else tracks nothing.
        #[method(updateTrackingAreas)]
        fn update_tracking_areas(&self) {
            let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };
            if let Some(old) = self.ivars().tracking_area.borrow_mut().take() {
                let _: () = unsafe { msg_send![self, removeTrackingArea: &*old] };
            }
            if self.ivars().state_setter.borrow().is_none() {
                return;
            }
            // `InVisibleRect` makes AppKit auto-size the area to the view's
            // visible rect (the passed rect is ignored and it stays correct
            // across resizes/scrolls); `ActiveInActiveApp` tracks while our
            // app is frontmost; `MouseEnteredAndExited` delivers the two
            // methods above. Owner is `self`, so they route here.
            let opts = NSTrackingAreaOptions::NSTrackingMouseEnteredAndExited
                | NSTrackingAreaOptions::NSTrackingActiveInActiveApp
                | NSTrackingAreaOptions::NSTrackingInVisibleRect;
            let mtm = MainThreadMarker::from(self);
            let area: Retained<NSTrackingArea> = unsafe {
                msg_send_id![
                    mtm.alloc::<NSTrackingArea>(),
                    initWithRect: CGRect::ZERO,
                    options: opts,
                    owner: self,
                    userInfo: std::ptr::null::<objc2::runtime::AnyObject>(),
                ]
            };
            let _: () = unsafe { msg_send![self, addTrackingArea: &*area] };
            *self.ivars().tracking_area.borrow_mut() = Some(area);
        }

        // AppKit calls this to (re)build the view's cursor rects whenever the
        // window's cursor rects are invalidated — on geometry changes and on
        // our explicit `invalidateCursorRectsForView:` in `set_cursor`. We
        // install a single rect over the whole bounds carrying the styled
        // cursor; with no styled cursor we add nothing, so the view shows the
        // OS default. Reading `self.bounds` here (not at set time) keeps the
        // rect correct across resizes.
        #[method(resetCursorRects)]
        fn reset_cursor_rects(&self) {
            let has_cursor = self.ivars().cursor.borrow().is_some();
            if has_cursor {
                let bounds: CGRect = unsafe { msg_send![self, bounds] };
                let vr: CGRect = unsafe { msg_send![self, visibleRect] };
                eprintln!(
                    "[CURSOR] resetCursorRects bounds=({:.0},{:.0},{:.0}x{:.0}) visibleRect=({:.0},{:.0},{:.0}x{:.0})",
                    bounds.origin.x, bounds.origin.y, bounds.size.width, bounds.size.height,
                    vr.origin.x, vr.origin.y, vr.size.width, vr.size.height
                );
            }
            if let Some(cursor) = self.ivars().cursor.borrow().as_ref() {
                let bounds: CGRect = unsafe { msg_send![self, bounds] };
                let _: () = unsafe { msg_send![self, addCursorRect: bounds, cursor: &**cursor] };
            }
        }

        // AppKit clips BOTH cursor rects (`resetCursorRects` above) AND
        // `NSTrackingInVisibleRect` hover areas (`updateTrackingAreas` above) to
        // a view's `visibleRect`. The default `visibleRect` is the bounds minus
        // whatever superviews clip away — and a zero-size ANCESTOR (a 0-height
        // `on_tap` wrapper, a Presence placeholder collapsed by its absolute-only
        // children) clips us to EMPTY. The view still PAINTS and hit-tests
        // (absolute positioning escapes the parent's box, and we special-case
        // hit-testing in `PresencePlaceholderView`), but an empty `visibleRect`
        // silently kills the pointer cursor and the hover state. Report our real
        // bounds in that collapsed case so both subsystems track the area we
        // actually occupy on screen.
        #[method(visibleRect)]
        fn visible_rect(&self) -> CGRect {
            let r: CGRect = unsafe { msg_send![super(self), visibleRect] };
            // Genuinely visible → use AppKit's answer unchanged.
            if r.size.width > 0.0 && r.size.height > 0.0 {
                return r;
            }
            // Nothing to rescue if we're truly zero-size ourselves.
            let bounds: CGRect = unsafe { msg_send![self, bounds] };
            if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
                return r;
            }
            // Inside a scroll view an empty `visibleRect` legitimately means
            // "scrolled out of the clip view" — don't override there, or we'd
            // give off-screen rows cursor rects + hover tracking.
            let mut cur: *mut NSView = unsafe { msg_send![self, superview] };
            while !cur.is_null() {
                let v: &NSView = unsafe { &*cur };
                if super::is_scroll_view(v) {
                    return r;
                }
                cur = unsafe { msg_send![v, superview] };
            }
            if self.ivars().cursor.borrow().is_some() {
                eprintln!(
                    "[CURSOR] visibleRect RESCUE → bounds=({:.0},{:.0},{:.0}x{:.0})",
                    bounds.origin.x, bounds.origin.y, bounds.size.width, bounds.size.height
                );
            }
            bounds
        }
    }
);

impl FlippedView {
    pub(crate) fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(FlippedViewIvars {
            handler: RefCell::new(None),
            wheel_handler: RefCell::new(None),
            active: Cell::new(false),
            cursor: RefCell::new(None),
            state_setter: RefCell::new(None),
            tracking_area: RefCell::new(None),
        });
        unsafe { msg_send_id![super(this), init] }
    }

    /// Install (or replace) the `on_touch` handler. Called by
    /// `Backend::install_touch_handler`.
    pub(crate) fn set_handler(&self, handler: TouchHandler) {
        *self.ivars().handler.borrow_mut() = Some(handler);
    }

    /// Install (or replace) the `on_wheel` handler. Called by
    /// `Backend::install_wheel_handler`.
    pub(crate) fn set_wheel_handler(&self, handler: WheelHandler) {
        *self.ivars().wheel_handler.borrow_mut() = Some(handler);
    }

    /// `true` if an `on_touch` handler has been installed — i.e. this view is
    /// an interactive control. The private-layer passthrough hit-test uses
    /// this to decide whether a click should be CAPTURED (a real control) or
    /// fall through to the app window beneath (a bare layout container). The
    /// macOS analogue of iOS's `IdealystTouchView::has_handler`.
    pub(crate) fn has_handler(&self) -> bool {
        self.ivars().handler.borrow().is_some()
    }

    /// Install the interaction-state setter (from `Backend::attach_states`)
    /// and build the hover tracking area. Idempotent — replacing the setter
    /// re-runs `updateTrackingAreas`, which swaps the area cleanly.
    pub(crate) fn set_state_setter(&self, setter: Rc<dyn Fn(StateBits, bool)>) {
        *self.ivars().state_setter.borrow_mut() = Some(setter);
        // Build the area now; AppKit also calls `updateTrackingAreas` once the
        // view is in a window and on later geometry changes.
        let _: () = unsafe { msg_send![self, updateTrackingAreas] };
    }

    /// Flip one interaction-state bit through the installed setter, if any.
    /// Snapshots the `Rc` first so the ivar borrow isn't held across the
    /// callback (it re-enters the backend to re-resolve + re-apply style).
    fn flip_state(&self, bit: StateBits, on: bool) {
        let setter = self.ivars().state_setter.borrow().clone();
        if let Some(s) = setter {
            s(bit, on);
        }
    }

    /// Set (or clear, with `None`) the hover cursor and ask the window to
    /// rebuild this view's cursor rects so the change takes effect without
    /// waiting for the next geometry pass. Called from `apply_style` when
    /// `StyleRules::cursor` is present; `None` restores the OS default.
    pub(crate) fn set_cursor(&self, cursor: Option<Retained<NSCursor>>) {
        *self.ivars().cursor.borrow_mut() = cursor;
        // `invalidateCursorRectsForView:` is a no-op when the view isn't in a
        // window yet; the rect is (re)built on the first `resetCursorRects`
        // after the view is mounted, so a pre-mount style apply still lands.
        let window: *mut objc2::runtime::AnyObject = unsafe { msg_send![self, window] };
        if !window.is_null() {
            let _: () = unsafe { msg_send![window, invalidateCursorRectsForView: self] };
        }
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

        // True top-left WINDOW coords. We can't reuse `local` (it's relative to
        // THIS view) for `window_position`: a drag handler that moves its own
        // view by the pointer delta would feed the moving frame back into the
        // delta → the widget never tracks the cursor and flickers (the macOS
        // "camera repositioning is janky" bug). Convert the window-space
        // location into the window's `contentView` — that view is the flipped,
        // full-window host, so its coordinate space IS top-left window space.
        let win_tl: CGPoint = unsafe {
            let window: *mut objc2::runtime::AnyObject = msg_send![self, window];
            if window.is_null() {
                local
            } else {
                let content: *mut objc2::runtime::AnyObject =
                    msg_send![window, contentView];
                if content.is_null() {
                    local
                } else {
                    msg_send![content, convertPoint: win, fromView: std::ptr::null::<NSView>()]
                }
            }
        };
        let ts: f64 = unsafe { msg_send![event, timestamp] };

        let ev = TouchEvent {
            id: TouchId(MOUSE_TOUCH_ID),
            phase,
            position: TouchPoint::new(local.x as f32, local.y as f32),
            window_position: TouchPoint::new(win_tl.x as f32, win_tl.y as f32),
            timestamp_ns: (ts * 1_000_000_000.0) as u64,
            force: None,
        };
        // Surface the keyboard modifiers for this event so a handler can read them
        // via `runtime_core::pointer_modifiers()` (e.g. Cmd/Shift-click to extend a
        // selection). Same `NSEventModifierFlags` bits the keyboard path uses.
        {
            const FLAG_SHIFT: usize = 1 << 17;
            const FLAG_CONTROL: usize = 1 << 18;
            const FLAG_OPTION: usize = 1 << 19;
            const FLAG_COMMAND: usize = 1 << 20;
            let flags: usize = unsafe { msg_send![event, modifierFlags] };
            set_pointer_modifiers(PointerModifiers {
                shift: flags & FLAG_SHIFT != 0,
                ctrl: flags & FLAG_CONTROL != 0,
                alt: flags & FLAG_OPTION != 0,
                meta: flags & FLAG_COMMAND != 0,
            });
        }
        // Auto-batch: run the handler inside a reactive `batch` so every signal
        // write it makes (a camera move writes pan_x, pan_y, zoom, + a repaint
        // tick) fans out its effects ONCE, after the handler returns, instead of
        // synchronously per write. Without this the canvas repaint effect runs
        // mid-update and presents inconsistent intermediate frames (pan moved,
        // zoom not yet) → visible flicker/jitter. Batching collapses the burst to
        // a single consistent render per input event — the native analogue of the
        // web renderer's rAF coalescing, and it lets app code drop manual
        // `batch(..)` around camera mutations.
        // Batching is automatic: the `on_touch` handler is wrapped in a
        // reactive cycle at attach time (see `runtime_core::cycle`), so a
        // burst of camera signal writes coalesces into one consistent render
        // per input event — no backend-side `batch()` needed. (Previously a
        // local `batch(..)` here; centralized so every backend gets it.)
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

    /// Translate one AppKit `magnify:` / `scrollWheel:` event into a
    /// [`WheelEvent`] and dispatch it. `is_zoom` selects the source: `true`
    /// for `magnifyWithEvent:` (a trackpad pinch — `WheelKind::Zoom`), `false`
    /// for `scrollWheel:` (`WheelKind::Scroll`). Returns `true` when the
    /// handler consumed the event (caller does NOT bubble to `super`).
    fn dispatch_wheel(&self, event: &NSEvent, is_zoom: bool) -> bool {
        let handler = match self.ivars().wheel_handler.borrow().as_ref() {
            Some(h) => h.clone(),
            None => return false,
        };

        // Same window→local conversion as `dispatch_mouse`: `isFlipped` makes
        // this top-left dp.
        let win: CGPoint = unsafe { msg_send![event, locationInWindow] };
        let local: CGPoint =
            unsafe { msg_send![self, convertPoint: win, fromView: std::ptr::null::<NSView>()] };
        let win_tl: CGPoint = unsafe {
            let window: *mut objc2::runtime::AnyObject = msg_send![self, window];
            if window.is_null() {
                local
            } else {
                let content: *mut objc2::runtime::AnyObject = msg_send![window, contentView];
                if content.is_null() {
                    local
                } else {
                    msg_send![content, convertPoint: win, fromView: std::ptr::null::<NSView>()]
                }
            }
        };
        let ts: f64 = unsafe { msg_send![event, timestamp] };

        let (kind, delta_x, delta_y, scale) = if is_zoom {
            // `NSEvent.magnification` is the incremental zoom fraction for this
            // event; `scale = 1 + magnification` is the per-event multiplier
            // the framework's normalized `WheelEvent::scale` expects (web's
            // ctrl+wheel maps onto the same scale via `exp()`).
            let magnification: f64 = unsafe { msg_send![event, magnification] };
            (WheelKind::Zoom, 0.0, 0.0, 1.0 + magnification as f32)
        } else {
            let dx: f64 = unsafe { msg_send![event, scrollingDeltaX] };
            let dy: f64 = unsafe { msg_send![event, scrollingDeltaY] };
            (WheelKind::Scroll, dx as f32, dy as f32, 1.0)
        };

        let we = WheelEvent {
            kind,
            delta_x,
            delta_y,
            scale,
            position: TouchPoint::new(local.x as f32, local.y as f32),
            window_position: TouchPoint::new(win_tl.x as f32, win_tl.y as f32),
            timestamp_ns: (ts * 1_000_000_000.0) as u64,
        };
        // Batching is automatic via the core `on_wheel` cycle wrapper (see
        // `dispatch_mouse` and `runtime_core::cycle`) — wheel pan/zoom writes to
        // several camera signals coalesce into one render per event.
        (handler)(&we).consumed
    }
}

/// Map a framework [`runtime_core::Cursor`] to the matching [`NSCursor`].
/// Returns `None` for `Auto` (and `None` means "install no cursor rect", so
/// the view shows the OS default). Values with no AppKit equivalent fall back
/// to the arrow — the honest default rather than a per-platform hack; web
/// still gets the precise keyword. Built via `msg_send` against the
/// `NSCursor` class methods so it doesn't depend on a specific binding's
/// method coverage.
pub(crate) fn cursor_for(c: runtime_core::Cursor) -> Option<Retained<NSCursor>> {
    use runtime_core::Cursor;
    // SAFETY: each is a documented `NSCursor` class method returning a
    // shared, autoreleased cursor; `msg_send_id` retains it.
    unsafe {
        let cls = objc2::class!(NSCursor);
        Some(match c {
            Cursor::Auto => return None,
            Cursor::Default | Cursor::Wait | Cursor::Progress | Cursor::Help | Cursor::Move => {
                msg_send_id![cls, arrowCursor]
            }
            Cursor::Pointer => msg_send_id![cls, pointingHandCursor],
            Cursor::Text => msg_send_id![cls, IBeamCursor],
            Cursor::NotAllowed => msg_send_id![cls, operationNotAllowedCursor],
            Cursor::Grab => msg_send_id![cls, openHandCursor],
            Cursor::Grabbing => msg_send_id![cls, closedHandCursor],
            Cursor::Crosshair => msg_send_id![cls, crosshairCursor],
            Cursor::ColResize | Cursor::EwResize => msg_send_id![cls, resizeLeftRightCursor],
            Cursor::RowResize | Cursor::NsResize => msg_send_id![cls, resizeUpDownCursor],
        })
    }
}

declare_class!(
    /// Non-interactive display label — the framework's `text()` primitive.
    ///
    /// Overrides `hitTest:` to return `nil` so mouse events (clicks AND
    /// scroll-wheel) pass THROUGH the text to whatever is behind/under it: a
    /// `Link` / `Pressable` parent that owns the tap, or an enclosing
    /// `NSScrollView` that owns the scroll. A plain `NSTextField` label sits in
    /// the hit-test path and `NSControl`'s mouse tracking SWALLOWS the click
    /// (and the scroll wheel) on a non-editable/non-selectable cell — so a
    /// button whose face is mostly its text never fired, and a page whose body
    /// is mostly text wouldn't scroll when the pointer was over the text. This
    /// is the macOS analogue of iOS labels' `userInteractionEnabled = false`.
    pub struct IdealystLabel;

    unsafe impl ClassType for IdealystLabel {
        type Super = NSTextField;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystLabel";
    }

    impl DeclaredClass for IdealystLabel {
        type Ivars = ();
    }

    unsafe impl IdealystLabel {
        #[method_id(hitTest:)]
        fn hit_test(&self, _point: CGPoint) -> Option<Retained<NSView>> {
            // Always decline: a display label must not capture mouse events.
            // AppKit then resolves the interactive view behind it.
            None
        }
    }
);

impl IdealystLabel {
    /// Create a hit-transparent display label configured like
    /// `+[NSTextField labelWithString:]` (non-editable, non-selectable, no
    /// bezel/border, transparent background). `create_text` applies the cell's
    /// wrap config + font/color styling on top, exactly as it did for the
    /// stock label.
    pub(crate) fn label_with_string(mtm: MainThreadMarker, s: &NSString) -> Retained<NSTextField> {
        let this: Retained<Self> =
            unsafe { msg_send_id![mtm.alloc::<Self>(), initWithFrame: CGRect::default()] };
        // Swap in an `IdealystLabelCell` so author `padding_*` on a `text()`
        // node insets the drawn glyphs (see that type). Must happen BEFORE the
        // configuration below so `setStringValue:` etc. land on the new cell.
        let cell = IdealystLabelCell::new(mtm);
        unsafe {
            let _: () = msg_send![&this, setCell: &*cell];
            let _: () = msg_send![&this, setStringValue: s];
            let _: () = msg_send![&this, setEditable: false];
            let _: () = msg_send![&this, setSelectable: false];
            let _: () = msg_send![&this, setBordered: false];
            let _: () = msg_send![&this, setBezeled: false];
            let _: () = msg_send![&this, setDrawsBackground: false];
        }
        // IdealystLabel IS-A NSTextField; expose it as the super type the rest
        // of the backend (measure_fn, MacosNode::Label) already speaks.
        Retained::into_super(this)
    }
}

/// Per-side text inset in points (top, left, bottom, right).
#[derive(Clone, Copy, Default)]
pub(crate) struct LabelInsets {
    pub top: f64,
    pub left: f64,
    pub bottom: f64,
    pub right: f64,
}

pub(crate) struct CellIvars {
    insets: StdCell<LabelInsets>,
}

declare_class!(
    /// `NSTextFieldCell` subclass that draws its text inset by author
    /// `padding_*`.
    ///
    /// The framework's `StyleRules.padding_*` is applied by Taffy as the text
    /// node's padding rect, which grows the label's outer frame but does NOT
    /// push the glyphs in — `NSTextFieldCell` paints at `cellFrame.origin`, so
    /// `text(style = padding: 12)` rendered its glyphs flush in a corner with the
    /// padding space dumped on the opposite sides. Overriding
    /// `drawInteriorWithFrame:inView:` to inset the frame by the same padding
    /// puts the glyphs in the content rect — the macOS analogue of iOS's
    /// `IdealystLabel.drawText(in:)` inset.
    ///
    /// We intentionally do NOT touch `cellSizeForBounds:`: Taffy keeps the
    /// padding on the node (reserving the outer size) and hands the measure the
    /// content-box width, so the glyphs wrap to the same width they're drawn in.
    /// Inset only the drawing — sizing already accounts for the padding.
    pub(crate) struct IdealystLabelCell;

    unsafe impl ClassType for IdealystLabelCell {
        type Super = NSTextFieldCell;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystLabelCell";
    }

    impl DeclaredClass for IdealystLabelCell {
        type Ivars = CellIvars;
    }

    unsafe impl IdealystLabelCell {
        #[method(drawInteriorWithFrame:inView:)]
        fn draw_interior(&self, frame: CGRect, view: &NSView) {
            let i = self.ivars().insets.get();
            let inset = CGRect {
                origin: CGPoint {
                    x: frame.origin.x + i.left,
                    y: frame.origin.y + i.top,
                },
                size: CGSize {
                    width: (frame.size.width - i.left - i.right).max(0.0),
                    height: (frame.size.height - i.top - i.bottom).max(0.0),
                },
            };
            let _: () = unsafe { msg_send![super(self), drawInteriorWithFrame: inset, inView: view] };
        }
    }
);

impl IdealystLabelCell {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(CellIvars {
            insets: StdCell::new(LabelInsets::default()),
        });
        let empty = NSString::from_str("");
        unsafe { msg_send_id![super(this), initTextCell: &*empty] }
    }

    /// Update the per-side text insets. Called by `apply_style` from the text
    /// node's `padding_*`. Repaints via the control view so the change shows
    /// without a relayout (`setNeedsDisplay:` is an `NSView` method — an
    /// `NSCell` redraws through its `controlView`).
    pub(crate) fn set_insets(&self, insets: LabelInsets) {
        self.ivars().insets.set(insets);
        let cv: *mut NSView = unsafe { msg_send![self, controlView] };
        if !cv.is_null() {
            let _: () = unsafe { msg_send![cv, setNeedsDisplay: true] };
        }
    }
}

/// Set per-side text insets on `label` if its cell is an [`IdealystLabelCell`].
/// `apply_style` calls this for every `MacosNode::Label` so author `padding_*`
/// on a `text()` node insets the glyphs. No-op for any other cell class.
pub(crate) fn set_label_insets(label: &NSView, insets: LabelInsets) {
    let cell: *mut NSTextFieldCell = unsafe { msg_send![label, cell] };
    if cell.is_null() {
        return;
    }
    let cls = IdealystLabelCell::class();
    let is_ours: bool = unsafe { msg_send![cell, isKindOfClass: cls] };
    if !is_ours {
        return;
    }
    // SAFETY: just confirmed the dynamic class is `IdealystLabelCell`.
    let cell: &IdealystLabelCell = unsafe { &*(cell as *const IdealystLabelCell) };
    cell.set_insets(insets);
}

