//! App-level pointer source: mouse / trackpad / pen buttons, movement and
//! wheel for the whole window, plus pointer lock and cursor hiding,
//! delivered into the framework's [`PointerSink`]
//! (`AppEnvOps::set_pointer_sink` / `set_pointer_lock` / `set_cursor_hidden`).
//!
//! ## One capture-phase `EventControllerLegacy` on the host window
//!
//! Same reason as the keyboard (`app_keys.rs`): GTK4 propagates an event
//! from the toplevel down to its target (capture) and back up (bubble); a
//! window controller in the CAPTURE phase sees every pointer event first,
//! whichever widget is under the pointer, and returning
//! `Propagation::Stop` keeps it from that widget — the Web
//! `preventDefault` shape.
//!
//! Why the legacy controller rather than `GestureClick` + `EventControllerMotion`
//! + `EventControllerScroll`: `GtkGestureClick` is a *click* recognizer —
//! once the pointer drags past the double-click distance it emits
//! `stopped` and never reports that press's `released`, and as a
//! `GtkGestureSingle` it follows only one button at a time. A game that
//! holds the left button while aiming would get a stuck button and miss a
//! right-click made while the left is held. The legacy controller hands us
//! each raw `GdkEvent` (press, release, motion, scroll, crossing) with its
//! own return value, which is exactly the app-wide button/move/wheel stream
//! the contract asks for.
//!
//! ## PreventDefault
//!
//! - Press: `Stop` — no child widget sees the press, so no focus change,
//!   no text selection, and on a secondary press no context menu (GTK's
//!   entry / text-view menus open from their own press gestures). The
//!   matching release is stopped too, so a widget never sees half a click.
//! - Wheel: `Stop` — no `GtkScrolledWindow` scrolls.
//! - Motion cannot be prevented (nothing to suppress); while LOCKED it is
//!   stopped so widgets under the hidden, parked cursor see no hover churn.
//!
//! ## Pointer lock
//!
//! X11 only — `x11_lock.rs` explains why Wayland cannot be supported with
//! GTK4 and how the X11 lock works. A refused or unsupported request
//! reports `lock_changed(false)`, never a fake `true`. The lock is dropped
//! (and reported) when the window stops being active, before
//! `focus_lost`. Lock results are reported from a GLib idle, never inline:
//! the request arrives through the dispatcher while it holds a mutable
//! borrow of this backend.
//!
//! ## Cursor hiding
//!
//! The window's cursor becomes `"none"` (the previous cursor is restored
//! after), which hides it over the window and over every widget that
//! inherits its cursor. A widget with its OWN cursor (a text field's
//! I-beam, an idea-ui button's hand) would still win GTK's lookup, so on
//! every pointer event the source also re-sets the surface's device cursor
//! to `"none"` — the capture controller runs after GTK's per-event cursor
//! update, so the last write is ours.
//!
//! ## Re-entrancy
//!
//! The closures own an `Rc` of the shared state and a clone of the sink,
//! never the backend; the state borrow is always released before the sink
//! runs (a listener may add/remove listeners, which re-enters
//! `set_pointer_sink` on this backend).

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;

use runtime_shared::pointer_input::{AppPointerEvent, PointerOutcome, PointerPhase, PointerSink};

use crate::pointer_policy::{self, LockTracker, Scroll};
use crate::x11_lock::X11Lock;

struct Shared {
    window: glib::WeakRef<gtk4::Window>,
    sink: PointerSink,
    state: RefCell<State>,
}

#[derive(Default)]
struct State {
    /// Last pointer position in app-root coordinates (for `dx`/`dy` and
    /// the position of wheel events).
    last: Option<(f32, f32)>,
    /// Last pointer position in surface coordinates (where a lock puts the
    /// cursor back on release).
    last_surface: Option<(f64, f64)>,
    /// Buttons whose press was prevented: their release is stopped too.
    prevented: u8,
    /// `set_cursor_hidden(true)` is in effect.
    hidden: bool,
    /// The window cursor before hiding (`Some(prev)` while hidden).
    saved_cursor: Option<Option<gdk::Cursor>>,
    lock: Option<(X11Lock, LockTracker)>,
}

