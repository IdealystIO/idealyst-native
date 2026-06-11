//! Raw touch event delivery for the iOS backend.
//!
//! [`IdealystTouchView`] is a `UIView` subclass that overrides the
//! four touch entry points (`touchesBegan:`/`Moved:`/`Ended:`/
//! `Cancelled:`) and routes the events to a framework-installed
//! [`TouchHandler`].
//!
//! The framework's View primitive creates instances of this class
//! (via `create_view`) instead of plain `UIView`, so a later
//! [`Backend::install_touch_handler`](runtime_core::Backend::install_touch_handler)
//! drops the handler into the view's ivars without recreating the
//! node. Views with no installed handler dispatch touches straight
//! to `super` — overhead is one method call per touch event.
//!
//! See `docs/native-touch-backends-plan.md` for the design.

use std::cell::RefCell;
use std::collections::HashSet;

use runtime_core::{TouchEvent, TouchHandler, TouchId, TouchPhase, TouchPoint};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_foundation::{CGFloat, CGPoint, MainThreadMarker, NSArray, NSSet};
use objc2_ui_kit::{UIEvent, UITouch, UIView};

pub(crate) struct TouchViewIvars {
    /// Installed by `Backend::install_touch_handler`. Wrapped in
    /// `RefCell<Option<_>>` so the framework can swap or remove the
    /// handler later (currently install-only).
    handler: RefCell<Option<TouchHandler>>,
    /// Touch ids whose `Began` we consumed. Subsequent
    /// `Moved`/`Ended`/`Cancelled` for these ids invoke the handler
    /// with the corresponding phase. UIKit's natural routing already
    /// delivers them to us (we won at `Began` by not calling super);
    /// the set lets us be explicit about per-touch consumption.
    active_touches: RefCell<HashSet<u64>>,
}

declare_class!(
    pub(crate) struct IdealystTouchView;

    unsafe impl ClassType for IdealystTouchView {
        type Super = UIView;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystTouchView";
    }

    impl DeclaredClass for IdealystTouchView {
        type Ivars = TouchViewIvars;
    }

    unsafe impl IdealystTouchView {
        #[method(touchesBegan:withEvent:)]
        fn touches_began(&self, touches: &NSSet<UITouch>, event: Option<&UIEvent>) {
            let any_consumed = self.dispatch_touches(touches, TouchPhase::Began);
            if !any_consumed {
                // Unconsumed → call super so UIKit's responder
                // chain bubbles the touch to the next responder
                // (typically the superview). Subsequent events
                // for the same touches will be routed by UIKit
                // to whichever ancestor consumed `Began`.
                let _: () = unsafe { msg_send![super(self), touchesBegan: touches, withEvent: event] };
            }
        }

        #[method(touchesMoved:withEvent:)]
        fn touches_moved(&self, touches: &NSSet<UITouch>, event: Option<&UIEvent>) {
            let any_consumed = self.dispatch_touches(touches, TouchPhase::Moved);
            if !any_consumed {
                let _: () = unsafe { msg_send![super(self), touchesMoved: touches, withEvent: event] };
            }
        }

        #[method(touchesEnded:withEvent:)]
        fn touches_ended(&self, touches: &NSSet<UITouch>, event: Option<&UIEvent>) {
            let any_consumed = self.dispatch_touches(touches, TouchPhase::Ended);
            if !any_consumed {
                let _: () = unsafe { msg_send![super(self), touchesEnded: touches, withEvent: event] };
            }
        }

        #[method(touchesCancelled:withEvent:)]
        fn touches_cancelled(&self, touches: &NSSet<UITouch>, event: Option<&UIEvent>) {
            // Always dispatch to the handler (it may need to reset
            // recognizer state) but also always call super so UIKit
            // can clean up its own routing tables. Calling super on
            // a Cancelled doesn't bubble a new gesture — it just
            // releases UIKit's bookkeeping.
            let _ = self.dispatch_touches(touches, TouchPhase::Cancelled);
            let _: () = unsafe { msg_send![super(self), touchesCancelled: touches, withEvent: event] };
        }
    }
);

