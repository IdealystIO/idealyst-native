//! Host facts about the app's situation that app code reacts to: whether
//! the app has focus, whether it is visible, the user's reduce-motion
//! preference, what kind of pointer the device primarily has, and whether
//! the user has interacted yet (browsers only allow sound after that).
//!
//! ## Flow
//!
//! Backends observe the platform and call [`report`] with each change
//! (and once at boot with what they can read up front), then schedule
//! their usual post-event flush. The author surface lives per world in
//! `runtime_vocabulary::app_state` (`app_focused()`, `app_visible()`,
//! `reduce_motion()`, `primary_pointer()`, `user_activated()`): each
//! world's context (also `pointer_locked()`) seeds from [`current`] and
//! subscribes through
//! [`observe`].
//!
//! The thread-local value here is the single source of truth so a world
//! created after a report (a second world, an SSR request world) still
//! starts from the right value.
//!
//! ## Defaults
//!
//! [`AppState::default`] describes a desktop app in the foreground that
//! the user is already using: focused, visible, no reduced motion, fine
//! pointer, activated. A backend reports only what differs on its
//! platform (web reports `user_activated: false` at boot; phones report a
//! coarse pointer), so a backend that reports nothing still gives app code
//! sane answers.

use std::cell::RefCell;
use std::rc::Rc;

/// What kind of pointing device the user primarily has — the CSS
/// `(pointer: …)` media feature, on every platform.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "remote-serde", derive(serde::Serialize, serde::Deserialize))]
pub enum PointerKind {
    /// An accurate pointer: mouse, trackpad, stylus.
    #[default]
    Fine,
    /// An imprecise pointer: a finger on a touch screen. A "touch-only
    /// device" is one whose primary pointer is `Coarse`.
    Coarse,
    /// No pointing device at all (TV remote, keyboard-only).
    None,
}

/// A snapshot of the host facts. See the module docs for defaults.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "remote-serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AppState {
    /// The app's window has input focus (it is the active window / tab).
    pub focused: bool,
    /// The app is on screen (not a background tab, not minimized, not
    /// backgrounded on mobile).
    pub visible: bool,
    /// The user asked the OS for less motion.
    pub reduce_motion: bool,
    /// The device's primary pointer.
    pub primary_pointer: PointerKind,
    /// The user has interacted with the app (a press, click, or key) —
    /// on web this is when the browser starts allowing audio. Never goes
    /// back to `false`.
    pub user_activated: bool,
    /// The pointer is locked (see [`crate::pointer_input`]). Reported by
    /// the pointer dispatcher, not by backends directly.
    pub pointer_locked: bool,
}

impl Default for AppState {
    fn default() -> Self {
        AppState {
            focused: true,
            visible: true,
            reduce_motion: false,
            primary_pointer: PointerKind::Fine,
            user_activated: true,
            pointer_locked: false,
        }
    }
}

/// One change a backend reports.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AppStateChange {
    Focused(bool),
    Visible(bool),
    ReduceMotion(bool),
    PrimaryPointer(PointerKind),
    /// `false` only makes sense at boot (web, before any gesture); after
    /// the first `true` a later `false` is ignored — activation is sticky.
    UserActivated(bool),
    /// Sent by [`crate::pointer_input`] when the lock is granted or lost.
    PointerLocked(bool),
}

type Observer = Rc<dyn Fn(&AppState)>;

thread_local! {
    static STATE: RefCell<AppState> = RefCell::new(AppState::default());
    static OBSERVERS: RefCell<Vec<Observer>> = const { RefCell::new(Vec::new()) };
    /// Whether activation has been observed as `true` after a boot-time
    /// `false` — makes [`AppStateChange::UserActivated`] sticky.
    static ACTIVATED_ONCE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The current host facts.
pub fn current() -> AppState {
    STATE.with(|s| *s.borrow())
}

/// Apply one change from a backend and notify observers if anything
/// actually changed. Call from the platform callback; then schedule the
/// backend's flush so world signals commit.
pub fn report(change: AppStateChange) {
    let changed = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let before = *s;
        match change {
            AppStateChange::Focused(v) => s.focused = v,
            AppStateChange::Visible(v) => s.visible = v,
            AppStateChange::ReduceMotion(v) => s.reduce_motion = v,
            AppStateChange::PrimaryPointer(v) => s.primary_pointer = v,
            AppStateChange::PointerLocked(v) => s.pointer_locked = v,
            AppStateChange::UserActivated(v) => {
                if v {
                    ACTIVATED_ONCE.with(|a| a.set(true));
                    s.user_activated = true;
                } else if !ACTIVATED_ONCE.with(|a| a.get()) {
                    s.user_activated = false;
                }
            }
        }
        (*s != before).then_some(*s)
    });
    let Some(state) = changed else { return };
    // Snapshot outside the borrow: an observer may register another.
    let observers: Vec<Observer> = OBSERVERS.with(|o| o.borrow().clone());
    for obs in observers {
        obs(&state);
    }
}

/// Subscribe to changes (called by the per-world context). Observers live
/// for the thread; a world's observer writes through signal handles,
/// which are inert once that world is gone.
pub fn observe(f: impl Fn(&AppState) + 'static) {
    OBSERVERS.with(|o| o.borrow_mut().push(Rc::new(f)));
}

/// Reset to defaults and drop observers. Test isolation only.
#[doc(hidden)]
pub fn reset_for_tests() {
    STATE.with(|s| *s.borrow_mut() = AppState::default());
    OBSERVERS.with(|o| o.borrow_mut().clear());
    ACTIVATED_ONCE.with(|a| a.set(false));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn report_updates_and_notifies_only_on_change() {
        reset_for_tests();
        let calls = Rc::new(Cell::new(0));
        let c = calls.clone();
        observe(move |_| c.set(c.get() + 1));
        report(AppStateChange::Focused(true)); // already true
        assert_eq!(calls.get(), 0);
        report(AppStateChange::Focused(false));
        assert!(!current().focused);
        assert_eq!(calls.get(), 1);
        report(AppStateChange::PrimaryPointer(PointerKind::Coarse));
        assert_eq!(current().primary_pointer, PointerKind::Coarse);
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn user_activation_is_sticky() {
        reset_for_tests();
        report(AppStateChange::UserActivated(false)); // web boot
        assert!(!current().user_activated);
        report(AppStateChange::UserActivated(true)); // first gesture
        assert!(current().user_activated);
        report(AppStateChange::UserActivated(false));
        assert!(current().user_activated, "activation never reverts");
    }
}
