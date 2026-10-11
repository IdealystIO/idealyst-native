//! App-level pointer plumbing for the GPU engine (the pointer
//! counterpart of the keyboard sink): pure translation from the
//! render-API input vocabulary to `runtime_shared::pointer_input`
//! events, plus the shell hook that carries pointer-lock and
//! cursor-visibility requests out of the engine.
//!
//! # Why a shell hook
//!
//! The engine has no window: locking the cursor and hiding it are
//! windowing-system calls (`winit::Window::set_cursor_grab`,
//! `set_cursor_visible`, …) only the platform shell can make. The
//! framework's `AppEnvOps::set_pointer_lock` / `set_cursor_hidden` land
//! on `WgpuBackend`, which forwards them through the
//! [`PointerControl`] hook the shell installed with
//! [`Host::set_pointer_control_hook`](crate::Host::set_pointer_control_hook).
//! The shell answers a lock request through
//! `EventSink::pointer_lock_changed`. A shell that installs no hook
//! cannot lock: requests go nowhere and the app never sees
//! `pointer_locked() == true` — the honest "unsupported" answer.
//!
//! The hook is called while the backend is mutably borrowed (the
//! framework's dispatcher reaches `set_pointer_lock` through
//! `try_borrow_mut`), so it must NOT call back into the `Host` /
//! backend synchronously. Post the request to the shell's event loop
//! (winit: an `EventLoopProxy` user event) and answer from there.

use render_api::{KeyModifiers, PointerButton};
use runtime_shared::pointer_input::{AppPointerEvent, MouseButton, PointerPhase};

/// A request the engine forwards to the platform shell.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PointerControl {
    /// Lock (`true`: hide the cursor, hold it in place, report raw
    /// motion through `EventSink::mouse_motion`) or release (`false`).
    /// Answer with `EventSink::pointer_lock_changed`.
    Lock(bool),
    /// Hide (`true`) or show the cursor while it is over the window.
    HideCursor(bool),
}

/// The shell's handler for [`PointerControl`] requests. See the module
/// docs for the no-re-entry rule.
pub type PointerControlHook = std::rc::Rc<dyn Fn(PointerControl)>;

/// The app-level button for a render-API button. `Other(3)` / `Other(4)`
/// are the back / forward side buttons (the winit shell maps them there,
/// and they are the DOM `button` numbers); any other extra button has no
/// app-level name and is not delivered.
pub(crate) fn app_mouse_button(b: PointerButton) -> Option<MouseButton> {
    match b {
        PointerButton::Primary => Some(MouseButton::Primary),
        PointerButton::Secondary => Some(MouseButton::Secondary),
        PointerButton::Middle => Some(MouseButton::Middle),
        PointerButton::Other(3) => Some(MouseButton::Back),
        PointerButton::Other(4) => Some(MouseButton::Forward),
        PointerButton::Other(_) => None,
    }
}

/// A blank app event stamped with the current modifier state.
pub(crate) fn app_event(phase: PointerPhase, at: (f32, f32), m: KeyModifiers) -> AppPointerEvent {
    AppPointerEvent {
        shift: m.shift,
        ctrl: m.ctrl,
        alt: m.alt,
        meta: m.meta,
        ..AppPointerEvent::new(phase, at.0, at.1)
    }
}

/// `Move` from an absolute position: `dx`/`dy` are the movement since
/// the previous mouse position (0 for the first event).
pub(crate) fn move_event(
    at: (f32, f32),
    last: Option<(f32, f32)>,
    m: KeyModifiers,
) -> AppPointerEvent {
    let (dx, dy) = last.map_or((0.0, 0.0), |l| (at.0 - l.0, at.1 - l.1));
    AppPointerEvent { dx, dy, ..app_event(PointerPhase::Move, at, m) }
}

/// `Wheel` from a render-API scroll delta. `ScrollEvent::delta` is
/// already logical px in the web `deltaY` sign (positive = content
/// scrolls down), so it passes straight through.
pub(crate) fn wheel_event(at: (f32, f32), delta: (f32, f32), m: KeyModifiers) -> AppPointerEvent {
    AppPointerEvent {
        wheel_dx: delta.0,
        wheel_dy: delta.1,
        ..app_event(PointerPhase::Wheel, at, m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_map_to_app_buttons() {
        assert_eq!(app_mouse_button(PointerButton::Primary), Some(MouseButton::Primary));
        assert_eq!(app_mouse_button(PointerButton::Secondary), Some(MouseButton::Secondary));
        assert_eq!(app_mouse_button(PointerButton::Middle), Some(MouseButton::Middle));
        assert_eq!(app_mouse_button(PointerButton::Other(3)), Some(MouseButton::Back));
        assert_eq!(app_mouse_button(PointerButton::Other(4)), Some(MouseButton::Forward));
        assert_eq!(app_mouse_button(PointerButton::Other(7)), None);
    }

    #[test]
    fn move_delta_is_since_previous_position() {
        let m = KeyModifiers::default();
        let first = move_event((10.0, 20.0), None, m);
        assert_eq!((first.dx, first.dy), (0.0, 0.0));
        let next = move_event((13.0, 16.0), Some((10.0, 20.0)), m);
        assert_eq!((next.x, next.y, next.dx, next.dy), (13.0, 16.0, 3.0, -4.0));
        assert_eq!(next.phase, PointerPhase::Move);
    }

    #[test]
    fn wheel_keeps_web_sign_and_modifiers() {
        let m = KeyModifiers { shift: true, meta: true, ..Default::default() };
        let w = wheel_event((1.0, 2.0), (0.0, 48.0), m);
        assert_eq!(w.phase, PointerPhase::Wheel);
        assert_eq!((w.wheel_dx, w.wheel_dy), (0.0, 48.0));
        assert!(w.shift && w.meta && !w.ctrl && !w.alt);
    }
}
