//! App-level keyboard input — key presses AND releases anywhere in the app,
//! regardless of which view has focus. The input substrate for games
//! (WASD / arrow movement, held keys) and app-wide shortcuts.
//!
//! ## Shape
//!
//! Any number of **listeners** ([`add_listener`]) receive every
//! [`AppKeyEvent`]. The dispatcher also keeps the set of keys currently held
//! ([`is_key_down`] / [`keys_down`]) so a frame loop can poll instead of
//! tracking downs/ups itself.
//!
//! The backend's native key source (a `document` listener on web, an
//! `NSEvent` monitor on macOS, a first-responder view on iOS, a root
//! `OnKeyListener` on Android, a window key controller on GTK, `WM_KEY*` on
//! Win32, the shell's key events on wgpu / terminal) is installed only while
//! at least one listener is live — some of those sources have side effects
//! (iOS/Android take focus to receive hardware keys), so an app that never
//! asks for keys never pays for them.
//!
//! ## Why the install is immediate, not queued
//!
//! The previous single-slot `set_app_key_handler` queued the handler and
//! relied on the next style/theme flush to forward it to the backend. A
//! listener registered from an event handler (a "Start game" button) in an
//! app whose screen registers no new stylesheet never reached the backend
//! at all, silently. Installs now go through a host closure the boot path
//! wires ([`install_keyboard_host`], from
//! `runtime_vocabulary::install_env_services`), so a listener is live the
//! moment it's added.
//!
//! ## Uniform semantics every backend gets for free
//!
//! The dispatcher, not the backends, owns:
//!
//! - **Repeat normalization** — a down for a key already held is
//!   `repeat: true`, whether or not the platform flags repeats (GTK and
//!   iOS don't).
//! - **No stuck keys** — on [`KeyboardSink::focus_lost`] (window blur) the
//!   dispatcher synthesizes a [`KeyPhase::Up`] for every held key, because
//!   no platform delivers the real releases to an unfocused window.
//! - **Batched writes** — each event runs listeners inside one reactive
//!   cycle, so a listener that sets several signals notifies once.
//!
//! Held keys are tracked by [`AppKeyEvent::code`] (physical key), falling
//! back to `key` when a platform can't name the physical key. Tracking by
//! `key` alone would strand keys whose meaning changes mid-hold (`"W"`
//! pressed with Shift, `"w"` released without it).

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

use crate::primitives::key::{AppKeyEvent, KeyDownHandler, KeyOutcome, KeyPhase, KeyboardSink};

/// A listener callback. Return [`KeyOutcome::PreventDefault`] to stop the
/// platform's default handling of the key (page scroll on Space/arrows on
/// web, the macOS "unhandled key" beep).
pub type AppKeyHandler = Rc<dyn Fn(&AppKeyEvent) -> KeyOutcome>;

/// The boot-wired closure that applies a sink to the live backend. Returns
/// `false` when the backend couldn't take it right now (it is mid-call —
/// e.g. a listener added from inside a terminal backend's own key
/// dispatch); the dispatcher retries on the next microtask.
pub type KeyboardHost = Rc<dyn Fn(Option<KeyboardSink>) -> bool>;

struct Listener {
    id: u64,
    alive: Rc<Cell<bool>>,
    handler: AppKeyHandler,
}

#[derive(Default)]
struct State {
    listeners: Vec<Listener>,
    next_id: u64,
    /// Held keys, keyed by `code` (or `key` when `code` is empty), mapped
    /// to the down event that pressed them — its `key` is reused for the
    /// synthesized release on focus loss.
    pressed: BTreeMap<String, AppKeyEvent>,
    host: Option<KeyboardHost>,
    /// Whether the backend currently holds our sink.
    installed: bool,
    /// Nesting depth of in-flight dispatches. Install changes requested
    /// while > 0 wait for the outermost dispatch to finish — the backend
    /// is running the sink at that moment.
    dispatch_depth: u32,
    retry_pending: bool,
    /// The listener backing the single-slot [`set_app_key_handler`].
    legacy: Option<KeyListener>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// RAII registration returned by [`add_listener`]. Dropping it removes the
/// listener (and uninstalls the backend key source if it was the last).
#[must_use = "dropping a KeyListener removes it immediately"]
pub struct KeyListener {
    id: u64,
    alive: Rc<Cell<bool>>,
}

impl Drop for KeyListener {
    fn drop(&mut self) {
        self.alive.set(false);
        let id = self.id;
        // `try_with`: a KeyListener held in another thread-local may drop
        // during thread teardown, after STATE is gone.
        let _ = STATE.try_with(|s| {
            if let Ok(mut s) = s.try_borrow_mut() {
                s.listeners.retain(|l| l.id != id);
            }
        });
        reconcile();
    }
}

impl std::fmt::Debug for KeyListener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyListener").field("id", &self.id).finish()
    }
}

