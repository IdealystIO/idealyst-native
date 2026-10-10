//! App-level keyboard source: key down AND up for the whole window,
//! delivered into the framework's [`KeyboardSink`]
//! (`AppEnvOps::set_keyboard_sink`).
//!
//! ## Why a CAPTURE-phase controller on the host window
//!
//! GTK4 routes a key event from the toplevel down to the focus widget
//! (capture), then back up (bubble). A controller on the window in the
//! default bubble phase only sees keys the focused widget did NOT consume —
//! a focused `GtkEntry` eats every printable, so a game listener would go
//! deaf the moment an input has focus. In the capture phase the window
//! controller runs FIRST for every key, whichever widget is focused, and
//! returning `Propagation::Stop` (for `KeyOutcome::PreventDefault`) keeps
//! the key from the entry — the Web `preventDefault` shape. `Proceed`
//! leaves the focused input's own handling (and its `on_key_down`)
//! untouched.
//!
//! GTK4 emits `key-pressed` / `key-released` for modifier keys themselves
//! (the separate `modifiers` signal is additional, not a replacement), so
//! holding Shift produces a down and an up like any other key. GTK has no
//! auto-repeat flag; repeats arrive as plain `key-pressed` and the
//! dispatcher marks them (`runtime_shared::key_input`). `key-released` has
//! no return value, so a release cannot be swallowed — there is no native
//! default to suppress on release anyway.
//!
//! ## Focus loss
//!
//! A window that stops being the active toplevel receives no further key
//! events, including the releases of keys held at that moment. The
//! `notify::is-active` handler reports that to the sink, which synthesizes
//! the releases so no key sticks down.
//!
//! ## Re-entrancy
//!
//! The closures own a clone of the sink and never touch backend state, so
//! no backend `RefCell` borrow is held while author listeners run — a
//! listener may add/remove listeners, which calls back into
//! `set_keyboard_sink` on this backend.

use gtk4::glib;
use gtk4::prelude::*;

use runtime_shared::primitives::key::{AppKeyEvent, KeyOutcome, KeyPhase, KeyboardSink};

use crate::keymap;

/// The installed native source, kept so `set_keyboard_sink(None)` (or a
/// replacement) can tear it down.
pub(crate) struct AppKeySource {
    controller: gtk4::EventControllerKey,
    active_handler: glib::SignalHandlerId,
}

/// Translate one GDK key event into the framework's [`AppKeyEvent`].
pub(crate) fn app_key_event(
    phase: KeyPhase,
    keyval: gtk4::gdk::Key,
    keycode: u32,
    state: gtk4::gdk::ModifierType,
) -> AppKeyEvent {
    use gtk4::gdk::ModifierType as M;
    let code = keymap::hardware_keycode_to_code(keycode);
    let (shift, ctrl, alt, meta) = keymap::own_modifier_flags(
        phase == KeyPhase::Down,
        code,
        state.contains(M::SHIFT_MASK),
        state.contains(M::CONTROL_MASK),
        state.contains(M::ALT_MASK),
        state.intersects(M::SUPER_MASK | M::META_MASK),
    );
    AppKeyEvent {
        phase,
        key: crate::key_name(keyval),
        code: code.to_string(),
        repeat: false,
        shift,
        ctrl,
        alt,
        meta,
    }
}

/// Deliver one key event and map the outcome onto GTK propagation.
/// Split out of the signal closure so the GTK integration test can drive
/// it with real `gdk::Key` values (GTK4 has no public API to synthesize a
/// `GdkKeyEvent`).
pub(crate) fn deliver(
    sink: &KeyboardSink,
    phase: KeyPhase,
    keyval: gtk4::gdk::Key,
    keycode: u32,
    state: gtk4::gdk::ModifierType,
) -> glib::Propagation {
    let ev = app_key_event(phase, keyval, keycode, state);
    match sink.key(&ev) {
        KeyOutcome::PreventDefault => glib::Propagation::Stop,
        KeyOutcome::Default => glib::Propagation::Proceed,
    }
}

impl AppKeySource {
    /// Attach the capture-phase key controller and the focus-loss watcher
    /// to `window`.
    pub(crate) fn install(window: &gtk4::Window, sink: KeyboardSink) -> Self {
        let controller = gtk4::EventControllerKey::new();
        controller.set_propagation_phase(gtk4::PropagationPhase::Capture);

        let down_sink = sink.clone();
        controller.connect_key_pressed(move |_, keyval, keycode, state| {
            deliver(&down_sink, KeyPhase::Down, keyval, keycode, state)
        });
        let up_sink = sink.clone();
        controller.connect_key_released(move |_, keyval, keycode, state| {
            // No return value: GTK cannot cancel a release.
            let _ = deliver(&up_sink, KeyPhase::Up, keyval, keycode, state);
        });
        window.add_controller(controller.clone());

        let active_handler = window.connect_is_active_notify(move |w| {
            if !w.is_active() {
                sink.focus_lost();
            }
        });

        AppKeySource { controller, active_handler }
    }

    /// Detach both native hooks. After this the sink's closures are
    /// unreachable from GTK (the controller's closures drop with it).
    pub(crate) fn remove(self, window: &gtk4::Window) {
        window.remove_controller(&self.controller);
        window.disconnect(self.active_handler);
    }

    /// The installed controller (tests assert it is attached / detached).
    #[allow(dead_code)]
    pub(crate) fn controller(&self) -> &gtk4::EventControllerKey {
        &self.controller
    }
}
