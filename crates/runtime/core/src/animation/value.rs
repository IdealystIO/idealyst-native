//! [`AnimatedValue<T>`] — the value handle authors actually hold.
//!
//! ```ignore
//! let scale = AnimatedValue::new(1.0_f32);
//!
//! // Snap, no animation.
//! scale.set(1.0);
//!
//! // Tween to 1.1 over 150ms.
//! scale.animate(TweenTo::new(1.1, Duration::from_millis(150)).ease_out());
//!
//! // Hand off to a spring (preserves whatever velocity the tween had).
//! scale.animate(SpringTo::new(1.0).stiffness(280).damping(22));
//! ```
//!
//! # Identity model
//!
//! `AnimatedValue<T>` is cheap to clone — internally a `Rc<RefCell<…>>`
//! handle, so multiple clones share the same underlying storage. Pass
//! clones into closures the way `Signal` clones are passed around.
//!
//! # Threading
//!
//! Single-threaded: `Rc` + `RefCell` are deliberate (see project
//! rationale on threading). Listeners are dispatched *outside*
//! the value-state borrow (each listener slot is held behind its
//! own `Rc<RefCell<…>>` so the dispatch loop snapshots the
//! handles and releases the outer borrow before invoking), so a
//! listener may freely call `get` / `set` / `animate` / `cancel`
//! / `subscribe` on the same value. The one runtime panic is a
//! listener that re-invokes *itself* — which is a real bug, not
//! a footgun.

use std::cell::RefCell;
use std::rc::Rc;

use super::animator::{Animator, AnimatorFactory};
use super::clock::{register_guarded, TickRegistration};
use super::Animatable;

/// A reactive scalar (or compound) value that can be driven by
/// [`Animator`]s. Cheap to clone — clones share state.
pub struct AnimatedValue<T: Animatable> {
    inner: Rc<RefCell<Inner<T>>>,
}

impl<T: Animatable> Clone for AnimatedValue<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Rc::clone(&self.inner),
        }
    }
}

struct Inner<T: Animatable> {
    value: T,
    velocity: T,
    animator: Option<Box<dyn Animator<T>>>,
    /// Held while an animator is live; dropping it unregisters the
    /// tick from the clock and removes per-frame work for this
    /// value.
    tick: Option<TickRegistration>,
    listeners: Vec<Listener<T>>,
    next_listener_id: u64,
}

struct Listener<T: Animatable> {
    id: u64,
    /// Wrapped in `Rc<RefCell<…>>` so the dispatch loop can clone
    /// each handle out of the inner state, release the outer
    /// borrow on `Inner`, and *then* invoke the listener. That way
    /// a listener can call `.get()` / `.set(…)` / `.animate(…)` /
    /// `.subscribe(…)` on the same value without tripping the
    /// outer `RefCell` — only the listener's own slot is locked
    /// while it runs, which only matters if the listener invokes
    /// *itself* (which is a runtime panic, intentionally — that's
    /// a real bug, not a re-entrancy footgun).
    f: Rc<RefCell<Box<dyn FnMut(&T, &T)>>>,
}

impl<T: Animatable> AnimatedValue<T> {
    /// Build a new value at `initial`, with zero velocity and no
    /// active animator.
    pub fn new(initial: T) -> Self {
        Self {
            inner: Rc::new(RefCell::new(Inner {
                value: initial,
                velocity: T::zero(),
                animator: None,
                tick: None,
                listeners: Vec::new(),
                next_listener_id: 0,
            })),
        }
    }

    /// Read the current value. Cheap (`Clone`-bounded).
    pub fn get(&self) -> T {
        self.inner.borrow().value.clone()
    }

    /// Read the current velocity in `T`-per-second. Returns `T::zero()`
    /// when no animator is active.
    pub fn velocity(&self) -> T {
        self.inner.borrow().velocity.clone()
    }

    /// Snap to `value`. Cancels any running animator, zeroes
    /// velocity, notifies listeners.
    ///
    /// Use this for instantaneous changes (e.g., gesture move: the
    /// finger is driving the value frame-by-frame; no animator
    /// needed in between).
    pub fn set(&self, value: T) {
        {
            let mut inner = self.inner.borrow_mut();
            inner.value = value;
            inner.velocity = T::zero();
            inner.animator = None;
            inner.tick = None;
        }
        self.notify();
    }