/// Register an app-level key listener. It sees every key down and up in the
/// app (including keys typed into a focused text input — act only on the
/// keys you care about and return [`KeyOutcome::Default`] for the rest)
/// until the returned guard drops.
///
/// Every live listener sees every event; the event's default is prevented
/// if ANY of them returns [`KeyOutcome::PreventDefault`].
///
/// This is the unscoped substrate. Component code normally uses the
/// author-facing `on_key` (runtime-core), which ties the listener to the
/// component's lifetime.
pub fn add_listener(handler: impl Fn(&AppKeyEvent) -> KeyOutcome + 'static) -> KeyListener {
    let alive = Rc::new(Cell::new(true));
    let id = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.next_id += 1;
        let id = s.next_id;
        s.listeners.push(Listener { id, alive: alive.clone(), handler: Rc::new(handler) });
        id
    });
    reconcile();
    KeyListener { id, alive }
}

/// Whether the physical key `code` (Web `KeyboardEvent.code`: `"KeyW"`,
/// `"ArrowUp"`, `"Space"`, `"ShiftLeft"`) is currently held.
///
/// Tracked only while at least one listener is registered (that's what
/// keeps the backend's key source installed) — `on_key` / a `key_state()`
/// handle in the author API does that for you.
pub fn is_key_down(code: &str) -> bool {
    STATE.with(|s| s.borrow().pressed.contains_key(code))
}

/// The physical codes of every key currently held, sorted. Same tracking
/// caveat as [`is_key_down`].
pub fn keys_down() -> Vec<String> {
    STATE.with(|s| s.borrow().pressed.keys().cloned().collect())
}

/// Install (or, with `None`, remove) the single app-level key-DOWN handler.
///
/// Predates [`add_listener`]: it sees key-downs only (auto-repeats
/// included, as before) and there is one slot — a second call replaces the
/// first. Now a thin wrapper over a listener, so it installs immediately
/// rather than on the next style flush. Prefer `on_key` for new code.
pub fn set_app_key_handler(handler: Option<KeyDownHandler>) {
    let listener = handler.map(|h| {
        add_listener(move |e: &AppKeyEvent| {
            if e.phase == KeyPhase::Down {
                h(&e.to_key_event())
            } else {
                KeyOutcome::Default
            }
        })
    });
    // Swap outside the borrow: dropping the old listener re-enters STATE.
    let old = STATE.with(|s| std::mem::replace(&mut s.borrow_mut().legacy, listener));
    drop(old);
}

/// Wire the backend's key-source installer. Called once per boot by
/// `runtime_vocabulary::install_env_services`; `None` detaches (teardown /
/// tests). Listeners registered before boot are installed now.
#[doc(hidden)]
pub fn install_keyboard_host(host: Option<KeyboardHost>) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.host = host;
        // A new backend holds nothing yet.
        s.installed = false;
        s.pressed.clear();
    });
    reconcile();
}

/// Whether the backend currently holds the dispatcher's sink. Test
/// introspection.
#[doc(hidden)]
pub fn is_source_installed() -> bool {
    STATE.with(|s| s.borrow().installed)
}

/// Clear all state (listeners, held keys, host). Test isolation only.
#[doc(hidden)]
pub fn reset_for_tests() {
    let legacy = STATE.with(|s| s.borrow_mut().legacy.take());
    drop(legacy);
    STATE.with(|s| *s.borrow_mut() = State::default());
}

/// The sink handed to backends. Stateless — it routes into the
/// thread-local dispatcher, so one sink serves every install.
fn sink() -> KeyboardSink {
    KeyboardSink::new(dispatch, release_all)
}

