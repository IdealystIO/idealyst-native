//! The GTK source of the app-wide `user_activity()` heartbeat
//! (`runtime_shared::user_activity`): every button press, touch begin, key
//! press, wheel / scroll and touchpad gesture on the host window ticks it.
//!
//! ## One capture-phase `EventControllerLegacy` on the host window
//!
//! GTK4 propagates an event from the toplevel down to its target
//! (capture) and back up (bubble), so a window controller in the CAPTURE
//! phase sees every event first, whichever widget is under the pointer or
//! has focus — the same seam `app_keys.rs` / `app_pointer.rs` use. One
//! legacy controller (raw `GdkEvent`s) covers presses, touches, keys and
//! scrolls at once; `GestureClick` + `EventControllerKey` +
//! `EventControllerScroll` would be three controllers for the same
//! observation, and a `GestureClick` follows one button at a time.
//!
//! **Observe only**: the handler always returns `Propagation::Proceed`, so
//! nothing it sees is claimed — every widget and every later controller
//! gets the event exactly as before.
//!
//! ## Always the NEWEST controller on the window
//!
//! GTK runs a widget's same-phase controllers newest-first, and a
//! NON-gesture controller that handles an event ends the walk: the
//! controllers added before it never see that event. The app pointer and
//! key sources (`app_pointer.rs`, `app_keys.rs`) are such controllers —
//! they return `Stop` for a press or key a listener claims with
//! `PreventDefault` — and they come and go at runtime as listeners are
//! added and removed. So installing this once at boot is not enough: every
//! time `LinuxBackend` installs one of those sources it calls
//! [`UserActivitySource::raise`], which re-adds this controller so it is
//! again the newest and runs before them. Without that, a claimed press —
//! a game's click, a canvas's wheel — would be invisible to the idle
//! clock. `tests/user_activity.rs` pins this against a real X server
//! (measured: an observer added before the pointer source saw 0 of the
//! claimed presses, one added after it saw all of them).
//!
//! Installed at boot (`newcore::start_with`), unconditionally — the source
//! has no side effects, so unlike the key source it is always on.
//!
//! ## Windows covered
//!
//! The host window is the backend's only toplevel: portals (modals,
//! sheets, popovers, tooltips) are `gtk::Overlay` children of that same
//! window (`portal.rs`), so their input propagates through its capture
//! phase as well. There is no second window to observe.
//!
//! Every tick runs [`crate::newcore::schedule_flush`] so world signals
//! commit; a throttled report schedules nothing.

use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;

/// Whether a raw GDK event is user activity. Presses, touch begins, key
/// presses, wheel / smooth scroll and touchpad swipe / pinch (the
/// touchpad's scroll-and-zoom gestures). Not: motion and crossing (hover
/// is not "using the app"), releases and touch updates / ends (they follow
/// a begin that already counted), focus and proximity changes.
pub fn is_activity(kind: gdk::EventType) -> bool {
    matches!(
        kind,
        gdk::EventType::ButtonPress
            | gdk::EventType::TouchBegin
            | gdk::EventType::KeyPress
            | gdk::EventType::Scroll
            | gdk::EventType::TouchpadSwipe
            | gdk::EventType::TouchpadPinch
            | gdk::EventType::PadButtonPress
    )
}

/// The installed observer, kept so [`UserActivitySource::uninstall`] can
/// remove it.
pub struct UserActivitySource {
    window: glib::WeakRef<gtk4::Window>,
    controller: gtk4::EventControllerLegacy,
}

impl UserActivitySource {
    /// Attach the capture-phase observer to `window`.
    pub fn install(window: &gtk4::Window) -> Self {
        let controller = gtk4::EventControllerLegacy::new();
        controller.set_propagation_phase(gtk4::PropagationPhase::Capture);
        controller.connect_event(|_, ev| {
            if is_activity(ev.event_type()) && runtime_shared::user_activity::report() {
                crate::newcore::schedule_flush();
            }
            glib::Propagation::Proceed
        });
        window.add_controller(controller.clone());
        UserActivitySource { window: window.downgrade(), controller }
    }

    /// Re-add the controller so it is the newest on the window and runs
    /// before every capture controller installed so far (see the module
    /// doc). Safe during event dispatch: GTK tolerates controllers being
    /// removed / added mid-walk, which the app sources already rely on
    /// when a listener is added from inside an event.
    pub fn raise(&self) {
        if let Some(window) = self.window.upgrade() {
            window.remove_controller(&self.controller);
            window.add_controller(self.controller.clone());
        }
    }

    /// Remove the observer (no-op if the window is already gone).
    pub fn uninstall(self) {
        if let Some(window) = self.window.upgrade() {
            window.remove_controller(&self.controller);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The classification needs no display: pin which raw events are
    /// activity. (The live controller is covered by
    /// `tests/user_activity.rs` under Xvfb.)
    #[test]
    fn activity_is_presses_keys_and_scrolls_not_motion_or_releases() {
        use gdk::EventType as T;
        for t in [T::ButtonPress, T::TouchBegin, T::KeyPress, T::Scroll, T::TouchpadSwipe, T::TouchpadPinch] {
            assert!(is_activity(t), "{t:?} is activity");
        }
        for t in [
            T::MotionNotify,
            T::ButtonRelease,
            T::KeyRelease,
            T::TouchUpdate,
            T::TouchEnd,
            T::TouchCancel,
            T::EnterNotify,
            T::LeaveNotify,
            T::FocusChange,
        ] {
            assert!(!is_activity(t), "{t:?} is not activity");
        }
    }
}
