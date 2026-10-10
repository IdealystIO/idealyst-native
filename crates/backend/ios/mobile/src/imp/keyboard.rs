//! App-level keyboard source for the iOS backend.
//!
//! Unlike the per-`UITextField` key bridge (`TextKeyDelegate`, focus-scoped),
//! this installs an invisible `UIResponder`-in-the-chain view
//! (`IdealystKeyResponder`) that becomes first responder and overrides all
//! four `presses*` methods, so it sees every HARDWARE key press AND release
//! while it holds first responder, delivering each as an [`AppKeyEvent`] into
//! the framework's [`KeyboardSink`]. Drives `IosBackend::set_keyboard_sink_impl`.
//!
//! - **Down + up**: `pressesBegan:` → `Down`; `pressesEnded:` and
//!   `pressesCancelled:` → `Up` (a cancelled press is released as far as the
//!   app is concerned). Modifier keys are presses in their own right on iOS
//!   (`UIKeyboardHIDUsageKeyboardLeftShift`, …), so they arrive here too.
//! - **`code`** comes from the `UIKey`'s HID usage, never its character
//!   (`key_input_policy::physical_code`). iOS doesn't flag auto-repeat; the
//!   dispatcher marks a down for an already-held key as a repeat.
//! - **`PreventDefault`** on a press's `began` swallows that press: neither its
//!   begin nor its end reaches `super`. Unclaimed presses are forwarded up the
//!   responder chain. `key_input_policy::PressLedger` keeps each press's end
//!   routed like its begin, which UIKit requires of a responder that doesn't
//!   forward every press.
//! - **Focus loss** — the app resigning active
//!   (`UIApplicationWillResignActiveNotification`) or this view resigning
//!   first responder (e.g. a text field took focus) — calls
//!   [`KeyboardSink::focus_lost`]: the releases for keys held then never come
//!   here, so the dispatcher synthesizes them.
//!
//! Hardware-keyboard only (the on-screen keyboard delivers text via the input
//! system, not `presses*`). A focused text field is the first responder and
//! gets its own keys; this view is not in that field's responder chain, so it
//! sees nothing until it is first responder again (reclaiming focus when the
//! field resigns is NOT automatic).

use std::cell::RefCell;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_foundation::{MainThreadMarker, NSObject, NSString};
use objc2_ui_kit::UIView;
use runtime_shared::primitives::key::{KeyOutcome, KeyPhase, KeyboardSink};

use super::IosBackend;
use crate::key_input_policy::{translate, PressLedger};

/// The live sink, shared with deferred focus-loss microtasks. Callers CLONE
/// the sink out and drop the borrow before calling it: a listener may
/// add/remove listeners, which re-enters `set_keyboard_sink` on the backend.
type SinkSlot = Rc<RefCell<Option<KeyboardSink>>>;

fn current_sink(slot: &SinkSlot) -> Option<KeyboardSink> {
    slot.borrow().clone()
}

/// Call `focus_lost` on the NEXT main-queue turn. Both triggers can fire
/// synchronously inside UIKit work the framework itself started (a flush
/// focusing a text field makes this view resign first responder), and the
/// synthesized releases run author listeners — so they run after the current
/// call stack unwinds. The slot is read at fire time: a teardown in between
/// clears it, and nothing is delivered to a removed sink.
fn defer_focus_lost(slot: &SinkSlot) {
    let slot = slot.clone();
    runtime_shared::schedule_microtask(move || {
        if let Some(sink) = current_sink(&slot) {
            sink.focus_lost();
        }
    });
}

/// Translate a `UIPress` (raw object) for `phase`. `None` when the press
/// carries no `key` (e.g. a game-controller / remote button).
unsafe fn app_key_event_from_press(
    press: *mut NSObject,
    phase: KeyPhase,
) -> Option<runtime_shared::primitives::key::AppKeyEvent> {
    let key: *mut NSObject = msg_send![press, key];
    if key.is_null() {
        return None;
    }
    let usage: isize = msg_send![key, keyCode];
    let flags: isize = msg_send![key, modifierFlags];
    let s: *mut NSString = msg_send![key, charactersIgnoringModifiers];
    let chars = if s.is_null() { None } else { Some((*s).to_string()) };
    Some(translate(phase, usage, flags, chars.as_deref()))
}