impl IdealystTouchView {
    pub(crate) fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(TouchViewIvars {
            handler: RefCell::new(None),
            active_touches: RefCell::new(HashSet::new()),
        });
        let this: Retained<Self> = unsafe { msg_send_id![super(this), init] };
        // multipleTouchEnabled = YES so every finger reaches us, not
        // just the primary. Single-finger callers don't care; we
        // filter for the cases that do.
        let _: () = unsafe { msg_send![&*this, setMultipleTouchEnabled: true] };
        // userInteractionEnabled defaults to YES on `UIView` but we
        // set explicitly in case a future subclass changes the
        // default.
        let _: () = unsafe { msg_send![&*this, setUserInteractionEnabled: true] };
        this
    }

    /// Replace any previously-installed handler. Called by the
    /// `Backend::install_touch_handler` impl on `IosBackend`.
    pub(crate) fn set_handler(&self, handler: TouchHandler) {
        *self.ivars().handler.borrow_mut() = Some(handler);
    }

    /// Whether an `on_touch` handler is currently installed. Every framework
    /// `View` is minted as an `IdealystTouchView`, so the class alone can't
    /// distinguish interactive controls from plain layout containers — only a
    /// set handler does. The private-layer passthrough hit-test
    /// (`callbacks::private_layer_blocks_touch`) uses this to treat
    /// handler-bearing views as touch-blocking while letting taps fall through
    /// handler-less transparent containers.
    pub(crate) fn has_handler(&self) -> bool {
        self.ivars().handler.borrow().is_some()
    }

    /// Dispatch each `UITouch` in the set to the installed handler,
    /// honoring the bubble decision per-touch. Returns whether ANY
    /// touch in the set was consumed — the touch override uses this
    /// to decide whether to call `super`.
    ///
    /// UIKit doesn't support per-touch bubbling within a single
    /// method call: the choice to call super applies to the whole
    /// set. v1 semantics: if any touch in the set is consumed, the
    /// whole set is consumed (no super). This matches typical
    /// responder implementations and rarely produces user-visible
    /// effects since concurrent touches landing on one view in the
    /// same frame is uncommon.
    fn dispatch_touches(&self, touches: &NSSet<UITouch>, phase: TouchPhase) -> bool {
        // ObjC-dispatched via touchesBegan/Moved/Ended/Cancelled, whose
        // `declare_class!` IMPs are plain `extern "C"`. Guard the body so
        // a panic in author `on_touch` code aborts loudly instead of
        // unwinding across the boundary into UIKit (UB).
        crate::imp::ffi_guard::guard_ffi("IdealystTouchView::dispatch_touches", || {
            self.dispatch_touches_inner(touches, phase)
        })
    }

    fn dispatch_touches_inner(&self, touches: &NSSet<UITouch>, phase: TouchPhase) -> bool {
        // Snapshot the handler ref so we don't hold the borrow
        // across the closure invocation (the closure may call back
        // into the framework which could re-enter ivars).
        let handler = match self.ivars().handler.borrow().as_ref() {
            Some(h) => h.clone(),
            None => return false,
        };

        // NSSet→NSArray for stable indexed iteration. allObjects
        // is the standard objc bridge; cheap on small sets.
        let all: Retained<NSArray<UITouch>> = unsafe { msg_send_id![touches, allObjects] };

        let mut any_consumed = false;
        let count = all.len();
        for i in 0..count {
            // SAFETY: `i < count`; the array is freshly produced by
            // `allObjects` and not mutated mid-loop.
            let touch = unsafe { all.objectAtIndex(i) };
            let touch_id_u64 = touch_id_for(&touch);

            // Move/End/Cancel are only delivered to handlers that
            // consumed the corresponding Began. Without this gate, a
            // touch we never accepted would still reach us during
            // its lifecycle through UIKit's natural routing once an
            // ancestor consumed Began but we happened to receive
            // forwarded events.
            if matches!(phase, TouchPhase::Moved | TouchPhase::Ended | TouchPhase::Cancelled) {
                if !self.ivars().active_touches.borrow().contains(&touch_id_u64) {
                    continue;
                }
            }

            let event = self.make_touch_event(&touch, phase);
            let response = (handler)(&event);

            if response.consumed {
                any_consumed = true;
                if matches!(phase, TouchPhase::Began) {
                    self.ivars().active_touches.borrow_mut().insert(touch_id_u64);
                }
            }

            // Terminal phases drop the active flag regardless of
            // consume — we won't see this touch again.
            if matches!(phase, TouchPhase::Ended | TouchPhase::Cancelled) {
                self.ivars().active_touches.borrow_mut().remove(&touch_id_u64);
            }

            if response.claim {
                claim_touch_internal(self);
            }
        }
        any_consumed
    }

    fn make_touch_event(&self, touch: &UITouch, phase: TouchPhase) -> TouchEvent {
        // `locationInView: self` → view-local coords.
        // `locationInView: nil` → window-relative coords (UIKit
        // convention: nil means the window).
        let local: CGPoint = unsafe { msg_send![touch, locationInView: self] };
        let window: CGPoint = unsafe {
            msg_send![touch, locationInView: std::ptr::null::<UIView>()]
        };
        // `UITouch.timestamp` is NSTimeInterval (seconds, double).
        let timestamp: f64 = unsafe { msg_send![touch, timestamp] };
        // Force normalization. `maximumPossibleForce` is 0 on
        // devices that don't report pressure; we surface `None` in
        // that case so handlers can branch on Option<f32>.
        let force_raw: CGFloat = unsafe { msg_send![touch, force] };
        let max_force: CGFloat = unsafe { msg_send![touch, maximumPossibleForce] };
        let force = if max_force > 0.0 {
            Some((force_raw / max_force) as f32)
        } else {
            None
        };
        TouchEvent {
            id: TouchId(touch_id_for(touch)),
            phase,
            position: TouchPoint::new(local.x as f32, local.y as f32),
            window_position: TouchPoint::new(window.x as f32, window.y as f32),
            timestamp_ns: (timestamp * 1_000_000_000.0) as u64,
            force,
        }
    }
}

