//! Author surface for app-level keyboard input — key presses and releases
//! anywhere in the app, scoped to the component that asks for them.
//!
//! ```ignore
//! #[component]
//! fn Game() -> Element {
//!     let x = signal(0.0_f32);
//!     // Event style: react to presses.
//!     on_key(move |e| {
//!         if e.is_press() && e.code == "Space" {
//!             /* jump */
//!             return KeyOutcome::PreventDefault; // no page scroll on web
//!         }
//!         KeyOutcome::Default
//!     });
//!     // Polling style: read held keys from a frame loop.
//!     let keys = key_state();
//!     raf_loop_scoped(move || {
//!         if keys.is_down("KeyA") || keys.is_down("ArrowLeft") { x.update(|v| v - 2.0) }
//!         if keys.is_down("KeyD") || keys.is_down("ArrowRight") { x.update(|v| v + 2.0) }
//!     });
//!     ui! { /* … */ }
//! }
//! ```
//!
//! Both register through `runtime_shared::key_input`, which owns the
//! cross-backend semantics (down + up, repeat normalization, physical
//! `code`, synthesized releases on focus loss). See that module.

use std::rc::Rc;

use runtime_shared::key_input::{self as substrate, KeyListener};
use runtime_shared::primitives::key::{AppKeyEvent, KeyOutcome};

/// Listen to every app-level key down and up for as long as the calling
/// scope lives — a component body (until it unmounts), an effect run
/// (until it re-runs), or a scoped deferred body. The listener sees keys
/// regardless of focus, including keys typed into a focused text input,
/// so act only on the keys you care about and return
/// [`KeyOutcome::Default`] otherwise.
///
/// Outside any scope (e.g. a button's `on_click`) there is no lifetime to
/// tie it to: use [`add_key_listener`] and hold the guard yourself. Calling
/// this there logs a warning and registers nothing — mirroring the scoped
/// timers, rather than leaking a listener for the rest of the app's life.
pub fn on_key(handler: impl Fn(&AppKeyEvent) -> KeyOutcome + 'static) {
    let listener = substrate::add_listener(handler);
    if let Err(listener) = crate::scoped_scheduling::own_in_scope(listener) {
        drop(listener);
        runtime_shared::logging::log(
            runtime_shared::logging::LogLevel::Warn,
            "on_key called outside any component/effect scope; nothing registered. \
             Use add_key_listener and keep the returned guard.",
        );
    }
}

/// Register an app-level key listener with an explicit lifetime: it stays
/// live until the returned guard drops. The imperative counterpart of
/// [`on_key`] for code that runs outside a scope.
pub fn add_key_listener(handler: impl Fn(&AppKeyEvent) -> KeyOutcome + 'static) -> KeyListener {
    substrate::add_listener(handler)
}

/// A polling view of which keys are held, live for the calling scope (same
/// lifetime rules as [`on_key`]). Cheap to clone; read it from a frame
/// loop or event handler.
///
/// Keys are named by physical [`code`](AppKeyEvent::code) — `"KeyW"`,
/// `"ArrowUp"`, `"Space"`, `"ShiftLeft"` — so WASD works on any layout and
/// a key released after Shift lets go is still recognized.
#[derive(Clone)]
pub struct KeyState {
    // Keeps the backend key source installed while this handle (or its
    // scope) is alive.
    _keepalive: Rc<KeyListener>,
}

impl KeyState {
    /// Whether the physical key `code` is held right now.
    pub fn is_down(&self, code: &str) -> bool {
        substrate::is_key_down(code)
    }

    /// Whether ANY of `codes` is held — e.g. `any_down(&["KeyA", "ArrowLeft"])`.
    pub fn any_down(&self, codes: &[&str]) -> bool {
        codes.iter().any(|c| substrate::is_key_down(c))
    }

    /// Every held key's code, sorted.
    pub fn keys_down(&self) -> Vec<String> {
        substrate::keys_down()
    }
}

/// Start tracking held keys for the calling scope and return a handle to
/// poll them. See [`KeyState`]. Outside a scope the handle itself keeps
/// tracking alive until it (and its clones) drop.
pub fn key_state() -> KeyState {
    let keepalive = Rc::new(substrate::add_listener(|_| KeyOutcome::Default));
    // Tie a clone to the scope so the tracking survives even when the
    // author only keeps the handle inside a closure that outlives nothing.
    let _ = crate::scoped_scheduling::own_in_scope(keepalive.clone());
    KeyState { _keepalive: keepalive }
}