/// Deliver one event from a backend: normalize, update the held set, run
/// listeners, then apply any install change they requested.
fn dispatch(raw: &AppKeyEvent) -> KeyOutcome {
    let mut event = raw.clone();
    let listeners = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let id = track_id(&event);
        match event.phase {
            KeyPhase::Down => {
                if s.pressed.contains_key(&id) {
                    event.repeat = true;
                } else {
                    s.pressed.insert(id, event.clone());
                }
            }
            KeyPhase::Up => {
                event.repeat = false;
                s.pressed.remove(&id);
            }
        }
        s.dispatch_depth += 1;
        snapshot(&s)
    });
    let outcome = run_listeners(&listeners, &event);
    end_dispatch();
    outcome
}

/// [`KeyboardSink::focus_lost`]: synthesize an `Up` for every held key.
fn release_all() {
    let (held, listeners) = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let held: Vec<AppKeyEvent> = std::mem::take(&mut s.pressed).into_values().collect();
        s.dispatch_depth += 1;
        (held, snapshot(&s))
    });
    for down in held {
        // Modifiers are cleared: the platform can't tell us their state
        // after focus left, and a stale `shift: true` on the release would
        // be a lie.
        let up = AppKeyEvent::up(down.key, down.code);
        run_listeners(&listeners, &up);
    }
    end_dispatch();
}

fn track_id(event: &AppKeyEvent) -> String {
    if event.code.is_empty() {
        event.key.clone()
    } else {
        event.code.clone()
    }
}

fn snapshot(s: &State) -> Vec<(Rc<Cell<bool>>, AppKeyHandler)> {
    s.listeners.iter().map(|l| (l.alive.clone(), l.handler.clone())).collect()
}

fn run_listeners(listeners: &[(Rc<Cell<bool>>, AppKeyHandler)], event: &AppKeyEvent) -> KeyOutcome {
    crate::cycle(|| {
        let mut outcome = KeyOutcome::Default;
        for (alive, handler) in listeners {
            // A listener removed by an earlier one in this same event
            // (game over → screen unmounts) must not fire.
            if !alive.get() {
                continue;
            }
            if handler(event) == KeyOutcome::PreventDefault {
                outcome = KeyOutcome::PreventDefault;
            }
        }
        outcome
    })
}

fn end_dispatch() {
    STATE.with(|s| s.borrow_mut().dispatch_depth -= 1);
    reconcile();
}

/// Bring the backend's install state in line with "is anyone listening".
fn reconcile() {
    let plan = STATE.try_with(|s| {
        let s = s.try_borrow().ok()?;
        let want = !s.listeners.is_empty();
        if want == s.installed || s.dispatch_depth > 0 {
            return None;
        }
        Some((want, s.host.clone()?))
    });
    let Ok(Some((want, host))) = plan else {
        return;
    };
    // Call the host outside any STATE borrow: installing re-enters the
    // backend, which may call back into us synchronously.
    let applied = host(if want { Some(sink()) } else { None });
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if applied {
            s.installed = want;
            if !want {
                // No source → no more releases will arrive; held state
                // would only go stale.
                s.pressed.clear();
            }
        }
    });
    if !applied {
        schedule_retry();
    }
}

