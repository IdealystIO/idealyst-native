//! App-level keyboard handling for the iOS backend.
//!
//! Unlike the per-`UITextField` key bridge (`TextKeyDelegate`, focus-scoped),
//! this installs an invisible `UIResponder`-in-the-chain view
//! (`IdealystKeyResponder`) that becomes first responder and overrides
//! `pressesBegan:withEvent:`, so it sees every HARDWARE key press when no text
//! input is focused, routing it through the framework's [`KeyDownHandler`].
//! Drives [`IosBackend::set_app_key_handler`](super::IosBackend).
//!
//! Hardware-keyboard only (the on-screen keyboard delivers text via the input
//! system, not `pressesBegan:`) — the mobile in-app gesture path (e.g. the
//! whiteboard's two-finger swipe) covers touch input. A focused text field is
//! the first responder and gets its own keys; this view reclaims them when the
//! field resigns is NOT automatic, so it's best-effort for app shortcuts.

use objc2::rc::Retained;
use objc2::{declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_foundation::{MainThreadMarker, NSObject, NSString};
use objc2_ui_kit::UIView;
use runtime_core::primitives::key::{KeyDownHandler, KeyEvent, KeyOutcome};
use std::cell::RefCell;

use super::IosBackend;

// `UIKeyModifierFlags` bits.
const FLAG_SHIFT: isize = 1 << 17;
const FLAG_CONTROL: isize = 1 << 18;
const FLAG_ALTERNATE: isize = 1 << 19;
const FLAG_COMMAND: isize = 1 << 20;

/// Map a `UIKeyboardHIDUsage` to a Web `KeyboardEvent.key` name for the named
/// keys (arrows + common editing keys); printable keys return `None` and fall
/// back to `charactersIgnoringModifiers`.
fn key_name_for_usage(usage: isize) -> Option<&'static str> {
    Some(match usage {
        0x50 => "ArrowLeft",
        0x4F => "ArrowRight",
        0x52 => "ArrowUp",
        0x51 => "ArrowDown",
        0x28 => "Enter",
        0x2B => "Tab",
        0x2A => "Backspace",
        0x29 => "Escape",
        0x4C => "Delete",
        _ => return None,
    })
}

/// Convert a `UIPress` (as a raw object) to a framework `KeyEvent`. Returns
/// `None` when the press carries no `key` (e.g. a game-controller button).
unsafe fn key_event_from_press(press: *mut NSObject) -> Option<KeyEvent> {
    let key: *mut NSObject = msg_send![press, key];
    if key.is_null() {
        return None;
    }
    let usage: isize = msg_send![key, keyCode];
    let flags: isize = msg_send![key, modifierFlags];
    let name = key_name_for_usage(usage).map(|s| s.to_string()).unwrap_or_else(|| {
        let s: *mut NSString = msg_send![key, charactersIgnoringModifiers];
        if s.is_null() {
            String::new()
        } else {
            (*s).to_string()
        }
    });
    Some(KeyEvent {
        key: name,
        shift: flags & FLAG_SHIFT != 0,
        ctrl: flags & FLAG_CONTROL != 0,
        alt: flags & FLAG_ALTERNATE != 0,
        meta: flags & FLAG_COMMAND != 0,
        selection_start: 0,
        selection_end: 0,
    })
}

pub(crate) struct KeyResponderIvars {
    pub(crate) handler: RefCell<Option<KeyDownHandler>>,
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

        #[method(pressesBegan:withEvent:)]
        fn presses_began(&self, presses: *mut NSObject, event: *mut NSObject) {
            let mut consumed = false;
            // Iterate the NSSet<UIPress> via its `allObjects` array.
            let arr: *mut NSObject = unsafe { msg_send![presses, allObjects] };
            if !arr.is_null() {
                let count: usize = unsafe { msg_send![arr, count] };
                for i in 0..count {
                    let press: *mut NSObject = unsafe { msg_send![arr, objectAtIndex: i] };
                    let Some(ev) = (unsafe { key_event_from_press(press) }) else {
                        continue;
                    };
                    // Guard the author key handler: pressesBegan: is an
                    // extern "C" IMP, so a panic here would unwind into
                    // UIKit's press routing (UB) — abort loudly instead.
                    let outcome = crate::imp::ffi_guard::guard_ffi(
                        "IdealystKeyResponder::pressesBegan",
                        || {
                            let h = self.ivars().handler.borrow();
                            match h.as_ref() {
                                Some(handler) => handler(&ev),
                                None => KeyOutcome::Default,
                            }
                        },
                    );
                    if matches!(outcome, KeyOutcome::PreventDefault) {
                        consumed = true;
                    }
                }
            }
            if !consumed {
                // Unhandled → bubble up the responder chain.
                let _: () = unsafe { msg_send![super(self), pressesBegan: presses, withEvent: event] };
            }
        }
    }
);

impl IdealystKeyResponder {
    fn new(mtm: MainThreadMarker, handler: KeyDownHandler) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(KeyResponderIvars { handler: RefCell::new(Some(handler)) });
        unsafe { msg_send_id![super(this), init] }
    }
}

/// Install (or, with `None`, remove) the app-level key responder. Replacing
/// first tears down the previous responder.
pub(crate) fn set_app_key_handler(backend: &mut IosBackend, handler: Option<KeyDownHandler>) {
    if let Some(prev) = backend.app_key_responder.take() {
        unsafe {
            let _: bool = msg_send![&*prev, resignFirstResponder];
            let _: () = msg_send![&*prev, removeFromSuperview];
        }
    }
    let Some(handler) = handler else {
        return;
    };
    let Some(host) = backend.host_root.clone() else {
        return;
    };
    let responder = IdealystKeyResponder::new(backend.mtm, handler);
    unsafe {
        // Add to the host view (zero frame — invisible) so it's in the window's
        // responder chain, then make it first responder to receive key presses.
        let _: () = msg_send![&*host, addSubview: &*responder];
        let _: bool = msg_send![&*responder, becomeFirstResponder];
    }
    backend.app_key_responder = Some(responder);
}
