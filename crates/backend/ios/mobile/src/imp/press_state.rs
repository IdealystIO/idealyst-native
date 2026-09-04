//! `StateBits::PRESSED` for iOS.
//!
//! Every idea-ui control declares `state pressed` (and `state hovered`)
//! in its stylesheet, and on macOS, Linux and web those fire: AppKit
//! from `mouseDown`/`mouseUp`, GTK from `GestureClick`, web from the
//! generated `:active` rule. iOS drove NEITHER — `attach_states_impl`
//! installed a FOCUSED setter for text fields and no-op'd for everything
//! else — so on a phone every button, icon button, menu row, tab and
//! table row rendered permanently at rest. A press did nothing visible.
//!
//! # Why a recognizer and not `touchesBegan`
//!
//! The obvious hook is `IdealystTouchView`'s touch overrides, but a
//! `Pressable` is NOT one: `create_pressable_impl` mints a plain
//! `UIView` and hangs a `UITapGestureRecognizer` on it, and Pressable is
//! what Button, IconButton and the menu rows are built from. Subclassing
//! them all to catch touches would change the view class under every
//! control in the app.
//!
//! A zero-duration `UILongPressGestureRecognizer` reads the same touch
//! sequence from outside the view, so nothing about the view changes. It
//! also brings the cancel semantics for free: a long-press recognizer
//! FAILS once the touch moves past `allowableMovement`, which is exactly
//! "the finger started scrolling, so this was never a press".
//!
//! It must not steal the touch from anything: `cancelsTouchesInView` and
//! `delaysTouchesBegan`/`Ended` are all off, and the delegate answers YES
//! to simultaneous recognition, so the view's own tap recognizer, an
//! ancestor scroll view's pan and this all read the same touches.

use std::cell::RefCell;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::{declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_foundation::{MainThreadMarker, NSObject};
use objc2_ui_kit::UIView;
use runtime_shared::StateBits;

/// `UIGestureRecognizerState`. Only the terminal values matter here.
const STATE_BEGAN: isize = 1;
const STATE_ENDED: isize = 3;
const STATE_CANCELLED: isize = 4;
const STATE_FAILED: isize = 5;

pub(crate) struct PressTargetIvars {
    setter: RefCell<Option<Rc<dyn Fn(StateBits, bool)>>>,
}

declare_class!(
    pub(crate) struct PressTarget;

    unsafe impl ClassType for PressTarget {
        type Super = NSObject;
        type Mutability = mutability::InteriorMutable;
        const NAME: &'static str = "IdealystPressTarget";
    }

    impl DeclaredClass for PressTarget {
        type Ivars = PressTargetIvars;
    }

    unsafe impl PressTarget {
        /// The recognizer's action. `Began` marks the node pressed;
        /// every terminal state clears it — `Ended` for a completed
        /// press, `Cancelled` when UIKit takes the touch away (a
        /// scroll view's pan winning), `Failed` when the finger moved
        /// too far to still be a press. Missing any of those three is
        /// how a control latches on and stays lit.
        #[method(pressChanged:)]
        fn press_changed(&self, recognizer: &objc2_ui_kit::UIGestureRecognizer) {
            // ObjC action IMP (extern "C"): a panic in the style path
            // must abort loudly rather than unwind into UIKit.
            crate::imp::ffi_guard::guard_ffi("PressTarget::pressChanged", || {
                let state: isize = unsafe { msg_send![recognizer, state] };
                let on = match state {
                    STATE_BEGAN => true,
                    STATE_ENDED | STATE_CANCELLED | STATE_FAILED => false,
                    // `Possible`/`Changed` say nothing new — a zero-duration
                    // press reports `Changed` for every wobble inside the
                    // slop radius, and re-writing `true` there would churn
                    // the style path once per touch-move.
                    _ => return,
                };
                let setter = self.ivars().setter.borrow().as_ref().cloned();
                if let Some(setter) = setter {
                    setter(StateBits::PRESSED, on);
                }
            })
        }

        /// `UIGestureRecognizerDelegate`. YES for everything: this
        /// recognizer only OBSERVES, so failing to share would mean
        /// either the control's own tap or an ancestor's scroll losing
        /// its touches to a press highlight.
        #[method(gestureRecognizer:shouldRecognizeSimultaneouslyWithGestureRecognizer:)]
        fn should_recognize_simultaneously(
            &self,
            _recognizer: &objc2_ui_kit::UIGestureRecognizer,
            _other: &objc2_ui_kit::UIGestureRecognizer,
        ) -> objc2::runtime::Bool {
            objc2::runtime::Bool::YES
        }
    }
);

impl PressTarget {
    fn new(mtm: MainThreadMarker, setter: Rc<dyn Fn(StateBits, bool)>) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(PressTargetIvars {
            setter: RefCell::new(Some(setter)),
        });
        unsafe { msg_send_id![super(this), init] }
    }
}

/// Hang press tracking on `view`, reporting through `setter`.
///
/// Called from `attach_states_impl`, which the style layer invokes only
/// for nodes whose stylesheet actually declares states — so a view with
/// no `state pressed` arm never grows a recognizer.
pub(crate) fn install(
    mtm: MainThreadMarker,
    view: &UIView,
    setter: Rc<dyn Fn(StateBits, bool)>,
) -> Retained<PressTarget> {
    let target = PressTarget::new(mtm, setter);
    let gr: Retained<objc2_ui_kit::UILongPressGestureRecognizer> = unsafe {
        msg_send_id![
            mtm.alloc::<objc2_ui_kit::UILongPressGestureRecognizer>(),
            initWithTarget: &*target,
            action: objc2::sel!(pressChanged:),
        ]
    };
    unsafe {
        // Zero duration: report the press on touch-down, the way a
        // highlight has to feel. (The default 0.5s is a LONG press —
        // the gesture, not the state.)
        let _: () = msg_send![&*gr, setMinimumPressDuration: 0.0f64];
        // Observe, never consume — see the module note.
        let _: () = msg_send![&*gr, setCancelsTouchesInView: false];
        let _: () = msg_send![&*gr, setDelaysTouchesBegan: false];
        let _: () = msg_send![&*gr, setDelaysTouchesEnded: false];
        let _: () = msg_send![&*gr, setDelegate: &*target];
        let _: () = msg_send![view, addGestureRecognizer: &*gr];
    }
    target
}
