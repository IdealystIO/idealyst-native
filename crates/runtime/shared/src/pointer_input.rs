//! App-level mouse input — the pointer counterpart of
//! [`key_input`](crate::key_input): buttons, wheel and movement anywhere in
//! the app, plus **pointer lock** (the cursor is hidden and held in place,
//! and only movement amounts are reported — what first-person games and 3D
//! viewers need).
//!
//! ## Scope
//!
//! Mouse-like devices: mouse, trackpad, pen hover/press. Finger touches
//! are NOT delivered here — they belong to the view-level touch and
//! gesture APIs. A view's own `on_touch` / `on_wheel` still fire as
//! before; this is the app-wide layer above them.
//!
//! ## Shape
//!
//! - Listeners ([`add_listener`]) see every [`AppPointerEvent`]: button
//!   down/up, movement, wheel.
//! - [`request_lock`] / [`exit_lock`] / [`set_cursor_hidden`] go straight
//!   to the backend. Lock state comes back asynchronously (the browser
//!   asks the user's permission implicitly and can refuse; the user can
//!   drop it with Esc) and is published as `AppState::pointer_locked`
//!   (`pointer_locked()` in the author API).
//! - Held buttons are tracked here ([`is_button_down`]) so a frame loop can
//!   poll; movement and wheel are accumulated per polling handle in the
//!   author API (`pointer_state()`).
//!
//! The backend source is installed while anything needs it: a listener, a
//! lock that is wanted or held, or a hidden cursor. Some sources have side
//! effects (suppressing the context menu, capturing the pointer), so an
//! app that never asks pays nothing.
//!
//! ## Uniform semantics owned here
//!
//! - **No stuck buttons** — on [`PointerSink::focus_lost`] the dispatcher
//!   synthesizes an `Up` for every held button.
//! - **Lock bookkeeping** — a lock the platform drops (Esc, alt-tab,
//!   window blur) clears the "wanted" flag, so nothing re-locks behind the
//!   user's back; the app decides whether to ask again.
//! - **Immediate requests** — [`request_lock`] reaches the backend
//!   synchronously, inside the caller's event handler. On web that
//!   matters: `requestPointerLock` is only honored during a user gesture.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::app_state::{self, AppStateChange};
pub use crate::primitives::key::KeyOutcome as PointerOutcome;

/// Logical pixels one notch of a notched mouse wheel scrolls, on every
/// native backend. Platforms disagree natively (AppKit's line ≈ 10 pt,
/// AOSP's scroll factor 64 px, Win32 leaves it to the app, winit reports
/// bare lines), so a wheel turn would move content and cameras a
/// different distance per platform; one value keeps them identical
/// (CLAUDE.md §7). 40 px is Chromium's per-tick value on macOS and Linux.
/// Trackpads / precise wheels report pixels and don't use it. Web line-mode
/// deltas use the CSS line (16 px) instead — that unit is a text line, not
/// a notch — and terminals scroll by cells.
pub const WHEEL_NOTCH_PX: f32 = 40.0;

/// Which button an [`AppPointerEvent`] is about.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "remote-serde", derive(serde::Serialize, serde::Deserialize))]
pub enum MouseButton {
    /// Left button (or the only button / a tap on a trackpad).
    Primary,
    /// Right button (context menu).
    Secondary,
    /// Middle button / wheel click.
    Middle,
    /// Browser "back" side button.
    Back,
    /// Browser "forward" side button.
    Forward,
}

impl MouseButton {
    /// Bit in [`AppPointerEvent::buttons`].
    pub const fn bit(self) -> u8 {
        match self {
            MouseButton::Primary => 1,
            MouseButton::Secondary => 2,
            MouseButton::Middle => 4,
            MouseButton::Back => 8,
            MouseButton::Forward => 16,
        }
    }

    const ALL: [MouseButton; 5] = [
        MouseButton::Primary,
        MouseButton::Secondary,
        MouseButton::Middle,
        MouseButton::Back,
        MouseButton::Forward,
    ];
}

/// What happened.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "remote-serde", derive(serde::Serialize, serde::Deserialize))]
pub enum PointerPhase {
    /// A button went down ([`AppPointerEvent::button`] says which).
    Down,
    /// A button came up.
    Up,
    /// The pointer moved — or, while locked, the mouse moved
    /// ([`dx`](AppPointerEvent::dx)/[`dy`](AppPointerEvent::dy) carry the
    /// amount; the position stays put).
    Move,
    /// The wheel / trackpad scrolled
    /// ([`wheel_dx`](AppPointerEvent::wheel_dx)/[`wheel_dy`](AppPointerEvent::wheel_dy)).
    Wheel,
}