/// The installed native source, kept so `set_pointer_sink(None)` (or a
/// replacement) can tear it down.
pub(crate) struct AppPointerSource {
    controller: gtk4::EventControllerLegacy,
    active_handler: glib::SignalHandlerId,
    shared: Rc<Shared>,
}

impl AppPointerSource {
    /// Attach the capture-phase controller and the focus-loss watcher to
    /// `window`. `hidden` carries a cursor-hidden request that arrived
    /// before the source existed.
    pub(crate) fn install(window: &gtk4::Window, sink: PointerSink, hidden: bool) -> Self {
        let shared = Rc::new(Shared {
            window: window.downgrade(),
            sink,
            state: RefCell::new(State::default()),
        });

        let controller = gtk4::EventControllerLegacy::new();
        controller.set_propagation_phase(gtk4::PropagationPhase::Capture);
        let ev_shared = shared.clone();
        controller.connect_event(move |_, ev| handle_event(&ev_shared, ev));
        window.add_controller(controller.clone());

        let active_shared = shared.clone();
        let active_handler = window.connect_is_active_notify(move |w| {
            if !w.is_active() {
                // Drop the lock first (the user alt-tabbed away: the
                // pointer must be free), then release held buttons.
                if release_lock(&active_shared, false) {
                    active_shared.sink.lock_changed(false);
                }
                active_shared.state.borrow_mut().prevented = 0;
                active_shared.sink.focus_lost();
            }
        });

        let source = AppPointerSource { controller, active_handler, shared };
        if hidden {
            source.set_cursor_hidden(true);
        }
        source
    }

    /// Detach every native hook, release a held lock and restore the
    /// cursor. After this the sink is unreachable from GTK.
    pub(crate) fn remove(self, window: &gtk4::Window) {
        release_lock(&self.shared, true);
        self.shared.state.borrow_mut().hidden = false;
        apply_cursor(&self.shared, window);
        window.remove_controller(&self.controller);
        window.disconnect(self.active_handler);
    }

    /// Whether the cursor is hidden right now (tests / diagnostics).
    pub(crate) fn cursor_hidden(&self) -> bool {
        self.shared.state.borrow().hidden
    }

    /// Hide / show the cursor over the window.
    pub(crate) fn set_cursor_hidden(&self, hidden: bool) {
        let Some(window) = self.shared.window.upgrade() else { return };
        self.shared.state.borrow_mut().hidden = hidden;
        apply_cursor(&self.shared, &window);
    }

    /// Request (`true`) or release the lock. The result is reported from a
    /// GLib idle — see the module docs.
    pub(crate) fn set_pointer_lock(&self, locked: bool) {
        let Some(window) = self.shared.window.upgrade() else { return };
        let report = if locked {
            if self.shared.state.borrow().lock.is_some() {
                return; // already locked; already reported
            }
            // Like the browser: only the active window may take the pointer.
            let restore = self.shared.state.borrow().last_surface;
            let lock = if window.is_active() { X11Lock::acquire(&window, restore) } else { None };
            match lock {
                Some(lock) => {
                    let tracker = LockTracker::new(lock.center(), restore);
                    self.shared.state.borrow_mut().lock = Some((lock, tracker));
                    apply_cursor(&self.shared, &window);
                    true
                }
                // Refused / unsupported (Wayland): say so, never fake it.
                None => false,
            }
        } else {
            if !release_lock(&self.shared, true) {
                return;
            }
            false
        };
        let sink = self.shared.sink.clone();
        glib::idle_add_local_once(move || sink.lock_changed(report));
    }
}

/// Drop the lock if held (restoring the cursor position when
/// `restore_cursor`). Returns whether a lock was held.
fn release_lock(shared: &Rc<Shared>, restore_cursor: bool) -> bool {
    let Some((lock, _)) = shared.state.borrow_mut().lock.take() else {
        return false;
    };
    lock.release(restore_cursor);
    if let Some(window) = shared.window.upgrade() {
        apply_cursor(shared, &window);
    }
    true
}

