//! The Win32 `user_activity()` source: what the pump reports.
//!
//! ## The bug this pins
//!
//! The Win32 backend fed nothing into `runtime_shared::user_activity`, so
//! an idle clock built on `user_activity()` (kiosk "still there?", session
//! timeout, auto-lock) expired while the person was clicking, typing and
//! scrolling — including over native child controls, whose messages never
//! reach the host `WndProc`.
//!
//! The crate body is `cfg(target_os = "windows")`; this test compiles the
//! source module (and the two pure tables it classifies with) directly
//! (`#[path]`) so it runs on every host. It drives the exact functions
//! `KeyboardSlot::pre_dispatch` / `PointerSlot::pre_dispatch` call on each
//! pumped message.
//!
//! Each check starts from `reset_for_tests()`; no time source is installed
//! in this binary, so the substrate's throttle is off and every report
//! ticks.

#[path = "../src/keymap.rs"]
#[allow(dead_code)]
mod keymap;
#[path = "../src/pointer_policy.rs"]
#[allow(dead_code)]
mod pointer_policy;
#[path = "../src/user_activity.rs"]
mod user_activity;

use std::cell::Cell;

use keymap::{WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP};
use pointer_policy::*;
use runtime_shared::user_activity as activity;
use user_activity::*;

/// `HIWORD(wParam)` for `XBUTTON1` (back).
const XBUTTON1_WPARAM: usize = 1 << 16;

/// Offer one message to the slot hook it would reach first; return
/// `(ticks, flushes scheduled)`.
fn offer(key: bool, msg: u32, wparam: usize) -> (u64, u32) {
    activity::reset_for_tests();
    let flushes = Cell::new(0);
    let flush = || flushes.set(flushes.get() + 1);
    if key {
        observe_key(msg, flush);
    } else {
        observe_pointer(msg, wparam, flush);
    }
    (activity::count(), flushes.get())
}

#[test]
fn regression_win32_input_never_reported_user_activity() {
    for (msg, wparam, what) in [
        (WM_LBUTTONDOWN, 0, "left down"),
        (WM_LBUTTONDBLCLK, 0, "double-click"),
        (WM_RBUTTONDOWN, 0, "right down"),
        (WM_MBUTTONDOWN, 0, "middle down"),
        (WM_XBUTTONDOWN, XBUTTON1_WPARAM, "side button down"),
        (WM_MOUSEWHEEL, 0, "wheel"),
        (WM_MOUSEHWHEEL, 0, "horizontal wheel"),
        (WM_POINTERDOWN, 0, "touch / pen contact"),
        (WM_POINTERWHEEL, 0, "pointer wheel"),
    ] {
        assert_eq!(offer(false, msg, wparam), (1, 1), "{what} ticks and schedules a flush");
    }
    for msg in [WM_KEYDOWN, WM_SYSKEYDOWN] {
        assert_eq!(offer(true, msg, 0), (1, 1), "key down {msg:#x}");
    }

    // Not activity: hover, releases, key ups, raw input.
    for msg in [WM_MOUSEMOVE, WM_LBUTTONUP, WM_RBUTTONUP, WM_XBUTTONUP, WM_INPUT] {
        assert_eq!(offer(false, msg, XBUTTON1_WPARAM), (0, 0), "pointer {msg:#x}");
    }
    for msg in [WM_KEYUP, WM_SYSKEYUP] {
        assert_eq!(offer(true, msg, 0), (0, 0), "key up {msg:#x}");
    }
}

/// The pump offers a non-swallowed key message to BOTH slots (keyboard,
/// then pointer), and a mouse message to both as well. Each message must
/// tick from exactly one of them, or one keystroke / click would be two
/// ticks.
#[test]
fn win32_each_message_reports_from_exactly_one_slot() {
    for (msg, wparam) in [
        (WM_KEYDOWN, 0),
        (WM_SYSKEYDOWN, 0),
        (WM_LBUTTONDOWN, 0),
        (WM_XBUTTONDOWN, XBUTTON1_WPARAM),
        (WM_MOUSEWHEEL, 0),
        (WM_POINTERDOWN, 0),
    ] {
        activity::reset_for_tests();
        observe_key(msg, || {});
        observe_pointer(msg, wparam, || {});
        assert_eq!(activity::count(), 1, "message {msg:#x} ticks once across both slots");
    }
}
