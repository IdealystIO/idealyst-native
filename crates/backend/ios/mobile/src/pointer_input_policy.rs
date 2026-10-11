//! Pure halves of the iOS app-level pointer source (`imp/pointer.rs`) and
//! app-situation reporting (`imp/app_state.rs`): GameController button /
//! motion / scroll translation, the UIKit scroll-pan → wheel conversion,
//! the pointer-lock state machine, and the `UIApplicationState` /
//! connected-mouse readings. Un-gated so the tests run from any host; the
//! UIKit / GameController code that feeds these is ios-only.
//!
//! ## Conventions this pins
//!
//! - **GameController is y-up.** Every GameController axis (thumbsticks,
//!   d-pads, and `GCMouseInput`'s moved handler and `scroll` d-pad) reports
//!   +y for "up / away from the user". The framework's pointer space is
//!   top-left origin, y down, so motion `dy` is negated ([`gc_motion`]).
//! - **Wheel sign** follows web `WheelEvent.deltaY`: positive = content
//!   scrolls down (wheel toward the user). A UIKit scroll pan's translation
//!   is the *content* drag (+y = content follows the fingers down = view
//!   scrolls toward its top), so the wheel delta is the negated translation
//!   step ([`pan_wheel_step`]) — in points, after the user's
//!   natural-scrolling preference, exactly what Safari on iPadOS reports.
//! - **Locked wheel**: while the pointer is locked UIKit delivers no
//!   pointer scroll, so the wheel comes from `GCMouseInput.scroll`
//!   ([`gc_scroll_px`]): +y away from the user, raw device units treated as
//!   lines × [`GC_WHEEL_PX_PER_LINE`]. Unverified on hardware (no iPad
//!   harness here) — see the crate README.

use runtime_shared::app_state::PointerKind;
use runtime_shared::pointer_input::MouseButton;

/// Pixels per `GCMouseInput.scroll` unit while locked: one wheel line, the
/// same 40 px the macOS backend uses for a notched wheel (Chromium's
/// per-tick pixels on Apple platforms), so a locked wheel notch scrolls the
/// same distance on every Apple backend.
pub(crate) const GC_WHEEL_PX_PER_LINE: f32 = runtime_shared::pointer_input::WHEEL_NOTCH_PX;

/// Which `GCMouseInput` button a handler is attached to.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum GcButton {
    Left,
    Right,
    Middle,
    /// `auxiliaryButtons[i]`: the side buttons, back first (HID button 4,
    /// web `button == 3`) then forward.
    Aux(usize),
}

/// The framework button for a GameController mouse button. Auxiliary
/// buttons past the two side buttons have no web equivalent.
pub(crate) fn button_for(b: GcButton) -> Option<MouseButton> {
    match b {
        GcButton::Left => Some(MouseButton::Primary),
        GcButton::Right => Some(MouseButton::Secondary),
        GcButton::Middle => Some(MouseButton::Middle),
        GcButton::Aux(0) => Some(MouseButton::Back),
        GcButton::Aux(1) => Some(MouseButton::Forward),
        GcButton::Aux(_) => None,
    }
}

/// `mouseMovedHandler` deltas (y-up) → framework `dx`/`dy` (y-down).
pub(crate) fn gc_motion(delta_x: f32, delta_y: f32) -> (f32, f32) {
    (delta_x, -delta_y)
}

/// `GCMouseInput.scroll` value (y-up, lines) → web-convention wheel px.
pub(crate) fn gc_scroll_px(x: f32, y: f32) -> (f32, f32) {
    (x * GC_WHEEL_PX_PER_LINE, -y * GC_WHEEL_PX_PER_LINE)
}

/// One scroll-pan step → web-convention wheel px. `prev` is the
/// translation at the previous callback (zero at `began`).
pub(crate) fn pan_wheel_step(prev: (f64, f64), now: (f64, f64)) -> (f32, f32) {
    (-(now.0 - prev.0) as f32, -(now.1 - prev.1) as f32)
}

/// `(shift, ctrl, alt, meta)` from the coalesced hardware keyboard, given a
/// "is this HID usage pressed" probe — GameController pointer input
/// carries no modifier flags of its own.
pub(crate) fn modifiers(pressed: impl Fn(isize) -> bool) -> (bool, bool, bool, bool) {
    // UIKeyboardHIDUsage / GCKeyCode: left ctrl/shift/alt/gui 0xE0..0xE3,
    // right 0xE4..0xE7.
    let either = |l: isize| pressed(l) || pressed(l + 4);
    (either(0xE1), either(0xE0), either(0xE2), either(0xE3))
}

/// The device's primary pointer: a connected mouse / trackpad makes it
/// `Fine`; otherwise touch (`Coarse`).
pub(crate) fn primary_pointer(connected_mice: usize) -> PointerKind {
    if connected_mice > 0 {
        PointerKind::Fine
    } else {
        PointerKind::Coarse
    }
}

/// `UIApplicationState` → `(focused, visible)`. Active = 0, inactive = 1
/// (app switcher, Control Center, an incoming call), background = 2.
pub(crate) fn focus_and_visibility(application_state: isize) -> (bool, bool) {
    (application_state == 0, application_state != 2)
}

/// What [`LockState::request`] tells the caller to do.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum LockRequest {
    /// Report this to the sink now (refusal, or already locked).
    Report(bool),
    /// Set the host view controller's lock preference and wait for
    /// `UIPointerLockStateDidChangeNotification`.
    AskPlatform,
}