/// Bring the window cursor in line with `hidden || locked`.
fn apply_cursor(shared: &Shared, window: &gtk4::Window) {
    let mut st = shared.state.borrow_mut();
    let want = st.hidden || st.lock.is_some();
    if want {
        if st.saved_cursor.is_none() {
            st.saved_cursor = Some(window.cursor());
            window.set_cursor_from_name(Some("none"));
        }
        drop(st);
        reassert_hidden(window);
    } else if let Some(prev) = st.saved_cursor.take() {
        drop(st);
        // Re-setting the window cursor makes GTK recompute the surface
        // cursor from the widget under the pointer, undoing
        // `reassert_hidden`'s device-cursor override too.
        window.set_cursor(prev.as_ref());
    }
}

/// Override the surface's device cursor with `"none"` — beats a child
/// widget's own cursor until GTK's next per-event update (module docs).
fn reassert_hidden(window: &gtk4::Window) {
    let Some(surface) = window.surface() else { return };
    let display = WidgetExt::display(window);
    let Some(pointer) = display.default_seat().and_then(|s| s.pointer()) else { return };
    if let Some(none) = gdk::Cursor::from_name("none", None) {
        surface.set_device_cursor(&pointer, &none);
    }
}

/// Surface coordinates → app-root coordinates (logical px).
fn to_root(window: &gtk4::Window, sx: f64, sy: f64) -> (f32, f32) {
    // Event positions are relative to the SURFACE, which on a client-side
    // decorated window includes the shadow margin; the window widget sits
    // at `surface_transform` inside it, and the app root is its child.
    let (tx, ty) = window.surface_transform();
    let (wx, wy) = ((sx - tx) as f32, (sy - ty) as f32);
    match window.child() {
        Some(root) => window
            .compute_point(&root, &gtk4::graphene::Point::new(wx, wy))
            .map(|p| (p.x(), p.y()))
            .unwrap_or((wx, wy)),
        None => (wx, wy),
    }
}

fn with_modifiers(mut e: AppPointerEvent, state: gdk::ModifierType) -> AppPointerEvent {
    use gdk::ModifierType as M;
    e.shift = state.contains(M::SHIFT_MASK);
    e.ctrl = state.contains(M::CONTROL_MASK);
    e.alt = state.contains(M::ALT_MASK);
    e.meta = state.intersects(M::SUPER_MASK | M::META_MASK);
    e
}

fn propagation(outcome: PointerOutcome) -> glib::Propagation {
    match outcome {
        PointerOutcome::PreventDefault => glib::Propagation::Stop,
        PointerOutcome::Default => glib::Propagation::Proceed,
    }
}