/// Stable id for a `UITouch` through one gesture lifecycle. Apple
/// guarantees the same `UITouch` instance is reused for the same
/// finger from Began through Ended/Cancelled; the object's
/// pointer-as-integer is therefore a valid `TouchId`.
fn touch_id_for(touch: &UITouch) -> u64 {
    touch as *const UITouch as usize as u64
}

/// Implementation of `Backend::claim_touch` for iOS.
///
/// Walks up the responder chain from `view` looking for any
/// `UIScrollView` (or subclass — `UICollectionView`, `UITableView`,
/// etc.) and toggles its pan recognizer's enabled state. This is
/// the canonical "cancel in-flight pan" pattern on iOS: flipping
/// `enabled` to NO and back forces UIKit to transition the
/// recognizer to Cancelled and re-arm for the next gesture, which
/// stops any scroll currently in progress.
///
/// Walks the whole chain (not just the nearest scroll view) so
/// nested scroll containers all get the cancel.
pub(crate) fn claim_touch_internal(view: &UIView) {
    let scroll_class = objc2::class!(UIScrollView);
    let mut current: Option<Retained<UIView>> = unsafe { msg_send_id![view, superview] };
    while let Some(ancestor) = current {
        let is_scroll: bool = unsafe { msg_send![&ancestor, isKindOfClass: scroll_class] };
        if is_scroll {
            let pan: Option<Retained<AnyObject>> = unsafe {
                msg_send_id![&ancestor, panGestureRecognizer]
            };
            if let Some(pan) = pan {
                let _: () = unsafe { msg_send![&pan, setEnabled: false] };
                let _: () = unsafe { msg_send![&pan, setEnabled: true] };
            }
        }
        current = unsafe { msg_send_id![&ancestor, superview] };
    }
}
