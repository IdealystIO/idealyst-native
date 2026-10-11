//! Author surface for the app's situation and app-wide mouse input:
//!
//! - **Host facts as signals** — [`app_focused`], [`app_visible`],
//!   [`reduce_motion`], [`primary_pointer`], [`user_activated`],
//!   [`pointer_locked`]. Each is a `ReadSignal`, so effects and `ui!`
//!   re-run when the platform reports a change (auto-pause on blur, skip
//!   animations under reduce-motion, show "needs a mouse" on touch-only
//!   devices, start audio after the first gesture).
//! - **Mouse** — [`on_pointer`] (events, scoped like `on_key`),
//!   [`pointer_state`] (poll held buttons and take accumulated movement /
//!   wheel from a frame loop), [`request_pointer_lock`] /
//!   [`exit_pointer_lock`], [`set_cursor_hidden`].
//!
//! The substrate is `runtime_shared::app_state` (host facts) and
//! `runtime_shared::pointer_input` (dispatcher); see their module docs for
//! the cross-backend contract.
//!
//! ## Per-world signals
//!
//! Like `viewport_size()`, each world owns its signals, seeded from the
//! shared current state on first use and kept in step through a shared
//! observer. Backends report from raw platform callbacks (outside
//! `World::enter`); the writes stage on the signal handles and commit on
//! the backend's post-event flush. Writes into a dead world are no-ops.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use runtime_shared::app_state::{self as host_state, AppState, PointerKind};
use runtime_shared::pointer_input::{
    self as substrate, AppPointerEvent, MouseButton, PointerListener, PointerOutcome, PointerPhase,
};
use runtime_world::{inject, provide, signal, unscoped, ReadSignal, Signal};

/// The per-world signals mirroring [`AppState`].
#[derive(Clone)]
struct AppStateCtx {
    focused: Signal<bool>,
    visible: Signal<bool>,
    reduce_motion: Signal<bool>,
    primary_pointer: Signal<PointerKind>,
    user_activated: Signal<bool>,
    pointer_locked: Signal<bool>,
}

thread_local! {
    /// Handler fallback (same rationale as `viewport::LAST_CTX`): author
    /// code reading these from an event handler runs outside `enter`.
    static LAST_CTX: RefCell<Option<AppStateCtx>> = const { RefCell::new(None) };
}

impl AppStateCtx {
    fn apply(&self, s: &AppState) {
        // Equality-guarded sets: unchanged fields wake nobody.
        self.focused.set(s.focused);
        self.visible.set(s.visible);
        self.reduce_motion.set(s.reduce_motion);
        self.primary_pointer.set(s.primary_pointer);
        self.user_activated.set(s.user_activated);
        self.pointer_locked.set(s.pointer_locked);
    }
}

fn ctx() -> AppStateCtx {
    if !runtime_world::is_entered() {
        if let Some(ctx) = LAST_CTX.with(|c| c.borrow().clone()) {
            return ctx;
        }
    }
    if let Some(ctx) = inject::<AppStateCtx>() {
        LAST_CTX.with(|c| *c.borrow_mut() = Some(ctx.clone()));
        return ctx;
    }
    let ctx = unscoped(|| {
        let s = host_state::current();
        AppStateCtx {
            focused: signal(s.focused),
            visible: signal(s.visible),
            reduce_motion: signal(s.reduce_motion),
            primary_pointer: signal(s.primary_pointer),
            user_activated: signal(s.user_activated),
            pointer_locked: signal(s.pointer_locked),
        }
    });
    unscoped(|| provide(ctx.clone()));
    let observer = ctx.clone();
    host_state::observe(move |s| observer.apply(s));
    LAST_CTX.with(|c| *c.borrow_mut() = Some(ctx.clone()));
    ctx
}

/// Whether the app's window has input focus (the active window / tab).
/// Goes `false` on alt-tab, switching tabs, or clicking another app — the
/// signal to auto-pause a game.
pub fn app_focused() -> ReadSignal<bool> {
    ctx().focused.read_only()
}

/// Whether the app is on screen (`false` for a background tab, a minimized
/// window, or a backgrounded mobile app).
pub fn app_visible() -> ReadSignal<bool> {
    ctx().visible.read_only()
}

/// The OS "reduce motion" accessibility setting. Follows changes live.
pub fn reduce_motion() -> ReadSignal<bool> {
    ctx().reduce_motion.read_only()
}

/// The device's primary pointer: [`PointerKind::Coarse`] means a
/// touch-only device (phone, tablet without a mouse).
pub fn primary_pointer() -> ReadSignal<PointerKind> {
    ctx().primary_pointer.read_only()
}