/// What a platform lock-state change means.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub(crate) struct LockChange {
    /// Report to the sink.
    pub report: Option<bool>,
    /// Clear the view controller's preference: the platform dropped a lock
    /// the app held (app switcher, Split View, pointer disconnected), and
    /// UIKit would otherwise re-lock on its own when it can — behind the
    /// user's back. The app decides whether to ask again.
    pub clear_preference: bool,
}

/// Pointer-lock bookkeeping. iPadOS locks asynchronously: the app states a
/// preference (`prefersPointerLocked`) and the scene's `pointerLockState`
/// reports what the system granted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LockState {
    /// The preference is set on the view controller.
    pub wanted: bool,
    /// Last state reported to the sink.
    pub locked: bool,
}

impl LockState {
    /// A lock request. `can_lock` = the host view controller takes the
    /// preference AND the scene has a `pointerLockState` (an iPhone scene
    /// has none — it never locks).
    pub(crate) fn request(&mut self, can_lock: bool, locked_now: bool) -> LockRequest {
        if !can_lock {
            return LockRequest::Report(false);
        }
        if locked_now {
            self.wanted = true;
            self.locked = true;
            return LockRequest::Report(true);
        }
        self.wanted = true;
        LockRequest::AskPlatform
    }

    /// A release request. Returns whether to clear the preference. The
    /// `false` report arrives with the platform's change notification.
    pub(crate) fn release(&mut self) -> bool {
        std::mem::replace(&mut self.wanted, false)
    }

    /// The scene's lock state changed (or was re-read).
    pub(crate) fn platform_changed(&mut self, locked_now: bool) -> LockChange {
        let mut change = LockChange::default();
        if locked_now != self.locked {
            let was_locked = self.locked;
            self.locked = locked_now;
            change.report = Some(locked_now);
            if was_locked && !locked_now && self.wanted {
                self.wanted = false;
                change.clear_preference = true;
            }
        }
        change
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gc_buttons_map_like_web_buttons() {
        assert_eq!(button_for(GcButton::Left), Some(MouseButton::Primary));
        assert_eq!(button_for(GcButton::Right), Some(MouseButton::Secondary));
        assert_eq!(button_for(GcButton::Middle), Some(MouseButton::Middle));
        assert_eq!(button_for(GcButton::Aux(0)), Some(MouseButton::Back));
        assert_eq!(button_for(GcButton::Aux(1)), Some(MouseButton::Forward));
        assert_eq!(button_for(GcButton::Aux(2)), None);
    }

    /// REGRESSION (inverted mouse-look): GameController reports moving the
    /// mouse away from the user as +y; the framework's `dy` grows downward.
    #[test]
    fn regression_gc_motion_is_y_down() {
        assert_eq!(gc_motion(3.0, 4.0), (3.0, -4.0));
    }

    /// REGRESSION (web parity): wheel toward the user = positive `wheel_dy`.
    #[test]
    fn regression_wheel_toward_user_is_positive_dy() {
        // GC scroll: toward the user is -y.
        assert_eq!(gc_scroll_px(0.0, -1.0), (0.0, GC_WHEEL_PX_PER_LINE));
        // Scroll pan: content dragged UP (translation y falls) = the view
        // scrolls down = wheel toward the user.
        assert_eq!(pan_wheel_step((0.0, 0.0), (0.0, -12.0)), (-0.0, 12.0));
        assert_eq!(pan_wheel_step((5.0, -12.0), (2.0, -20.0)), (3.0, 8.0));
    }

    #[test]
    fn modifiers_read_either_side() {
        let (s, c, a, m) = modifiers(|u| u == 0xE5 /* right shift */ || u == 0xE3 /* left gui */);
        assert!(s && !c && !a && m);
        assert_eq!(modifiers(|_| false), (false, false, false, false));
    }

    #[test]
    fn primary_pointer_follows_connected_mice() {
        assert_eq!(primary_pointer(0), PointerKind::Coarse);
        assert_eq!(primary_pointer(1), PointerKind::Fine);
    }

    #[test]
    fn application_state_maps_to_focus_and_visibility() {
        assert_eq!(focus_and_visibility(0), (true, true));
        assert_eq!(focus_and_visibility(1), (false, true));
        assert_eq!(focus_and_visibility(2), (false, false));
    }

    /// An iPhone scene (no `pointerLockState`) or a host view controller
    /// without the lock hook refuses honestly — never a fake `true`.
    #[test]
    fn lock_refused_without_platform_support() {
        let mut s = LockState::default();
        assert_eq!(s.request(false, false), LockRequest::Report(false));
        assert!(!s.wanted);
    }

    #[test]
    fn lock_granted_asynchronously_then_released() {
        let mut s = LockState::default();
        assert_eq!(s.request(true, false), LockRequest::AskPlatform);
        assert_eq!(s.platform_changed(true), LockChange { report: Some(true), clear_preference: false });
        assert!(s.release(), "release clears the preference");
        assert_eq!(s.platform_changed(false), LockChange { report: Some(false), clear_preference: false });
        // Re-reads with no change report nothing.
        assert_eq!(s.platform_changed(false), LockChange::default());
    }

    /// REGRESSION (re-lock behind the user's back): the system dropped the
    /// lock (app switcher); the preference must be cleared or UIKit locks
    /// again on its own when the app returns.
    #[test]
    fn regression_platform_drop_clears_preference() {
        let mut s = LockState::default();
        s.request(true, false);
        s.platform_changed(true);
        let change = s.platform_changed(false);
        assert_eq!(change, LockChange { report: Some(false), clear_preference: true });
        assert!(!s.wanted);
    }

    #[test]
    fn request_while_already_locked_reports_true() {
        let mut s = LockState::default();
        assert_eq!(s.request(true, true), LockRequest::Report(true));
        assert!(s.locked && s.wanted);
    }
}
