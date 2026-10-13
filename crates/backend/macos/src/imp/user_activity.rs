//! App-wide user-activity source for the macOS backend
//! (`runtime_shared::user_activity` — read its module docs for the
//! contract). Installed once per boot, unconditionally, by
//! `newcore::start_with`.
//!
//! - **Source**: one `NSEvent addLocalMonitorForEventsMatchingMask:` monitor
//!   for every press-like event ([`ACTIVITY_MASK`]). A local monitor runs
//!   inside `-[NSApplication sendEvent:]` for EVERY window of the app — the
//!   host window, portal windows, panels, sheets and the field editor of a
//!   focused `NSTextField` / `NSTextView` (typing arrives as key-down events
//!   through `sendEvent:` like any other key) — so no view can hide an
//!   interaction from it the way a button hides a press from a parent's
//!   `on_touch`.
//! - **Observe only**: the handler ALWAYS returns the event it was given.
//!   Returning `nil` would swallow the event (that is how the key / pointer
//!   sources implement `PreventDefault`); activity must never change what
//!   the app receives, so there is no branch that does.
//! - **Blind spots** (AppKit's, not ours): events consumed by a nested
//!   tracking loop (an open menu, a live slider/scroller drag, window
//!   dragging) never pass through `sendEvent:`. The press that STARTED the
//!   loop did, so every interaction still ticks once at its start.
//! - **Teardown**: the monitor token is held by [`UserActivitySource`];
//!   dropping it (the backend dropping, or a re-install replacing it) calls
//!   `+[NSEvent removeMonitor:]`.

use std::rc::Rc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::{class, msg_send};
use objc2_foundation::NSObject;

// `NSEventType` values of the events that count as "the person is using the
// app": presses, key presses, wheel/scroll, and trackpad gestures. Motion
// and releases are left out — a mouse drifting over the window, or the
// release that ends a press already counted, is not a new interaction.
const T_LEFT_DOWN: u32 = 1;
const T_RIGHT_DOWN: u32 = 3;
const T_KEY_DOWN: u32 = 10;
const T_ROTATE: u32 = 18;
const T_SCROLL_WHEEL: u32 = 22;
const T_OTHER_DOWN: u32 = 25;
const T_MAGNIFY: u32 = 30;
const T_SWIPE: u32 = 31;
const T_SMART_MAGNIFY: u32 = 32;

/// The event types the monitor watches, in `NSEventType` order.
pub(crate) const ACTIVITY_TYPES: [u32; 9] = [
    T_LEFT_DOWN,
    T_RIGHT_DOWN,
    T_KEY_DOWN,
    T_ROTATE,
    T_SCROLL_WHEEL,
    T_OTHER_DOWN,
    T_MAGNIFY,
    T_SWIPE,
    T_SMART_MAGNIFY,
];

/// `NSEventMask` (`1 << NSEventType`, an `unsigned long long`) for
/// [`ACTIVITY_TYPES`]. `u64`, not `usize`-sized-by-accident: SmartMagnify is
/// type 32, so its bit only exists in a 64-bit mask.
pub(crate) const ACTIVITY_MASK: u64 = {
    let mut mask = 0u64;
    let mut i = 0;
    while i < ACTIVITY_TYPES.len() {
        mask |= 1u64 << ACTIVITY_TYPES[i];
        i += 1;
    }
    mask
};

/// The installed monitor. Removes itself on drop.
pub(crate) struct UserActivitySource {
    monitor: Retained<NSObject>,
}

impl Drop for UserActivitySource {
    fn drop(&mut self) {
        unsafe {
            let _: () = msg_send![class!(NSEvent), removeMonitor: &*self.monitor];
        }
    }
}

/// Install the monitor. `after_report` runs after every report that ticked
/// (the backend's flush, so the per-world activity signal commits); a
/// throttled report schedules nothing. `None` only if AppKit refuses the
/// monitor (it returns `nil`), in which case activity simply never ticks.
pub(crate) fn install(after_report: Rc<dyn Fn()>) -> Option<UserActivitySource> {
    // Crash-loud per project policy (an FFI callback that unwinds aborts).
    // The event pointer goes back out untouched on every path.
    let block = RcBlock::new(move |event: *mut NSObject| -> *mut NSObject {
        backend_apple_core::crash::abort_on_panic("user activity monitor", || {
            if runtime_shared::user_activity::report() {
                after_report();
            }
        });
        event
    });
    // `addLocalMonitor…` copies the handler block, so the local `block` may
    // drop after this; the returned token is what `removeMonitor:` needs.
    let monitor: *mut NSObject = unsafe {
        msg_send![
            class!(NSEvent),
            addLocalMonitorForEventsMatchingMask: ACTIVITY_MASK,
            handler: &*block,
        ]
    };
    let monitor = unsafe { Retained::retain(monitor) }?;
    Some(UserActivitySource { monitor })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every watched type has its bit, and nothing else does — in particular
    /// no release, motion or modifier (`flagsChanged`) type, which would tick
    /// on a mouse drifting over the window or a press already counted.
    #[test]
    fn mask_is_exactly_the_press_like_types() {
        for t in ACTIVITY_TYPES {
            assert!(ACTIVITY_MASK & (1u64 << t) != 0, "type {t} in the mask");
        }
        assert_eq!(ACTIVITY_MASK.count_ones() as usize, ACTIVITY_TYPES.len());
        // LeftUp 2, RightUp 4, MouseMoved 5, LeftDragged 6, KeyUp 11,
        // FlagsChanged 12, OtherUp 26.
        for t in [2u32, 4, 5, 6, 11, 12, 26] {
            assert_eq!(ACTIVITY_MASK & (1u64 << t), 0, "type {t} must not tick");
        }
    }
}