/// One app-level pointer event.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "remote-serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AppPointerEvent {
    pub phase: PointerPhase,
    /// The button for `Down` / `Up`; `None` otherwise.
    pub button: Option<MouseButton>,
    /// Pointer position in logical pixels relative to the app's root.
    /// While locked the cursor doesn't move; this stays at the lock point.
    pub x: f32,
    pub y: f32,
    /// Movement since the previous event, in logical pixels. The ONLY
    /// meaningful motion while locked. Unaccelerated where the platform
    /// offers raw input, otherwise the platform's pointer movement.
    pub dx: f32,
    pub dy: f32,
    /// Wheel amounts in logical pixels (`Wheel` only; 0 otherwise).
    /// Positive `wheel_dy` scrolls content down (wheel toward the user),
    /// matching the web `WheelEvent.deltaY` sign. Line/page-mode wheels
    /// are converted to pixels by the backend.
    pub wheel_dx: f32,
    pub wheel_dy: f32,
    /// Buttons held AFTER this event, as [`MouseButton::bit`]s.
    pub buttons: u8,
    /// Whether the pointer is locked.
    pub locked: bool,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

impl AppPointerEvent {
    /// A blank event of `phase` at `(x, y)`. Convenience for backends and
    /// tests; fill the rest with struct-update syntax.
    pub fn new(phase: PointerPhase, x: f32, y: f32) -> Self {
        AppPointerEvent {
            phase,
            button: None,
            x,
            y,
            dx: 0.0,
            dy: 0.0,
            wheel_dx: 0.0,
            wheel_dy: 0.0,
            buttons: 0,
            locked: false,
            shift: false,
            ctrl: false,
            alt: false,
            meta: false,
        }
    }

    /// Whether `button` is held after this event.
    pub fn is_held(&self, button: MouseButton) -> bool {
        self.buttons & button.bit() != 0
    }
}

/// What a backend's app-level pointer source delivers into. Installed by
/// `AppEnvOps::set_pointer_sink` while the dispatcher needs a source.
///
/// - [`event`](Self::event) for every button down/up, move and wheel; a
///   returned `PreventDefault` suppresses the platform default: focus
///   change / text selection for a down, the context menu for a
///   secondary down (and the platform's separate context-menu event that
///   follows it), page scroll for a wheel.
/// - [`lock_changed`](Self::lock_changed) whenever the lock is granted or
///   lost — including losses the app didn't ask for (Esc, alt-tab).
/// - [`focus_lost`](Self::focus_lost) when the window stops receiving
///   input; releases won't arrive for held buttons.
#[derive(Clone)]
pub struct PointerSink {
    event: Rc<dyn Fn(&AppPointerEvent) -> PointerOutcome>,
    lock_changed: Rc<dyn Fn(bool)>,
    focus_lost: Rc<dyn Fn()>,
}

impl PointerSink {
    pub fn new(
        event: impl Fn(&AppPointerEvent) -> PointerOutcome + 'static,
        lock_changed: impl Fn(bool) + 'static,
        focus_lost: impl Fn() + 'static,
    ) -> Self {
        PointerSink {
            event: Rc::new(event),
            lock_changed: Rc::new(lock_changed),
            focus_lost: Rc::new(focus_lost),
        }
    }

    pub fn event(&self, e: &AppPointerEvent) -> PointerOutcome {
        (self.event)(e)
    }

    pub fn lock_changed(&self, locked: bool) {
        (self.lock_changed)(locked)
    }

    pub fn focus_lost(&self) {
        (self.focus_lost)()
    }
}

impl std::fmt::Debug for PointerSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PointerSink")
    }
}

/// A request the dispatcher sends to the live backend.
#[derive(Clone, Debug)]
pub enum PointerHostCommand {
    /// Install (`Some`) or remove (`None`) the app-level pointer source.
    Sink(Option<PointerSink>),
    /// Ask for (`true`) or release (`false`) pointer lock. The result
    /// arrives through [`PointerSink::lock_changed`].
    Lock(bool),
    /// Hide (`true`) or show the cursor while it is over the app.
    HideCursor(bool),
}

/// The boot-wired closure applying a command to the live backend.
/// Returns `false` when the backend is busy (mid-call); the dispatcher
/// retries on the next microtask.
pub type PointerHost = Rc<dyn Fn(PointerHostCommand) -> bool>;

/// A listener callback.
pub type AppPointerHandler = Rc<dyn Fn(&AppPointerEvent) -> PointerOutcome>;

struct Listener {
    id: u64,
    alive: Rc<Cell<bool>>,
    handler: AppPointerHandler,
}

