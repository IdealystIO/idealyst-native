//! The Win32 source of the app-wide `user_activity()` heartbeat
//! (`runtime_shared::user_activity`): every button down, touch / pen
//! contact, wheel notch and key press of the app's thread ticks it.
//!
//! ## Why the message PUMP
//!
//! Same reason as the app-level keyboard and pointer (`app_keys.rs`,
//! `app_pointer.rs`): Win32 delivers a mouse message to the window UNDER
//! the cursor and a key message to the FOCUSED window, which is usually a
//! child control (`EDIT`, `BUTTON`, trackbar) whose window procedure the
//! backend does not own — a host `WndProc` hook would go deaf over every
//! native control. Every input message of the thread passes through the
//! host's `GetMessageW` loop, though, and the host already offers each one
//! to [`KeyboardSlot::pre_dispatch`](crate::app_keys::KeyboardSlot::pre_dispatch)
//! and [`PointerSlot::pre_dispatch`](crate::app_pointer::PointerSlot::pre_dispatch)
//! before dispatching it. Those two call [`observe_key`] / [`observe_pointer`]
//! FIRST, before looking for a sink, so:
//!
//! - activity ticks whether or not an app key / pointer listener is live
//!   (the source is unconditional), and
//! - a message a listener then swallows with `PreventDefault` still
//!   counted (observe-only: reporting never changes what happens to the
//!   message).
//!
//! Key and pointer kinds are classified separately so each message reports
//! from exactly one of the two slots — the pump offers a non-swallowed key
//! message to the pointer slot as well, and one keystroke must be one tick.
//!
//! Every window the backend creates is a `WS_CHILD` of the host window on
//! the UI thread (portals render a placeholder inside the tree; there is
//! no top-level popup class), and any window of the thread — a future
//! popup included — is pumped by the same loop, so the one seam covers
//! them all. The slots' "only this app's window tree" filter is applied
//! AFTER this hook, on purpose: any window of the app's thread is the app.
//!
//! ## Touch and pen
//!
//! A touch or pen contact arrives as `WM_POINTERDOWN` (Windows 8+), and —
//! because the backend does not handle pointer messages itself —
//! `DefWindowProc` then promotes it to a `WM_LBUTTONDOWN`. Both count; the
//! substrate's one-frame throttle collapses the pair into one tick.
//! `WM_TOUCH` is not observed: it is only sent to windows that called
//! `RegisterTouchWindow`, which nothing here does.
//!
//! Deliberately free of any `windows`-crate type, like `keymap.rs` and
//! `pointer_policy.rs`: this file is ALSO compiled by
//! `tests/user_activity.rs` via `#[path]`, which runs on any host — the
//! reachable regression test for the Win32 source (CLAUDE.md §8).

use crate::keymap;
use crate::pointer_policy::{self as policy, MouseMsg};

/// `WM_POINTERDOWN` (`winuser.h`, Windows 8+): a touch / pen / mouse
/// contact began.
pub const WM_POINTERDOWN: u32 = 0x0246;
/// `WM_POINTERWHEEL` / `WM_POINTERHWHEEL`: wheel input reported through
/// the pointer stack (precision touchpads on pointer-enabled windows).
pub const WM_POINTERWHEEL: u32 = 0x024E;
pub const WM_POINTERHWHEEL: u32 = 0x024F;

/// A key message that is user activity: every key down (`WM_KEYDOWN`,
/// and `WM_SYSKEYDOWN` — what Windows sends while Alt is held and for
/// F10), auto-repeat included. Key ups are not: they follow a down that
/// already counted.
pub fn is_key_activity(msg: u32) -> bool {
    keymap::key_message_is_down(msg) == Some(true)
}

/// A pointer message that is user activity: a button down (double-clicks
/// included — they replace the second down), a wheel notch, or a pointer
/// contact. Moves and button ups are not (hover is not "using the app",
/// and an up follows a down that already counted). `wparam` is needed only
/// to classify the X buttons.
pub fn is_pointer_activity(msg: u32, wparam: usize) -> bool {
    match policy::classify(msg, wparam) {
        Some(MouseMsg::Button { down, .. }) => down,
        Some(MouseMsg::Wheel { .. }) => true,
        Some(MouseMsg::Move) => false,
        None => matches!(msg, WM_POINTERDOWN | WM_POINTERWHEEL | WM_POINTERHWHEEL),
    }
}

/// Report activity when `activity` holds, and run `flush` (the backend's
/// `schedule_flush`) when the report ticked — a throttled report changes
/// nothing. Returns whether it ticked. The flush is a parameter so the
/// `#[path]` host test can drive this without the Win32-only crate body.
pub fn observe(activity: bool, flush: impl FnOnce()) -> bool {
    if !activity || !runtime_shared::user_activity::report() {
        return false;
    }
    flush();
    true
}

/// The keyboard slot's hook: one retrieved message, before any sink.
pub fn observe_key(msg: u32, flush: impl FnOnce()) -> bool {
    observe(is_key_activity(msg), flush)
}

/// The pointer slot's hook: one retrieved message, before any sink.
pub fn observe_pointer(msg: u32, wparam: usize, flush: impl FnOnce()) -> bool {
    observe(is_pointer_activity(msg, wparam), flush)
}
