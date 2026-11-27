//! Author surface for the app's situation and app-wide mouse input:
//!
//! - **Host facts as signals** — [`app_focused`], [`app_visible`],
//!   [`reduce_motion`], [`primary_pointer`], [`user_activated`],
//!   [`pointer_locked`], [`system_color_scheme`], [`splash_dismissed`].
//!   Each is a `ReadSignal`, so effects and `ui!` re-run when the platform
//!   reports a change (auto-pause on blur, skip animations under
//!   reduce-motion, show "needs a mouse" on touch-only devices, start audio
//!   after the first gesture, follow the OS into dark mode, start a
//!   loading screen's animation once the launch screen is gone).
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
use runtime_shared::ColorScheme;
use runtime_shared::held_press::HeldPressEvent;
use runtime_shared::pointer_input::{
    self as substrate, AppPointerEvent, MouseButton, PointerListener, PointerOutcome, PointerPhase,
};
use runtime_world::{inject, provide, signal, unscoped, ReadSignal, Signal};

/// The per-world signals mirroring [`AppState`], plus the launch-screen
/// hand-off (`host_state::splash_dismissed`).
#[derive(Clone)]
struct AppStateCtx {
    focused: Signal<bool>,
    visible: Signal<bool>,
    reduce_motion: Signal<bool>,
    primary_pointer: Signal<PointerKind>,
    user_activated: Signal<bool>,
    pointer_locked: Signal<bool>,
    splash_dismissed: Signal<bool>,
}

runtime_tls::thread_local! {
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
            splash_dismissed: signal(host_state::splash_dismissed()),
        }
    });
    unscoped(|| provide(ctx.clone()));
    let observer = ctx.clone();
    host_state::observe(move |s| observer.apply(s));
    let splash = ctx.splash_dismissed;
    host_state::observe_splash_dismissed(move |d| splash.set(d));
    LAST_CTX.with(|c| *c.borrow_mut() = Some(ctx.clone()));
    ctx
}

runtime_tls::thread_local! {
    /// [`system_color_scheme`]'s handler fallback, as [`LAST_CTX`].
    static LAST_SCHEME: RefCell<Option<Signal<ColorScheme>>> = const { RefCell::new(None) };
}

/// The per-world mirror of `runtime_shared::host::color_scheme`. Its own
/// context, not a field of [`AppStateCtx`]: the preference is reported
/// through `report_color_scheme`, not `app_state`.
#[derive(Clone)]
struct SchemeCtx(Signal<ColorScheme>);

fn scheme_signal() -> Signal<ColorScheme> {
    if !runtime_world::is_entered() {
        if let Some(s) = LAST_SCHEME.with(|c| *c.borrow()) {
            return s;
        }
    }
    if let Some(SchemeCtx(s)) = inject::<SchemeCtx>() {
        LAST_SCHEME.with(|c| *c.borrow_mut() = Some(s));
        return s;
    }
    let s = unscoped(|| signal(runtime_shared::host::color_scheme()));
    unscoped(|| provide(SchemeCtx(s)));
    runtime_shared::host::observe_color_scheme(move |scheme| s.set(scheme));
    LAST_SCHEME.with(|c| *c.borrow_mut() = Some(s));
    s
}

/// The platform's light / dark preference, followed live: the macOS
/// sunset schedule, iOS Control Centre, Android's dark theme, a browser
/// following the OS. [`ColorScheme::Auto`] when the backend cannot tell.
///
/// Read it to keep app-local colours in step with the system:
///
/// ```ignore
/// effect!({
///     let dark = system_color_scheme().get() == ColorScheme::Dark;
///     update_tokens(&brand_tokens(dark));
/// });
/// ```
///
/// A theme that follows the system needs none of this:
/// `Tokens::follow_system` and idea-theme's `ColorScheme::Auto` already
/// re-apply on a change. `color_scheme()` is the same value as a plain read.
pub fn system_color_scheme() -> ReadSignal<ColorScheme> {
    scheme_signal().read_only()
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

/// Whether the platform's launch screen has given way to the app: `true`
/// once the app's first frame is on screen, and never `false` again.
///
/// Time a loading screen that continues the launch screen from this
/// hand-off — its minimum hold, the start of its animation — rather than
/// from boot or [`app_focused`], which can flip while the launch screen
/// is still up.
///
/// ```ignore
/// let shown = splash_dismissed();
/// effect!({
///     if shown.get() {
///         start_glow();
///     }
/// });
/// ```
///
/// - **iOS**: the display-link tick after the first one that finds the
///   app's host view in a visible window, i.e. after the commit that drew
///   the first frame (iOS has no launch-screen callback).
/// - **Android**: right after the root view's first draw — the frame the
///   system splash (API 31+ `SplashScreen`, the theme's
///   `windowBackground` below) hands off to.
/// - **Everywhere else** (web, macOS, Windows, Linux, SSR, GPU, terminal,
///   test hosts): `true` from the start; there is no platform splash.
/// - **runtime-server**: `true` from the start on the sidecar, like every
///   app-situation signal there (none of them cross the wire).
pub fn splash_dismissed() -> ReadSignal<bool> {
    ctx().splash_dismissed.read_only()
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

/// Where the app's root sits in the viewport, in logical pixels. An
/// [`AppPointerEvent`]'s `x`/`y` are relative to the app root; a handle's
/// rect (`PressableHandle::rect`, `absolute_frame`) is relative to the
/// viewport. Add this to the event position before testing it against a
/// rect. `(0, 0)` unless the app is a web app mounted in an offset element.
pub fn pointer_root_origin() -> (f32, f32) {
    substrate::root_origin()
}

/// Follow a finger press that a `PressActivation::Down` pressable took,
/// for as long as the calling scope lives: its moves, and where it lifts.
/// Positions are in viewport pixels, the space handle rects are in. The
/// touch counterpart of [`on_pointer`] for that one case; see
/// `runtime_shared::held_press`. Outside a scope it registers nothing and
/// logs a warning.
pub fn on_held_press(handler: impl Fn(&HeldPressEvent) + 'static) {
    let listener = runtime_shared::held_press::add_listener(handler);
    if let Err(listener) = crate::scoped_scheduling::own_in_scope(listener) {
        drop(listener);
        runtime_shared::logging::log(
            runtime_shared::logging::LogLevel::Warn,
            "on_held_press called outside any component/effect scope; nothing registered.",
        );
    }
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