#[derive(Default)]
struct State {
    listeners: Vec<Listener>,
    next_id: u64,
    buttons: u8,
    position: (f32, f32),
    locked: bool,
    lock_wanted: bool,
    cursor_hidden: bool,
    host: Option<PointerHost>,
    installed: bool,
    /// Commands the backend was busy for; re-sent on retry, in order.
    pending: Vec<PointerHostCommand>,
    dispatch_depth: u32,
    retry_pending: bool,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// RAII registration from [`add_listener`]; dropping it removes the
/// listener.
#[must_use = "dropping a PointerListener removes it immediately"]
pub struct PointerListener {
    id: u64,
    alive: Rc<Cell<bool>>,
}

impl Drop for PointerListener {
    fn drop(&mut self) {
        self.alive.set(false);
        let id = self.id;
        let _ = STATE.try_with(|s| {
            if let Ok(mut s) = s.try_borrow_mut() {
                s.listeners.retain(|l| l.id != id);
            }
        });
        reconcile();
    }
}

impl std::fmt::Debug for PointerListener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PointerListener").field("id", &self.id).finish()
    }
}

/// Register an app-level pointer listener until the guard drops. Every
/// listener sees every event; the default is prevented if any returns
/// [`PointerOutcome::PreventDefault`].
pub fn add_listener(handler: impl Fn(&AppPointerEvent) -> PointerOutcome + 'static) -> PointerListener {
    let alive = Rc::new(Cell::new(true));
    let id = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.next_id += 1;
        let id = s.next_id;
        s.listeners.push(Listener { id, alive: alive.clone(), handler: Rc::new(handler) });
        id
    });
    reconcile();
    PointerListener { id, alive }
}

/// Ask the platform to lock the pointer. Call from a user-gesture handler
/// (a click or key press) — browsers refuse otherwise. The outcome arrives
/// as a `pointer_locked` change; a platform without pointer lock (a
/// terminal, a phone without a mouse) never reports `true`.
pub fn request_lock() {
    STATE.with(|s| s.borrow_mut().lock_wanted = true);
    reconcile();
    send(PointerHostCommand::Lock(true));
}

/// Release the pointer lock (no-op when not locked).
pub fn exit_lock() {
    STATE.with(|s| s.borrow_mut().lock_wanted = false);
    send(PointerHostCommand::Lock(false));
    reconcile();
}

/// Hide (`true`) or show the cursor over the app. A locked pointer is
/// always hidden regardless.
pub fn set_cursor_hidden(hidden: bool) {
    STATE.with(|s| s.borrow_mut().cursor_hidden = hidden);
    reconcile();
    send(PointerHostCommand::HideCursor(hidden));
}

/// Whether the pointer is locked right now.
pub fn is_locked() -> bool {
    STATE.with(|s| s.borrow().locked)
}

/// Whether `button` is held (tracked while a source is installed).
pub fn is_button_down(button: MouseButton) -> bool {
    STATE.with(|s| s.borrow().buttons & button.bit() != 0)
}

/// Last known pointer position (logical px, app-root relative).
pub fn position() -> (f32, f32) {
    STATE.with(|s| s.borrow().position)
}

/// Wire the backend. Called once per boot by
/// `runtime_vocabulary::install_env_services`.
#[doc(hidden)]
pub fn install_pointer_host(host: Option<PointerHost>) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.host = host;
        s.installed = false;
        s.buttons = 0;
        s.locked = false;
        s.pending.clear();
    });
    reconcile();
}

#[doc(hidden)]
pub fn is_source_installed() -> bool {
    STATE.with(|s| s.borrow().installed)
}

#[doc(hidden)]
pub fn reset_for_tests() {
    STATE.with(|s| *s.borrow_mut() = State::default());
}

fn sink() -> PointerSink {
    PointerSink::new(dispatch, lock_changed, release_all)
}

fn dispatch(raw: &AppPointerEvent) -> PointerOutcome {
    let mut event = raw.clone();
    let listeners = STATE.with(|s| {
        let mut s = s.borrow_mut();
        match (event.phase, event.button) {
            (PointerPhase::Down, Some(b)) => s.buttons |= b.bit(),
            (PointerPhase::Up, Some(b)) => s.buttons &= !b.bit(),
            _ => {}
        }
        event.buttons = s.buttons;
        event.locked = s.locked;
        s.position = (event.x, event.y);
        s.dispatch_depth += 1;
        snapshot(&s)
    });
    let outcome = run_listeners(&listeners, &event);
    end_dispatch();
    outcome
}