/// The backend was busy — try again once the current stack unwinds. One
/// retry in flight at a time; without a scheduler `schedule_microtask` runs
/// inline on native, and the pending flag stops that from recursing.
fn schedule_retry() {
    let first = STATE.with(|s| {
        let mut s = s.borrow_mut();
        !std::mem::replace(&mut s.retry_pending, true)
    });
    if !first {
        return;
    }
    crate::scheduling::schedule_microtask(|| {
        reconcile();
        STATE.with(|s| s.borrow_mut().retry_pending = false);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake backend: records every sink it's handed and lets the test
    /// drive events through the installed one.
    #[derive(Default)]
    struct FakeHost {
        sink: RefCell<Option<KeyboardSink>>,
        installs: Cell<u32>,
        removals: Cell<u32>,
        busy: Cell<bool>,
    }

    fn wire() -> Rc<FakeHost> {
        reset_for_tests();
        let host = Rc::new(FakeHost::default());
        let h = host.clone();
        install_keyboard_host(Some(Rc::new(move |sink: Option<KeyboardSink>| {
            if h.busy.get() {
                return false;
            }
            match &sink {
                Some(_) => h.installs.set(h.installs.get() + 1),
                None => h.removals.set(h.removals.get() + 1),
            }
            *h.sink.borrow_mut() = sink;
            true
        })));
        host
    }

    fn send(host: &FakeHost, ev: AppKeyEvent) -> KeyOutcome {
        let sink = host.sink.borrow().clone().expect("no sink installed");
        sink.key(&ev)
    }

    fn recorder() -> (Rc<RefCell<Vec<AppKeyEvent>>>, impl Fn(&AppKeyEvent) -> KeyOutcome) {
        let log = Rc::new(RefCell::new(Vec::new()));
        let l = log.clone();
        (log, move |e: &AppKeyEvent| {
            l.borrow_mut().push(e.clone());
            KeyOutcome::Default
        })
    }

    #[test]
    fn source_installs_only_while_a_listener_is_live() {
        let host = wire();
        assert!(!is_source_installed());
        assert_eq!(host.installs.get(), 0);
        let a = add_listener(|_| KeyOutcome::Default);
        let b = add_listener(|_| KeyOutcome::Default);
        assert!(is_source_installed());
        assert_eq!(host.installs.get(), 1, "second listener must not reinstall");
        drop(a);
        assert!(is_source_installed());
        drop(b);
        assert!(!is_source_installed());
        assert_eq!(host.removals.get(), 1);
    }

    #[test]
    fn listeners_see_both_down_and_up() {
        let host = wire();
        let (log, f) = recorder();
        let _l = add_listener(f);
        send(&host, AppKeyEvent::down("w", "KeyW"));
        send(&host, AppKeyEvent::up("w", "KeyW"));
        let log = log.borrow();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].phase, KeyPhase::Down);
        assert_eq!(log[1].phase, KeyPhase::Up);
    }

    #[test]
    fn held_set_tracks_physical_code() {
        let host = wire();
        let _l = add_listener(|_| KeyOutcome::Default);
        send(&host, AppKeyEvent::down("W", "KeyW"));
        send(&host, AppKeyEvent::down("ArrowUp", "ArrowUp"));
        assert!(is_key_down("KeyW"));
        assert_eq!(keys_down(), vec!["ArrowUp".to_string(), "KeyW".to_string()]);
        // Released WITHOUT shift: `key` differs, `code` matches.
        send(&host, AppKeyEvent::up("w", "KeyW"));
        assert!(!is_key_down("KeyW"));
        assert!(is_key_down("ArrowUp"));
    }

    #[test]
    fn regression_repeat_normalized_when_platform_does_not_flag_it() {
        let host = wire();
        let (log, f) = recorder();
        let _l = add_listener(f);
        // GTK / iOS deliver a held key's repeats as plain downs.
        send(&host, AppKeyEvent::down("a", "KeyA"));
        send(&host, AppKeyEvent::down("a", "KeyA"));
        send(&host, AppKeyEvent::down("a", "KeyA"));
        send(&host, AppKeyEvent::up("a", "KeyA"));
        send(&host, AppKeyEvent::down("a", "KeyA"));
        let repeats: Vec<bool> = log.borrow().iter().map(|e| e.repeat).collect();
        assert_eq!(repeats, vec![false, true, true, false, false]);
        assert!(log.borrow()[4].is_press());
    }

    #[test]
    fn regression_focus_loss_releases_held_keys() {
        let host = wire();
        let (log, f) = recorder();
        let _l = add_listener(f);
        let mut shifted = AppKeyEvent::down("D", "KeyD");
        shifted.shift = true;
        send(&host, shifted);
        send(&host, AppKeyEvent::down("ArrowLeft", "ArrowLeft"));
        host.sink.borrow().clone().unwrap().focus_lost();
        assert!(keys_down().is_empty(), "no key may stay stuck after blur");
        let log = log.borrow();
        let ups: Vec<&AppKeyEvent> = log.iter().filter(|e| e.phase == KeyPhase::Up).collect();
        assert_eq!(ups.len(), 2);
        assert!(ups.iter().all(|e| !e.shift && !e.repeat));
        assert!(ups.iter().any(|e| e.code == "KeyD" && e.key == "D"));
    }

    #[test]
    fn prevent_default_if_any_listener_claims() {
        let host = wire();
        let _a = add_listener(|_| KeyOutcome::Default);
        let _b = add_listener(|e| {
            if e.code == "Space" {
                KeyOutcome::PreventDefault
            } else {
                KeyOutcome::Default
            }
        });
        assert_eq!(send(&host, AppKeyEvent::down(" ", "Space")), KeyOutcome::PreventDefault);
        assert_eq!(send(&host, AppKeyEvent::down("x", "KeyX")), KeyOutcome::Default);
    }

    #[test]
    fn listener_removed_mid_dispatch_does_not_fire_and_uninstall_waits() {
        let host = wire();
        let second: Rc<RefCell<Option<KeyListener>>> = Rc::new(RefCell::new(None));
        let fired = Rc::new(Cell::new(false));
        let s2 = second.clone();
        let first = Rc::new(RefCell::new(None));
        let f2 = first.clone();
        *first.borrow_mut() = Some(add_listener(move |_| {
            // Drop BOTH listeners from inside the dispatch (screen unmount).
            s2.borrow_mut().take();
            f2.borrow_mut().take();
            KeyOutcome::Default
        }));
        let fl = fired.clone();
        *second.borrow_mut() = Some(add_listener(move |_| {
            fl.set(true);
            KeyOutcome::Default
        }));
        send(&host, AppKeyEvent::down("q", "KeyQ"));
        assert!(!fired.get(), "a listener dropped earlier in the same event must not run");
        // Uninstall happened after the dispatch unwound, not inside it.
        assert!(!is_source_installed());
        assert_eq!(host.removals.get(), 1);
    }

    #[test]
    fn regression_listener_added_after_boot_installs_without_a_flush() {
        // The old single-slot handler sat in a queue until a style flush.
        let host = wire();
        let _l = add_listener(|_| KeyOutcome::Default);
        assert_eq!(host.installs.get(), 1);
        assert!(host.sink.borrow().is_some());
    }

    #[test]
    fn listener_registered_before_boot_installs_at_boot() {
        reset_for_tests();
        let _l = add_listener(|_| KeyOutcome::Default);
        assert!(!is_source_installed());
        let host = Rc::new(FakeHost::default());
        let h = host.clone();
        install_keyboard_host(Some(Rc::new(move |s| {
            *h.sink.borrow_mut() = s;
            true
        })));
        assert!(is_source_installed());
        assert!(host.sink.borrow().is_some());
    }

    #[test]
    fn busy_backend_install_is_retried() {
        let host = wire();
        host.busy.set(true);
        let _l = add_listener(|_| KeyOutcome::Default);
        // No scheduler in unit tests → the retry ran inline and was still busy.
        assert!(!is_source_installed());
        host.busy.set(false);
        // The next reconcile point (any listener change) picks it up.
        let _m = add_listener(|_| KeyOutcome::Default);
        assert!(is_source_installed());
    }

    #[test]
    fn legacy_handler_sees_downs_only_and_replaces() {
        let host = wire();
        let seen = Rc::new(RefCell::new(Vec::<String>::new()));
        let s = seen.clone();
        set_app_key_handler(Some(Rc::new(move |e: &crate::primitives::key::KeyEvent| {
            s.borrow_mut().push(format!("a:{}", e.key));
            KeyOutcome::Default
        })));
        send(&host, AppKeyEvent::down("x", "KeyX"));
        send(&host, AppKeyEvent::up("x", "KeyX"));
        let s = seen.clone();
        set_app_key_handler(Some(Rc::new(move |e: &crate::primitives::key::KeyEvent| {
            s.borrow_mut().push(format!("b:{}", e.key));
            KeyOutcome::Default
        })));
        send(&host, AppKeyEvent::down("y", "KeyY"));
        assert_eq!(*seen.borrow(), vec!["a:x".to_string(), "b:y".to_string()]);
        set_app_key_handler(None);
        assert!(!is_source_installed());
    }

    #[test]
    fn empty_code_falls_back_to_key_for_tracking() {
        let host = wire();
        let _l = add_listener(|_| KeyOutcome::Default);
        send(&host, AppKeyEvent::down("é", ""));
        assert!(is_key_down("é"));
        send(&host, AppKeyEvent::up("é", ""));
        assert!(keys_down().is_empty());
    }
}