    /// Replace the active animator with one built by `factory`.
    ///
    /// The factory receives the *current* value and velocity; this
    /// is what gives us velocity-preserving handoff between
    /// animators. A tween mid-flight, replaced with a spring, will
    /// produce a spring that starts at the tween's current
    /// position with the tween's current (finite-difference)
    /// velocity.
    pub fn animate<F: AnimatorFactory<T>>(&self, factory: F) {
        let (current, velocity) = {
            let inner = self.inner.borrow();
            (inner.value.clone(), inner.velocity.clone())
        };
        let animator = factory.build(current, velocity);

        // Replace the animator, then ensure a tick is registered.
        // The clock-ticking closure captures a Weak<RefCell<Inner>>
        // so it doesn't keep the value alive past its last public
        // handle; on drop the registration cancels and the slot
        // frees.
        let needs_tick = {
            let mut inner = self.inner.borrow_mut();
            inner.animator = Some(animator);
            inner.tick.is_none()
        };
        if needs_tick {
            let weak = Rc::downgrade(&self.inner);
            let registration = register_guarded(Box::new(move |dt| {
                let Some(strong) = weak.upgrade() else {
                    return false; // value handle dropped → stop ticking
                };
                drive(strong, dt)
            }));
            // Store the registration so it lives at least as long
            // as the animator does. Re-borrow now that the closure
            // is built.
            self.inner.borrow_mut().tick = Some(registration);
        }
    }

    /// Stop the running animator without changing the current
    /// value. Velocity is **preserved** — a subsequent
    /// `animate(…)` call still sees it for handoff. Use [`set`]
    /// instead if you also want to zero velocity.
    pub fn cancel(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.animator = None;
        inner.tick = None;
    }

    /// Whether an animator is currently driving the value.
    pub fn is_animating(&self) -> bool {
        self.inner.borrow().animator.is_some()
    }

    /// Subscribe to value changes. The closure runs every time the
    /// value mutates — via `set`, via the clock advancing the
    /// animator, or on snap-to-target settle. Returns a
    /// [`Subscription`] guard that removes the listener on drop.
    ///
    /// The closure receives `(value, velocity)`. It may freely
    /// call `get` / `set` / `animate` / `cancel` / `subscribe` on
    /// the same value (re-entry is supported); the one constraint
    /// is that a listener must not invoke *itself* recursively.
    pub fn subscribe<F: FnMut(&T, &T) + 'static>(&self, f: F) -> Subscription<T> {
        let mut inner = self.inner.borrow_mut();
        let id = inner.next_listener_id;
        inner.next_listener_id += 1;
        inner.listeners.push(Listener {
            id,
            f: Rc::new(RefCell::new(Box::new(f))),
        });
        Subscription {
            inner: Rc::downgrade(&self.inner),
            id: Some(id),
        }
    }

    /// [`Self::subscribe`] plus an immediate call to the closure
    /// with the value handle's current `(value, velocity)`. The
    /// typical use case is wiring an animated value to a backend
    /// property: without this, the backend wouldn't reflect the
    /// value's *starting* position until the first frame tick
    /// (or until the value next mutates), which leaves a visual
    /// gap between mount and first paint. Calling
    /// `subscribe_and_apply` makes the binding immediately
    /// consistent.
    pub fn subscribe_and_apply<F: FnMut(&T, &T) + 'static>(&self, mut f: F) -> Subscription<T> {
        // Fire once with current state before subscribing so the
        // initial call sees a clean snapshot (no risk of the
        // listener firing twice in a frame if a tick lands
        // between fire and subscribe). Then subscribe — taking
        // ownership of `f` for repeat invocations.
        let (value, velocity) = {
            let inner = self.inner.borrow();
            (inner.value.clone(), inner.velocity.clone())
        };
        f(&value, &velocity);
        self.subscribe(f)
    }

    fn notify(&self) {
        // Snapshot (value, velocity, listener handles) under a
        // brief borrow, drop the borrow, then invoke. See the
        // comment on `Listener::f` for the re-entry rationale.
        let (value, velocity, snapshot) = {
            let inner = self.inner.borrow();
            let snapshot: Vec<Rc<RefCell<Box<dyn FnMut(&T, &T)>>>> = inner
                .listeners
                .iter()
                .map(|l| Rc::clone(&l.f))
                .collect();
            (inner.value.clone(), inner.velocity.clone(), snapshot)
        };
        dispatch(&snapshot, &value, &velocity);
    }
}