fn lock_changed(locked: bool) {
    let changed = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let changed = s.locked != locked;
        s.locked = locked;
        if !locked {
            // Lost (or released): don't re-lock behind the user's back.
            s.lock_wanted = false;
        }
        changed
    });
    if changed {
        app_state::report(AppStateChange::PointerLocked(locked));
    }
    reconcile();
}

fn release_all() {
    let (held, listeners, pos) = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let held = s.buttons;
        s.buttons = 0;
        s.dispatch_depth += 1;
        (held, snapshot(&s), s.position)
    });
    for b in MouseButton::ALL {
        if held & b.bit() != 0 {
            let up = AppPointerEvent { button: Some(b), ..AppPointerEvent::new(PointerPhase::Up, pos.0, pos.1) };
            run_listeners(&listeners, &up);
        }
    }
    end_dispatch();
}

fn snapshot(s: &State) -> Vec<(Rc<Cell<bool>>, AppPointerHandler)> {
    s.listeners.iter().map(|l| (l.alive.clone(), l.handler.clone())).collect()
}

fn run_listeners(listeners: &[(Rc<Cell<bool>>, AppPointerHandler)], event: &AppPointerEvent) -> PointerOutcome {
    crate::cycle(|| {
        let mut outcome = PointerOutcome::Default;
        for (alive, handler) in listeners {
            if !alive.get() {
                continue;
            }
            if handler(event) == PointerOutcome::PreventDefault {
                outcome = PointerOutcome::PreventDefault;
            }
        }
        outcome
    })
}

fn end_dispatch() {
    STATE.with(|s| s.borrow_mut().dispatch_depth -= 1);
    reconcile();
}

/// Send a command now; queue it for retry if the backend is busy. Lock
/// requests must not be deferred on web (gesture rule), and in practice a
/// web backend is never borrowed during an author event handler.
fn send(cmd: PointerHostCommand) {
    let Some(host) = STATE.with(|s| s.borrow().host.clone()) else {
        return;
    };
    if !host(cmd.clone()) {
        STATE.with(|s| s.borrow_mut().pending.push(cmd));
        schedule_retry();
    }
}

fn wants_source(s: &State) -> bool {
    !s.listeners.is_empty() || s.lock_wanted || s.locked || s.cursor_hidden
}

fn reconcile() {
    let plan = STATE.try_with(|s| {
        let s = s.try_borrow().ok()?;
        let want = wants_source(&s);
        if want == s.installed || s.dispatch_depth > 0 {
            return None;
        }
        Some((want, s.host.clone()?))
    });
    let Ok(Some((want, host))) = plan else {
        return;
    };
    let applied = host(PointerHostCommand::Sink(if want { Some(sink()) } else { None }));
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if applied {
            s.installed = want;
            if !want {
                s.buttons = 0;
            }
        }
    });
    if !applied {
        schedule_retry();
    }
}