/// The presses in an `NSSet<UIPress>`.
unsafe fn presses_of(set: *mut NSObject) -> Vec<*mut NSObject> {
    let arr: *mut NSObject = msg_send![set, allObjects];
    if arr.is_null() {
        return Vec::new();
    }
    let count: usize = msg_send![arr, count];
    (0..count).map(|i| msg_send![arr, objectAtIndex: i]).collect()
}

/// The set to hand `super` for the presses to forward: the original set when
/// all of them are forwarded, a fresh set of just those when some are, `None`
/// when none are (don't call `super` at all).
unsafe fn forward_set(original: *mut NSObject, all: &[*mut NSObject], keep: &[*mut NSObject]) -> Option<Retained<NSObject>> {
    if keep.is_empty() {
        return None;
    }
    if keep.len() == all.len() {
        return Retained::retain(original);
    }
    let set: Retained<NSObject> = msg_send_id![objc2::class!(NSMutableSet), set];
    for press in keep {
        let _: () = msg_send![&*set, addObject: *press];
    }
    Some(set)
}

pub(crate) struct KeyResponderIvars {
    sink: SinkSlot,
    ledger: RefCell<PressLedger>,
}

declare_class!(
    pub(crate) struct IdealystKeyResponder;

    unsafe impl ClassType for IdealystKeyResponder {
        type Super = UIView;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystKeyResponder";
    }

    impl DeclaredClass for IdealystKeyResponder {
        type Ivars = KeyResponderIvars;
    }

    unsafe impl IdealystKeyResponder {
        #[method(canBecomeFirstResponder)]
        fn can_become_first_responder(&self) -> bool {
            true
        }

        #[method(resignFirstResponder)]
        fn resign_first_responder(&self) -> bool {
            let resigned: bool = unsafe { msg_send![super(self), resignFirstResponder] };
            if resigned {
                crate::imp::ffi_guard::guard_ffi("IdealystKeyResponder::resignFirstResponder", || {
                    self.ivars().ledger.borrow_mut().clear();
                    defer_focus_lost(&self.ivars().sink);
                });
            }
            resigned
        }

        #[method(appWillResignActive:)]
        fn app_will_resign_active(&self, _note: *mut NSObject) {
            crate::imp::ffi_guard::guard_ffi("IdealystKeyResponder::appWillResignActive", || {
                defer_focus_lost(&self.ivars().sink);
            });
        }

        #[method(pressesBegan:withEvent:)]
        fn presses_began(&self, presses: *mut NSObject, event: *mut NSObject) {
            // Guard the author listeners: pressesBegan: is an extern "C" IMP,
            // so a panic here would unwind into UIKit's press routing (UB) —
            // abort loudly instead.
            let forward = crate::imp::ffi_guard::guard_ffi("IdealystKeyResponder::pressesBegan", || unsafe {
                let all = presses_of(presses);
                let mut keep = Vec::with_capacity(all.len());
                for &press in &all {
                    let swallow = match app_key_event_from_press(press, KeyPhase::Down) {
                        Some(ev) => match current_sink(&self.ivars().sink) {
                            Some(sink) => matches!(sink.key(&ev), KeyOutcome::PreventDefault),
                            None => false,
                        },
                        None => false,
                    };
                    if self.ivars().ledger.borrow_mut().began(press as usize, swallow) {
                        keep.push(press);
                    }
                }
                forward_set(presses, &all, &keep)
            });
            if let Some(set) = forward {
                // Unclaimed → bubble up the responder chain.
                let _: () = unsafe { msg_send![super(self), pressesBegan: &*set, withEvent: event] };
            }
        }

        #[method(pressesEnded:withEvent:)]
        fn presses_ended(&self, presses: *mut NSObject, event: *mut NSObject) {
            let forward = crate::imp::ffi_guard::guard_ffi("IdealystKeyResponder::pressesEnded", || unsafe {
                self.release(presses)
            });
            if let Some(set) = forward {
                let _: () = unsafe { msg_send![super(self), pressesEnded: &*set, withEvent: event] };
            }
        }

        #[method(pressesCancelled:withEvent:)]
        fn presses_cancelled(&self, presses: *mut NSObject, event: *mut NSObject) {
            let forward = crate::imp::ffi_guard::guard_ffi("IdealystKeyResponder::pressesCancelled", || unsafe {
                self.release(presses)
            });
            if let Some(set) = forward {
                let _: () = unsafe { msg_send![super(self), pressesCancelled: &*set, withEvent: event] };
            }
        }
    }
);

