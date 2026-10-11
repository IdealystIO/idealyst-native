//! The pure GDK → framework rules behind the app-level pointer source.
//!
//! ## The bug this pins
//!
//! `LinuxBackend` inherited the trait's no-op `set_pointer_sink`: a game's
//! mouse buttons, look-around and wheel were dead on Linux, and pointer
//! lock silently never happened. These are the translation rules the new
//! source depends on.
//!
//! The crate body is `cfg(target_os = "linux")`; this test compiles the
//! policy module directly (`#[path]`) so it runs on every host, including
//! the macOS dev machine. The live GTK wiring is covered by
//! `tests/app_pointer.rs` (Linux container, real X events via XTest).

#[path = "../src/pointer_policy.rs"]
mod pointer_policy;

use pointer_policy::*;
use runtime_shared::app_state::PointerKind;
use runtime_shared::pointer_input::MouseButton;

#[test]
fn regression_gtk_app_pointer_maps_gdk_buttons_including_side_buttons() {
    assert_eq!(gdk_button(1), Some(MouseButton::Primary));
    assert_eq!(gdk_button(2), Some(MouseButton::Middle));
    assert_eq!(gdk_button(3), Some(MouseButton::Secondary));
    assert_eq!(gdk_button(8), Some(MouseButton::Back));
    assert_eq!(gdk_button(9), Some(MouseButton::Forward));
    // Legacy wheel buttons are not buttons.
    for b in [0, 4, 5, 6, 7, 10] {
        assert_eq!(gdk_button(b), None, "button {b}");
    }
}

#[test]
fn wheel_uses_web_sign_and_converts_notches_but_not_trackpad_pixels() {
    // Wheel toward the user (GDK_SCROLL_DOWN) = content down = +dy.
    assert_eq!(scroll_px(Scroll::Down), (0.0, WHEEL_NOTCH_PX));
    assert_eq!(scroll_px(Scroll::Up), (0.0, -WHEEL_NOTCH_PX));
    assert_eq!(scroll_px(Scroll::Right), (WHEEL_NOTCH_PX, 0.0));
    assert_eq!(scroll_px(Scroll::Left), (-WHEEL_NOTCH_PX, 0.0));
    // Smooth, wheel units: 1.0 = one notch; high-res wheels send fractions.
    assert_eq!(
        scroll_px(Scroll::Smooth { dx: 0.0, dy: 0.5, surface_units: false }),
        (0.0, WHEEL_NOTCH_PX / 2.0)
    );
    // Smooth, surface units (trackpad): already logical px.
    assert_eq!(scroll_px(Scroll::Smooth { dx: -3.0, dy: 7.5, surface_units: true }), (-3.0, 7.5));
}

#[test]
fn first_motion_reports_no_jump() {
    assert_eq!(motion_delta(None, (100.0, 50.0)), (0.0, 0.0));
    assert_eq!(motion_delta(Some((100.0, 50.0)), (103.0, 46.0)), (3.0, -4.0));
}

#[test]
fn regression_x11_lock_does_not_double_count_motion_racing_the_warp() {
    // Locked at (200,150); the pointer was at (10,10) when the lock warped.
    let mut t = LockTracker::new(lock_center(400, 301), Some((10.0, 10.0)));
    // A motion queued before the acquire warp landed: measured from (10,10).
    assert_eq!(t.on_motion((12.0, 10.0)), (Some((2.0, 0.0)), true));
    // Two echoes (acquire + that recenter) — nothing to report.
    assert_eq!(t.on_motion((200.0, 150.0)), (None, false));
    assert_eq!(t.on_motion((200.0, 150.0)), (None, false));
    // Real motion: +5,+0 → report, warp back.
    assert_eq!(t.on_motion((205.0, 150.0)), (Some((5.0, 0.0)), true));
    // More motion BEFORE the server processed that warp: the pointer is at
    // 205+3. Against the centre this would be 8 (5 counted twice); it is 3.
    assert_eq!(t.on_motion((208.0, 150.0)), (Some((3.0, 0.0)), true));
    // Both echoes arrive; then motion relative to the centre again.
    assert_eq!(t.on_motion((200.0, 150.0)), (None, false));
    assert_eq!(t.on_motion((200.0, 150.0)), (None, false));
    assert_eq!(t.on_motion((199.0, 152.0)), (Some((-1.0, 2.0)), true));
}

#[test]
fn lock_started_on_the_centre_expects_no_echo() {
    let mut t = LockTracker::new((50, 50), Some((50.0, 50.0)));
    assert_eq!(t.on_motion((51.0, 50.0)), (Some((1.0, 0.0)), true));
}

#[test]
fn settings_map_to_app_state() {
    assert!(reduce_motion(false), "animations off → reduce motion");
    assert!(!reduce_motion(true));
    assert_eq!(pointer_kind(true, true), PointerKind::Fine, "touch laptop with a trackpad");
    assert_eq!(pointer_kind(false, true), PointerKind::Coarse);
    assert_eq!(pointer_kind(false, false), PointerKind::None);
}