/// Whether the user has interacted with the app yet (a press, click, or
/// key). Browsers only allow sound after this; on native it is `true`
/// from the start. Never goes back to `false`.
pub fn user_activated() -> ReadSignal<bool> {
    ctx().user_activated.read_only()
}

/// Whether the pointer is locked. Flips to `false` when the platform drops
/// the lock (Esc, alt-tab) — not only when the app releases it.
pub fn pointer_locked() -> ReadSignal<bool> {
    ctx().pointer_locked.read_only()
}

/// Listen to app-wide mouse events (buttons, movement, wheel — not finger
/// touches) for as long as the calling scope lives. Outside a scope it
/// registers nothing and logs a warning; use [`add_pointer_listener`].
pub fn on_pointer(handler: impl Fn(&AppPointerEvent) -> PointerOutcome + 'static) {
    let listener = substrate::add_listener(handler);
    if let Err(listener) = crate::scoped_scheduling::own_in_scope(listener) {
        drop(listener);
        runtime_shared::logging::log(
            runtime_shared::logging::LogLevel::Warn,
            "on_pointer called outside any component/effect scope; nothing registered. \
             Use add_pointer_listener and keep the returned guard.",
        );
    }
}

/// [`on_pointer`] with an explicit lifetime: live until the guard drops.
pub fn add_pointer_listener(
    handler: impl Fn(&AppPointerEvent) -> PointerOutcome + 'static,
) -> PointerListener {
    substrate::add_listener(handler)
}

/// Ask for pointer lock. Call it from a click or key handler — browsers
/// refuse a request that isn't part of a user gesture. Watch
/// [`pointer_locked`] for the result.
pub fn request_pointer_lock() {
    substrate::request_lock()
}

/// Release pointer lock.
pub fn exit_pointer_lock() {
    substrate::exit_lock()
}

/// Hide (`true`) or show the cursor over the app. A locked pointer is
/// always hidden.
pub fn set_cursor_hidden(hidden: bool) {
    substrate::set_cursor_hidden(hidden)
}

#[derive(Default)]
struct Accum {
    dx: Cell<f32>,
    dy: Cell<f32>,
    wheel_dx: Cell<f32>,
    wheel_dy: Cell<f32>,
}

/// A polling view of the mouse for a frame loop. Each handle (and its
/// clones) has its OWN movement and wheel accumulators, so two consumers
/// never steal each other's deltas.
#[derive(Clone)]
pub struct PointerState {
    accum: Rc<Accum>,
    _keepalive: Rc<PointerListener>,
}

impl PointerState {
    /// Whether `button` is held.
    pub fn is_down(&self, button: MouseButton) -> bool {
        substrate::is_button_down(button)
    }

    /// Movement since the last call (logical px), then reset to zero. While
    /// locked this is the mouse motion to turn the camera by.
    pub fn take_movement(&self) -> (f32, f32) {
        (self.accum.dx.replace(0.0), self.accum.dy.replace(0.0))
    }

    /// Wheel scrolled since the last call (logical px), then reset.
    pub fn take_wheel(&self) -> (f32, f32) {
        (self.accum.wheel_dx.replace(0.0), self.accum.wheel_dy.replace(0.0))
    }

    /// Last pointer position (logical px, app-root relative).
    pub fn position(&self) -> (f32, f32) {
        substrate::position()
    }

    /// Whether the pointer is locked right now.
    pub fn is_locked(&self) -> bool {
        substrate::is_locked()
    }

    /// Drop accumulated movement and wheel (e.g. when unpausing, so a
    /// paused-time mouse move doesn't snap the camera).
    pub fn clear(&self) {
        let _ = self.take_movement();
        let _ = self.take_wheel();
    }
}

/// Start tracking the mouse for the calling scope and return a handle to
/// poll. Outside a scope the handle keeps tracking alive until it drops.
pub fn pointer_state() -> PointerState {
    let accum = Rc::new(Accum::default());
    let a = accum.clone();
    let listener = substrate::add_listener(move |e| {
        match e.phase {
            PointerPhase::Move => {
                a.dx.set(a.dx.get() + e.dx);
                a.dy.set(a.dy.get() + e.dy);
            }
            PointerPhase::Wheel => {
                a.wheel_dx.set(a.wheel_dx.get() + e.wheel_dx);
                a.wheel_dy.set(a.wheel_dy.get() + e.wheel_dy);
            }
            _ => {}
        }
        PointerOutcome::Default
    });
    let keepalive = Rc::new(listener);
    let _ = crate::scoped_scheduling::own_in_scope(keepalive.clone());
    PointerState { accum, _keepalive: keepalive }
}