impl IdealystKeyResponder {
    fn new(mtm: MainThreadMarker, sink: SinkSlot) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(KeyResponderIvars { sink, ledger: RefCell::new(PressLedger::default()) });
        unsafe { msg_send_id![super(this), init] }
    }

    /// Shared body of `pressesEnded:` / `pressesCancelled:`: deliver an `Up`
    /// for each keyed press, and return the presses to forward — exactly those
    /// whose `began` was forwarded. A release's own outcome can't change the
    /// routing: UIKit needs each press's end to go where its begin went.
    unsafe fn release(&self, presses: *mut NSObject) -> Option<Retained<NSObject>> {
        let all = presses_of(presses);
        let mut keep = Vec::with_capacity(all.len());
        for &press in &all {
            if let Some(ev) = app_key_event_from_press(press, KeyPhase::Up) {
                if let Some(sink) = current_sink(&self.ivars().sink) {
                    let _ = sink.key(&ev);
                }
            }
            if self.ivars().ledger.borrow_mut().ended(press as usize) {
                keep.push(press);
            }
        }
        forward_set(presses, &all, &keep)
    }
}

/// The installed app-level key source: the responder view and the slot its
/// sink lives in (swappable without re-taking first responder).
pub(crate) struct AppKeyboard {
    sink: SinkSlot,
    responder: Retained<IdealystKeyResponder>,
}

impl AppKeyboard {
    fn teardown(self) {
        // Clear the slot FIRST: resigning below would otherwise queue a
        // focus_lost into the sink being removed.
        self.sink.borrow_mut().take();
        unsafe {
            let center: *mut AnyObject = msg_send![objc2::class!(NSNotificationCenter), defaultCenter];
            let _: () = msg_send![center, removeObserver: &*self.responder];
            let _: bool = msg_send![&*self.responder, resignFirstResponder];
            let _: () = msg_send![&*self.responder, removeFromSuperview];
        }
    }
}

/// Install (or, with `None`, remove) the app-level key responder. A `Some`
/// while installed swaps the sink in place — it does NOT re-take first
/// responder, which would steal focus from a text field the user is in.
pub(crate) fn set_keyboard_sink(backend: &mut IosBackend, sink: Option<KeyboardSink>) {
    let Some(sink) = sink else {
        if let Some(kb) = backend.app_keyboard.take() {
            kb.teardown();
        }
        return;
    };
    if let Some(kb) = backend.app_keyboard.as_ref() {
        *kb.sink.borrow_mut() = Some(sink);
        return;
    }
    let Some(host) = backend.host_root.clone() else {
        return;
    };
    let slot: SinkSlot = Rc::new(RefCell::new(Some(sink)));
    let responder = IdealystKeyResponder::new(backend.mtm, slot.clone());
    unsafe {
        // Add to the host view (zero frame — invisible) so it's in the window's
        // responder chain, then make it first responder to receive key presses.
        let _: () = msg_send![&*host, addSubview: &*responder];
        let _: bool = msg_send![&*responder, becomeFirstResponder];
        // App deactivation (app switcher, Control Center, an incoming call):
        // held keys' releases go nowhere. NSNotificationCenter doesn't retain
        // the observer; `AppKeyboard` does, and teardown removes it.
        let center: *mut AnyObject = msg_send![objc2::class!(NSNotificationCenter), defaultCenter];
        let name = NSString::from_str("UIApplicationWillResignActiveNotification");
        let nil: *mut AnyObject = std::ptr::null_mut();
        let _: () = msg_send![
            center,
            addObserver: &*responder,
            selector: objc2::sel!(appWillResignActive:),
            name: &*name,
            object: nil,
        ];
    }
    backend.app_keyboard = Some(AppKeyboard { sink: slot, responder });
}