/// Invoke each listener handle in `snapshot`. Uses
/// [`RefCell::try_borrow_mut`] so a listener that triggers a
/// chain leading back to its own slot ("listener calls `set` on
/// the same value which re-enters `notify`") silently skips its
/// own re-invocation instead of panicking. Other listeners on
/// the value still fire normally on the inner call.
fn dispatch<T: Animatable>(
    snapshot: &[Rc<RefCell<Box<dyn FnMut(&T, &T)>>>],
    value: &T,
    velocity: &T,
) {
    for f in snapshot {
        if let Ok(mut closure) = f.try_borrow_mut() {
            (closure)(value, velocity);
        }
        // else: listener is currently mid-invocation higher up
        // the call stack; skip to avoid re-entering its slot.
    }
}

/// Tick the value's animator forward by `dt` and notify listeners.
/// Returns `true` while the animator still has work to do, `false`
/// once it's settled (and the tick can be unregistered).
fn drive<T: Animatable>(inner: Rc<RefCell<Inner<T>>>, dt: std::time::Duration) -> bool {
    let (value, velocity, finished) = {
        let mut i = inner.borrow_mut();
        let Some(animator) = i.animator.as_mut() else {
            return false;
        };
        let sample = animator.sample(dt);
        i.value = sample.value.clone();
        i.velocity = sample.velocity.clone();
        if sample.finished {
            i.animator = None;
            // Clear our tick handle in the same step. The clock
            // is *not* borrowed while a tick closure runs (the
            // dispatch loop pulls the closure out of the map
            // before invocation), so dropping the
            // `TickRegistration` here is safe — `unregister`
            // succeeds (the id is already gone from the map, so
            // the call no-ops) and the field clears, which lets
            // the next `animate(…)` notice `tick.is_none()` and
            // install a fresh registration. Without this, the
            // stale `Some(_)` would cause the next animation to
            // never tick.
            i.tick = None;
        }
        (sample.value, sample.velocity, sample.finished)
    };

    // Snapshot listener handles under an immutable borrow, then
    // release before invoking. Same re-entry pattern as
    // `AnimatedValue::notify`.
    let snapshot: Vec<Rc<RefCell<Box<dyn FnMut(&T, &T)>>>> = inner
        .borrow()
        .listeners
        .iter()
        .map(|l| Rc::clone(&l.f))
        .collect();
    dispatch(&snapshot, &value, &velocity);

    // Debug-only: catch animations that never settle. A normal tween/spring
    // finishes in well under a second; a tick that keeps reporting `!finished`
    // for seconds pins the 60 Hz animation clock on forever, forcing a
    // main-thread `CA::Transaction::commit` every frame (the macOS "All
    // Components" scroll-jank root cause). Log its value type + velocity
    // magnitude so the stuck animator can be traced. Keyed by the value's
    // identity so each stuck value reports once per ~3 s window.
    #[cfg(debug_assertions)]
    __debug_track_long_anim::<T>(Rc::as_ptr(&inner) as usize, dt, finished, &velocity);

    !finished
}

#[cfg(debug_assertions)]
thread_local! {
    /// key = value-identity ptr → (cumulative secs, tick count, last-logged secs).
    static LONG_ANIM: RefCell<std::collections::HashMap<usize, (f64, u32, f64)>> =
        RefCell::new(std::collections::HashMap::new());
}

/// See the call site in [`drive`]. Tracks per-value cumulative animation time
/// and warns once every ~3 s for any animation still running past 3 s.
#[cfg(debug_assertions)]
fn __debug_track_long_anim<T: Animatable>(
    key: usize,
    dt: std::time::Duration,
    finished: bool,
    velocity: &T,
) {
    LONG_ANIM.with(|m| {
        let mut m = m.borrow_mut();
        if finished {
            m.remove(&key);
            return;
        }
        let e = m.entry(key).or_insert((0.0, 0, 0.0));
        e.0 += dt.as_secs_f64();
        e.1 += 1;
        if e.0 >= 3.0 && (e.0 - e.2) >= 3.0 {
            e.2 = e.0;
            let vmag = T::norm_sq(velocity).sqrt();
            crate::log_warn!(
                "[anim-stuck] {} running {:.1}s ({} ticks), |vel|={:.5} — never settled (pins the 60Hz clock)",
                std::any::type_name::<T>(),
                e.0,
                e.1,
                vmag
            );
        }
    });
}

/// RAII guard for a subscription. Dropping unsubscribes the
/// listener.
pub struct Subscription<T: Animatable> {
    inner: std::rc::Weak<RefCell<Inner<T>>>,
    id: Option<u64>,
}

