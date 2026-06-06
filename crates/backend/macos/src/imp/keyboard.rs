//! App-level keyboard handling for the macOS backend.
//!
//! Unlike the per-`NSTextField` `on_key_down` (focus-scoped), this installs a
//! single `NSEvent addLocalMonitorForEventsMatchingMask:NSEventMaskKeyDown`
//! monitor that sees every key press in the app regardless of focus, and routes
//! it through the framework's [`KeyDownHandler`]. Drives
//! [`MacosBackend::set_app_key_handler`](super::MacosBackend).

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::{class, msg_send};
use objc2_foundation::{NSObject, NSString};
use runtime_core::primitives::key::{KeyDownHandler, KeyEvent, KeyOutcome};

use super::MacosBackend;

/// `NSEventMaskKeyDown` = `1 << NSEventTypeKeyDown(10)`.
const NS_EVENT_MASK_KEY_DOWN: usize = 1 << 10;

// `NSEventModifierFlags` bits we surface.
const FLAG_SHIFT: usize = 1 << 17;
const FLAG_CONTROL: usize = 1 << 18;
const FLAG_OPTION: usize = 1 << 19;
const FLAG_COMMAND: usize = 1 << 20;

/// Map an AppKit virtual key code to a Web `KeyboardEvent.key` name for the
/// non-printable keys, so the cross-backend vocabulary matches (arrows + the
/// common editing keys). Printable keys return `None` and fall back to
/// `charactersIgnoringModifiers` (so `+`/`-`/`=`/letters are themselves).
fn key_name_for_code(code: u16) -> Option<&'static str> {
    Some(match code {
        123 => "ArrowLeft",
        124 => "ArrowRight",
        125 => "ArrowDown",
        126 => "ArrowUp",
        36 | 76 => "Enter", // Return + keypad Enter
        48 => "Tab",
        49 => " ", // Space — Web reports a literal space
        51 => "Backspace",
        53 => "Escape",
        117 => "Delete", // forward delete
        _ => return None,
    })
}

/// Build a framework `KeyEvent` from an `NSEvent` (key-down). Selection fields
/// are 0 — an app-level handler has no associated text field.
unsafe fn key_event_from_nsevent(event: *mut NSObject) -> KeyEvent {
    let key_code: u16 = msg_send![event, keyCode];
    let flags: usize = msg_send![event, modifierFlags];
    let key = key_name_for_code(key_code).map(|s| s.to_string()).unwrap_or_else(|| {
        let s: *mut NSString = msg_send![event, charactersIgnoringModifiers];
        if s.is_null() {
            String::new()
        } else {
            (*s).to_string()
        }
    });
    KeyEvent {
        key,
        shift: flags & FLAG_SHIFT != 0,
        ctrl: flags & FLAG_CONTROL != 0,
        alt: flags & FLAG_OPTION != 0,
        meta: flags & FLAG_COMMAND != 0,
        selection_start: 0,
        selection_end: 0,
    }
}

/// Install (or, with `None`, remove) the app-level key monitor on `backend`.
/// Replacing first removes the previous monitor; `None` just removes.
pub(crate) fn set_app_key_handler(backend: &mut MacosBackend, handler: Option<KeyDownHandler>) {
    // Tear down any existing monitor.
    if let Some(monitor) = backend.app_key_monitor.take() {
        unsafe {
            let _: () = msg_send![class!(NSEvent), removeMonitor: &*monitor];
        }
    }
    let Some(handler) = handler else {
        return;
    };

    // The monitor block: convert the NSEvent → KeyEvent, call the handler, and
    // return `nil` to SWALLOW the event when the handler claims it
    // (`PreventDefault`) so AppKit doesn't `NSBeep` on an unhandled key; return
    // the event unchanged otherwise so normal key routing continues. The catch
    // is crash-loud per project policy (an FFI callback that unwinds aborts).
    let block = RcBlock::new(move |event: *mut NSObject| -> *mut NSObject {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            let ev = key_event_from_nsevent(event);
            handler(&ev)
        }));
        match result {
            Ok(KeyOutcome::PreventDefault) => std::ptr::null_mut(),
            Ok(KeyOutcome::Default) => event,
            Err(_) => {
                eprintln!("[backend-macos] app key handler panicked");
                std::process::abort();
            }
        }
    });

    // `addLocalMonitor…` copies the handler block internally, so the local
    // `block` may drop after this; we retain the returned monitor token to feed
    // `removeMonitor:` later.
    let monitor: *mut NSObject = unsafe {
        msg_send![
            class!(NSEvent),
            addLocalMonitorForEventsMatchingMask: NS_EVENT_MASK_KEY_DOWN,
            handler: &*block,
        ]
    };
    backend.app_key_monitor = unsafe { Retained::retain(monitor) };
}