/// The legacy controller's `event` handler.
fn handle_event(shared: &Rc<Shared>, ev: &gdk::Event) -> glib::Propagation {
    use gdk::EventType as T;
    let ty = ev.event_type();
    if !matches!(
        ty,
        T::MotionNotify | T::ButtonPress | T::ButtonRelease | T::Scroll | T::EnterNotify | T::LeaveNotify
    ) {
        return glib::Propagation::Proceed;
    }
    // Finger touches belong to the view-level touch APIs; a button or
    // motion event GDK emulated from a touch is not mouse input. (Scroll
    // events are exempt: GDK uses the same flag on a SCROLL for "discrete
    // step emulated from / alongside smooth scrolling", which is still
    // the user's wheel, and GTK never delivers both shapes for one step.)
    if ty != T::Scroll && ev.is_pointer_emulated() {
        return glib::Propagation::Proceed;
    }
    let Some(window) = shared.window.upgrade() else {
        return glib::Propagation::Proceed;
    };
    // Only events on the window's own surface (a popover is its own
    // surface with its own coordinate space).
    if ev.surface() != window.surface() {
        return glib::Propagation::Proceed;
    }
    let (locked, hidden) = {
        let st = shared.state.borrow();
        (st.lock.is_some(), st.hidden)
    };
    if locked || hidden {
        reassert_hidden(&window);
    }
    let Some((sx, sy)) = ev.position() else {
        return glib::Propagation::Proceed;
    };
    let mods = ev.modifier_state();

    match ty {
        T::LeaveNotify => {
            // Re-entry is a jump, not movement.
            shared.state.borrow_mut().last = None;
            glib::Propagation::Proceed
        }
        T::EnterNotify => glib::Propagation::Proceed,
        T::MotionNotify => {
            let (event, warp) = {
                let mut st = shared.state.borrow_mut();
                if let Some((lock, tracker)) = st.lock.as_mut() {
                    let (delta, warp) = tracker.on_motion((sx, sy));
                    let c = lock.center();
                    let event = delta.map(|(dx, dy)| {
                        let (x, y) = to_root(&window, c.0 as f64, c.1 as f64);
                        AppPointerEvent { dx, dy, ..AppPointerEvent::new(PointerPhase::Move, x, y) }
                    });
                    (event, warp)
                } else {
                    let pos = to_root(&window, sx, sy);
                    let (dx, dy) = pointer_policy::motion_delta(st.last, pos);
                    st.last = Some(pos);
                    st.last_surface = Some((sx, sy));
                    (Some(AppPointerEvent { dx, dy, ..AppPointerEvent::new(PointerPhase::Move, pos.0, pos.1) }), false)
                }
            };
            if let Some(event) = event {
                shared.sink.event(&with_modifiers(event, mods));
            }
            if warp {
                if let Some((lock, _)) = shared.state.borrow().lock.as_ref() {
                    lock.recenter();
                }
            }
            if locked {
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        }
        T::ButtonPress | T::ButtonRelease => {
            let Some(button) = ev
                .downcast_ref::<gdk::ButtonEvent>()
                .and_then(|b| pointer_policy::gdk_button(b.button()))
            else {
                return glib::Propagation::Proceed;
            };
            let down = ty == T::ButtonPress;
            let (x, y, was_prevented) = {
                let mut st = shared.state.borrow_mut();
                let pos = match st.lock.as_ref() {
                    Some((lock, _)) => {
                        let c = lock.center();
                        to_root(&window, c.0 as f64, c.1 as f64)
                    }
                    None => to_root(&window, sx, sy),
                };
                st.last = Some(pos);
                let was = st.prevented & button.bit() != 0;
                if !down {
                    st.prevented &= !button.bit();
                }
                (pos.0, pos.1, was)
            };
            let phase = if down { PointerPhase::Down } else { PointerPhase::Up };
            let event = AppPointerEvent { button: Some(button), ..AppPointerEvent::new(phase, x, y) };
            let outcome = shared.sink.event(&with_modifiers(event, mods));
            if down && outcome == PointerOutcome::PreventDefault {
                shared.state.borrow_mut().prevented |= button.bit();
            }
            if !down && was_prevented {
                return glib::Propagation::Stop;
            }
            propagation(outcome)
        }
        T::Scroll => {
            let Some(se) = ev.downcast_ref::<gdk::ScrollEvent>() else {
                return glib::Propagation::Proceed;
            };
            let scroll = match se.direction() {
                gdk::ScrollDirection::Up => Scroll::Up,
                gdk::ScrollDirection::Down => Scroll::Down,
                gdk::ScrollDirection::Left => Scroll::Left,
                gdk::ScrollDirection::Right => Scroll::Right,
                _ => {
                    let (dx, dy) = se.deltas();
                    Scroll::Smooth { dx, dy, surface_units: se.unit() == gdk::ScrollUnit::Surface }
                }
            };
            let (wx, wy) = pointer_policy::scroll_px(scroll);
            if wx == 0.0 && wy == 0.0 {
                // A kinetic-scroll stop marker carries no movement.
                return glib::Propagation::Proceed;
            }
            let (x, y) = shared.state.borrow().last.unwrap_or_else(|| to_root(&window, sx, sy));
            let event = AppPointerEvent { wheel_dx: wx, wheel_dy: wy, ..AppPointerEvent::new(PointerPhase::Wheel, x, y) };
            propagation(shared.sink.event(&with_modifiers(event, mods)))
        }
        _ => glib::Propagation::Proceed,
    }
}
