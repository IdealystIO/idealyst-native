//! The pure Win32 → framework rules behind the app-level pointer source.
//!
//! ## The bug this pins
//!
//! `WindowsBackend` inherited the trait's no-op `set_pointer_sink`: mouse
//! buttons, look-around and wheel were dead for a game on Windows, and
//! pointer lock never engaged. These are the translation rules the new
//! source depends on.
//!
//! The crate body is `cfg(target_os = "windows")`; this test compiles the
//! policy module directly (`#[path]`) so it runs on every host. The live
//! slot is covered by `app_pointer::tests` (Windows target / Wine).

#[path = "../src/pointer_policy.rs"]
mod pointer_policy;

use pointer_policy::*;
use runtime_shared::app_state::PointerKind;
use runtime_shared::pointer_input::MouseButton;

#[test]
fn regression_win32_app_pointer_classifies_every_mouse_message() {
    let b = |button, down| Some(MouseMsg::Button { button, down });
    assert_eq!(classify(WM_LBUTTONDOWN, 0), b(MouseButton::Primary, true));
    assert_eq!(classify(WM_LBUTTONDBLCLK, 0), b(MouseButton::Primary, true), "double-click is a press");
    assert_eq!(classify(WM_LBUTTONUP, 0), b(MouseButton::Primary, false));
    assert_eq!(classify(WM_RBUTTONDOWN, 0), b(MouseButton::Secondary, true));
    assert_eq!(classify(WM_RBUTTONUP, 0), b(MouseButton::Secondary, false));
    assert_eq!(classify(WM_MBUTTONDOWN, 0), b(MouseButton::Middle, true));
    assert_eq!(classify(WM_MBUTTONUP, 0), b(MouseButton::Middle, false));
    assert_eq!(classify(WM_XBUTTONDOWN, 1 << 16), b(MouseButton::Back, true));
    assert_eq!(classify(WM_XBUTTONUP, 2 << 16), b(MouseButton::Forward, false));
    assert_eq!(classify(WM_XBUTTONDOWN, 3 << 16), None, "unknown X button");
    assert_eq!(classify(WM_MOUSEMOVE, 0), Some(MouseMsg::Move));
    assert_eq!(classify(WM_MOUSEWHEEL, 0), Some(MouseMsg::Wheel { horizontal: false }));
    assert_eq!(classify(WM_MOUSEHWHEEL, 0), Some(MouseMsg::Wheel { horizontal: true }));
    assert_eq!(classify(0x0100 /* WM_KEYDOWN */, 0), None);
}

#[test]
fn wheel_converts_to_web_sign_and_pixels() {
    // Rotated toward the user: negative WHEEL_DELTA → content down → +dy.
    assert_eq!(wheel_px(-120, false), (0.0, WHEEL_NOTCH_PX));
    assert_eq!(wheel_px(120, false), (0.0, -WHEEL_NOTCH_PX));
    // High-resolution wheel: a fraction of a notch.
    assert_eq!(wheel_px(-30, false), (0.0, WHEEL_NOTCH_PX / 4.0));
    // Tilt right → content right → +dx (same sign as Web).
    assert_eq!(wheel_px(120, true), (WHEEL_NOTCH_PX, 0.0));
}

#[test]
fn lparam_points_are_signed() {
    assert_eq!(lparam_point(((20u32 << 16) | 10) as isize), (10, 20));
    // (-5, -1) — left of / above the window (multi-monitor, captured).
    let lp = (((-1i16 as u16 as u32) << 16) | (-5i16 as u16 as u32)) as i32 as isize;
    assert_eq!(lparam_point(lp), (-5, -1));
}

#[test]
fn touch_synthesized_mouse_is_skipped_but_pen_is_not() {
    assert!(is_touch_synthesized(0xFF51_5780), "touch");
    assert!(!is_touch_synthesized(0xFF51_5700), "pen");
    assert!(!is_touch_synthesized(0), "real mouse");
}

#[test]
fn modifiers_and_moves() {
    assert_eq!(wparam_shift_ctrl(0x0004), (true, false));
    assert_eq!(wparam_shift_ctrl(0x0008 | 0x0001), (false, true));
    assert_eq!(motion_delta(None, (5.0, 5.0)), Some((0.0, 0.0)));
    assert_eq!(motion_delta(Some((5.0, 5.0)), (5.0, 5.0)), None, "no-motion repeat");
    assert_eq!(motion_delta(Some((5.0, 5.0)), (2.0, 9.0)), Some((-3.0, 4.0)));
}

#[test]
fn raw_input_relative_and_absolute() {
    // Message / flag values the slot keys on (winuser.h, ntddmou.h).
    assert_eq!(WM_INPUT, 0x00FF);
    assert_eq!(MOUSE_VIRTUAL_DESKTOP, 0x02);
    let mut prev = None;
    // Relative mouse: counts as-is.
    assert_eq!(raw_mouse_delta(0, 3, -2, (1920, 1080), &mut prev), (3.0, -2.0));
    assert_eq!(prev, None);
    // Absolute (RDP / tablet): first report seeds, then pixel differences.
    assert_eq!(raw_mouse_delta(MOUSE_MOVE_ABSOLUTE, 32767, 32767, (1920, 1080), &mut prev), (0.0, 0.0));
    let (dx, dy) = raw_mouse_delta(MOUSE_MOVE_ABSOLUTE, 32767 + 3413, 32767, (1920, 1080), &mut prev);
    assert!((dx - 100.0).abs() < 0.1 && dy == 0.0, "{dx} {dy}");
}

#[test]
fn lock_point_is_the_client_centre() {
    assert_eq!(lock_point((100, 50, 500, 350)), (300, 200));
}

#[test]
fn situation_mappings() {
    assert_eq!(visible_from_size(1), Some(false));
    assert_eq!(visible_from_size(0), Some(true));
    assert_eq!(visible_from_size(2), Some(true));
    assert_eq!(visible_from_size(3), None);
    assert!(reduce_motion(false));
    assert!(!reduce_motion(true));
    assert_eq!(pointer_kind(true, 0), PointerKind::Fine);
    assert_eq!(pointer_kind(false, 0x80 | 0x01), PointerKind::Coarse);
    assert_eq!(pointer_kind(false, 0x01), PointerKind::None, "digitizer not ready");
    assert_eq!(pointer_kind(false, 0), PointerKind::None);
}