fn schedule_retry() {
    let first = STATE.with(|s| !std::mem::replace(&mut s.borrow_mut().retry_pending, true));
    if !first {
        return;
    }
    crate::scheduling::schedule_microtask(|| {
        // One attempt per pass. `retry_pending` stays set until the pass
        // ends, so a command that fails again re-queues WITHOUT scheduling
        // another pass from inside this one — without a scheduler,
        // `schedule_microtask` runs inline and that would recurse forever.
        reconcile();
        let pending = STATE.with(|s| std::mem::take(&mut s.borrow_mut().pending));
        for cmd in pending {
            send(cmd);
        }
        STATE.with(|s| s.borrow_mut().retry_pending = false);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeHost {
        sink: RefCell<Option<PointerSink>>,
        commands: RefCell<Vec<String>>,
        busy: Cell<bool>,
    }

    fn wire() -> Rc<FakeHost> {
        reset_for_tests();
        crate::app_state::reset_for_tests();
        let host = Rc::new(FakeHost::default());
        let h = host.clone();
        install_pointer_host(Some(Rc::new(move |cmd: PointerHostCommand| {
            if h.busy.get() {
                return false;
            }
            h.commands.borrow_mut().push(match &cmd {
                PointerHostCommand::Sink(s) => format!("sink {}", s.is_some()),
                PointerHostCommand::Lock(v) => format!("lock {v}"),
                PointerHostCommand::HideCursor(v) => format!("hide {v}"),
            });
            if let PointerHostCommand::Sink(s) = cmd {
                *h.sink.borrow_mut() = s;
            }
            true
        })));
        host
    }

    fn sink_of(h: &FakeHost) -> PointerSink {
        h.sink.borrow().clone().expect("no pointer sink installed")
    }

    fn down(b: MouseButton) -> AppPointerEvent {
        AppPointerEvent { button: Some(b), ..AppPointerEvent::new(PointerPhase::Down, 10.0, 20.0) }
    }
    fn up(b: MouseButton) -> AppPointerEvent {
        AppPointerEvent { button: Some(b), ..AppPointerEvent::new(PointerPhase::Up, 10.0, 20.0) }
    }

    #[test]
    fn source_installs_for_listeners_and_lock() {
        let host = wire();
        assert!(!is_source_installed());
        let l = add_listener(|_| PointerOutcome::Default);
        assert!(is_source_installed());
        drop(l);
        assert!(!is_source_installed());
        // A lock alone keeps a source (lock_changed must have somewhere to land).
        request_lock();
        assert!(is_source_installed());
        assert_eq!(*host.commands.borrow(), vec!["sink true", "sink false", "sink true", "lock true"]);
    }

    #[test]
    fn regression_lock_request_reaches_backend_synchronously() {
        // Web only honors requestPointerLock inside the gesture's call stack.
        let host = wire();
        request_lock();
        assert_eq!(host.commands.borrow().last().map(String::as_str), Some("lock true"));
    }

    #[test]
    fn lock_state_published_and_cleared_on_platform_loss() {
        let host = wire();
        request_lock();
        sink_of(&host).lock_changed(true);
        assert!(is_locked());
        assert!(crate::app_state::current().pointer_locked);
        // User pressed Esc: the platform drops it.
        sink_of(&host).lock_changed(false);
        assert!(!is_locked());
        assert!(!crate::app_state::current().pointer_locked);
        // Nothing wanted any more → source goes away.
        assert!(!is_source_installed());
    }

    #[test]
    fn buttons_tracked_and_stamped_on_events() {
        let host = wire();
        let seen = Rc::new(RefCell::new(Vec::new()));
        let s = seen.clone();
        let _l = add_listener(move |e| {
            s.borrow_mut().push((e.phase, e.buttons, e.locked));
            PointerOutcome::Default
        });
        sink_of(&host).event(&down(MouseButton::Primary));
        assert!(is_button_down(MouseButton::Primary));
        sink_of(&host).event(&down(MouseButton::Secondary));
        sink_of(&host).event(&up(MouseButton::Primary));
        assert!(!is_button_down(MouseButton::Primary));
        assert!(is_button_down(MouseButton::Secondary));
        assert_eq!(seen.borrow()[1].1, 3);
        assert_eq!(seen.borrow()[2].1, 2);
        assert_eq!(position(), (10.0, 20.0));
    }

    #[test]
    fn regression_focus_loss_releases_held_buttons() {
        let host = wire();
        let ups = Rc::new(Cell::new(0));
        let u = ups.clone();
        let _l = add_listener(move |e| {
            if e.phase == PointerPhase::Up {
                u.set(u.get() + 1);
            }
            PointerOutcome::Default
        });
        sink_of(&host).event(&down(MouseButton::Primary));
        sink_of(&host).event(&down(MouseButton::Middle));
        sink_of(&host).focus_lost();
        assert_eq!(ups.get(), 2);
        assert!(!is_button_down(MouseButton::Primary) && !is_button_down(MouseButton::Middle));
    }

    #[test]
    fn prevent_default_if_any_listener_claims() {
        let host = wire();
        let _a = add_listener(|_| PointerOutcome::Default);
        let _b = add_listener(|e| {
            if e.button == Some(MouseButton::Secondary) {
                PointerOutcome::PreventDefault
            } else {
                PointerOutcome::Default
            }
        });
        assert_eq!(sink_of(&host).event(&down(MouseButton::Secondary)), PointerOutcome::PreventDefault);
        assert_eq!(sink_of(&host).event(&down(MouseButton::Primary)), PointerOutcome::Default);
    }

    #[test]
    fn cursor_hidden_keeps_source_and_forwards() {
        let host = wire();
        set_cursor_hidden(true);
        assert!(is_source_installed());
        assert_eq!(host.commands.borrow().last().map(String::as_str), Some("hide true"));
        set_cursor_hidden(false);
        assert!(!is_source_installed());
    }

    #[test]
    fn busy_backend_commands_are_retried() {
        let host = wire();
        host.busy.set(true);
        request_lock();
        // No scheduler in unit tests: the retry ran inline while still busy.
        assert!(host.commands.borrow().is_empty());
        host.busy.set(false);
        // The next reconcile point installs; its retry pass re-sends the
        // queued lock.
        let _l = add_listener(|_| PointerOutcome::Default);
        assert!(is_source_installed());
    }
}