impl<T: Animatable> Drop for Subscription<T> {
    fn drop(&mut self) {
        let Some(id) = self.id.take() else { return };
        if let Some(strong) = self.inner.upgrade() {
            let mut inner = strong.borrow_mut();
            inner.listeners.retain(|l| l.id != id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::clock::tick_for_test;
    use super::super::{DecayFrom, SpringTo, TweenTo};
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::Duration;

    const STEP: Duration = Duration::from_millis(16);

    fn approx_eq(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() < tol
    }

    fn drain_animation(v: &AnimatedValue<f32>) {
        for _ in 0..2_000 {
            if !v.is_animating() {
                break;
            }
            tick_for_test(STEP);
        }
        assert!(!v.is_animating(), "value never finished animating");
    }

    #[test]
    fn new_value_starts_at_initial_with_zero_velocity() {
        let v = AnimatedValue::new(7.0_f32);
        assert_eq!(v.get(), 7.0);
        assert_eq!(v.velocity(), 0.0);
        assert!(!v.is_animating());
    }

    #[test]
    fn set_snaps_value_zeroes_velocity_cancels_animator() {
        let v = AnimatedValue::new(0.0_f32);
        v.animate(TweenTo::new(10.0, Duration::from_millis(100)).linear());
        // Mid-flight snap.
        tick_for_test(STEP);
        assert!(v.is_animating());
        v.set(3.5);
        assert_eq!(v.get(), 3.5);
        assert_eq!(v.velocity(), 0.0);
        assert!(!v.is_animating());
    }

    #[test]
    fn tween_drives_to_target() {
        let v = AnimatedValue::new(0.0_f32);
        v.animate(TweenTo::new(1.0, Duration::from_millis(120)).linear());
        drain_animation(&v);
        assert!(approx_eq(v.get(), 1.0, 1e-4));
    }

    #[test]
    fn spring_drives_to_target() {
        let v = AnimatedValue::new(0.0_f32);
        v.animate(SpringTo::new(1.0));
        drain_animation(&v);
        assert!(approx_eq(v.get(), 1.0, 1e-4));
    }

    #[test]
    fn decay_lands_somewhere_past_start() {
        let v = AnimatedValue::new(0.0_f32);
        v.animate(DecayFrom::new(10.0));
        drain_animation(&v);
        // With initial_velocity = 10 and default friction = 3,
        // closed-form rest = 0 + 10/3 ≈ 3.33.
        assert!(approx_eq(v.get(), 10.0 / 3.0, 0.1));
    }

    #[test]
    fn handoff_preserves_velocity_across_factory_swap() {
        // Run a tween mid-way (so velocity is nonzero), then
        // replace with a spring whose seed velocity comes from the
        // tween's finite-difference output. The spring should
        // *continue* moving in the same direction on its first
        // sample.
        let v = AnimatedValue::new(0.0_f32);
        v.animate(TweenTo::new(10.0, Duration::from_millis(100)).linear());
        // Drive a few frames to build velocity.
        for _ in 0..3 {
            tick_for_test(STEP);
        }
        let mid_velocity = v.velocity();
        assert!(mid_velocity > 0.0, "tween velocity was {}", mid_velocity);

        // Hand off to a spring targeting the current value (zero
        // displacement) — only the inherited velocity drives the
        // first frame's motion.
        let mid_value = v.get();
        v.animate(SpringTo::new(mid_value));
        tick_for_test(STEP);
        // The spring saw a positive seed velocity, so the next
        // sample's velocity should still be positive (slightly
        // damped, but in the same direction).
        assert!(
            v.velocity() > 0.0,
            "post-handoff velocity was {}",
            v.velocity()
        );
    }

    #[test]
    fn cancel_preserves_velocity_for_subsequent_animate() {
        let v = AnimatedValue::new(0.0_f32);
        v.animate(TweenTo::new(10.0, Duration::from_millis(100)).linear());
        for _ in 0..3 {
            tick_for_test(STEP);
        }
        let cached_velocity = v.velocity();
        assert!(cached_velocity > 0.0);
        v.cancel();
        assert!(!v.is_animating());
        // Velocity still reads as the cached value.
        assert_eq!(v.velocity(), cached_velocity);
    }

    #[test]
    fn subscribe_fires_on_set() {
        let v = AnimatedValue::new(0.0_f32);
        let count = Rc::new(Cell::new(0u32));
        let cc = count.clone();
        let _sub = v.subscribe(move |_value, _vel| cc.set(cc.get() + 1));
        v.set(1.0);
        v.set(2.0);
        assert_eq!(count.get(), 2);
    }

    #[test]
    fn subscribe_fires_per_frame_during_animation() {
        let v = AnimatedValue::new(0.0_f32);
        let count = Rc::new(Cell::new(0u32));
        let cc = count.clone();
        let _sub = v.subscribe(move |_value, _vel| cc.set(cc.get() + 1));
        v.animate(TweenTo::new(1.0, Duration::from_millis(64)).linear());
        for _ in 0..6 {
            tick_for_test(STEP);
            if !v.is_animating() {
                break;
            }
        }
        // Tween over 64ms at 16ms steps = 4 frames + the settled
        // frame; listener should have run at least 3 times.
        assert!(count.get() >= 3, "listener fired only {} times", count.get());
    }

    #[test]
    fn subscribe_and_apply_fires_synchronously() {
        let v = AnimatedValue::new(7.5_f32);
        let observed = Rc::new(Cell::new(0.0_f32));
        let o = observed.clone();
        let _sub = v.subscribe_and_apply(move |value, _vel| o.set(*value));
        // Listener should have fired during subscribe_and_apply
        // with the current value, before any tick or set call.
        assert_eq!(observed.get(), 7.5);
    }

    #[test]
    fn subscribe_and_apply_still_fires_on_subsequent_changes() {
        let v = AnimatedValue::new(1.0_f32);
        let count = Rc::new(Cell::new(0u32));
        let c = count.clone();
        let _sub = v.subscribe_and_apply(move |_value, _vel| c.set(c.get() + 1));
        // One immediate fire from subscribe_and_apply.
        assert_eq!(count.get(), 1);
        v.set(2.0);
        // Now twice: immediate + the set.
        assert_eq!(count.get(), 2);
    }

    #[test]
    fn subscription_drop_unsubscribes() {
        let v = AnimatedValue::new(0.0_f32);
        let count = Rc::new(Cell::new(0u32));
        let cc = count.clone();
        let sub = v.subscribe(move |_value, _vel| cc.set(cc.get() + 1));
        v.set(1.0);
        drop(sub);
        v.set(2.0);
        // Only the first set fired; second was after unsubscribe.
        assert_eq!(count.get(), 1);
    }

    #[test]
    fn clone_shares_state() {
        let v = AnimatedValue::new(0.0_f32);
        let cloned = v.clone();
        v.set(5.0);
        assert_eq!(cloned.get(), 5.0);
    }

    #[test]
    fn listener_can_read_value_during_dispatch() {
        // Listener reads `.get()` on the same value handle —
        // would deadlock if dispatch held the outer RefCell.
        let v = AnimatedValue::new(0.0_f32);
        let observed = Rc::new(Cell::new(0.0_f32));
        let v_clone = v.clone();
        let o = observed.clone();
        let _sub = v.subscribe(move |_value, _vel| {
            o.set(v_clone.get());
        });
        v.set(42.0);
        assert_eq!(observed.get(), 42.0);
    }

    #[test]
    fn listener_can_mutate_value_during_dispatch() {
        // Listener calls `.set()` on a different handle of the
        // same value — this is the gesture-handler shape ("the
        // value changed, fire off another animation").
        let v = AnimatedValue::new(0.0_f32);
        let armed = Rc::new(Cell::new(false));
        let v_clone = v.clone();
        let a = armed.clone();
        let _sub = v.subscribe(move |value, _vel| {
            // Only re-arm once to avoid recursion in the test.
            if *value > 0.0 && !a.get() {
                a.set(true);
                v_clone.set(0.0);
            }
        });
        v.set(1.0);
        assert!(armed.get());
        assert_eq!(v.get(), 0.0);
    }

    #[test]
    fn dropped_handle_stops_ticking() {
        // Animator that would otherwise run for a long time —
        // dropping all handles should make the tick closure
        // return false (Weak::upgrade fails) and the clock should
        // unregister it.
        let v = AnimatedValue::new(0.0_f32);
        v.animate(TweenTo::new(1.0, Duration::from_secs(60)).linear());
        let before = super::super::clock::registered_count();
        assert!(before >= 1);
        drop(v);
        // One tick to let the closure observe the dead Weak.
        tick_for_test(STEP);
        let after = super::super::clock::registered_count();
        assert!(after < before);
    }
}
