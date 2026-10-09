//! [`AnimationClock`]: elapsed playback time as a signal, advanced every
//! display frame while playing.

use runtime_core::animation::clock::{register_guarded, TickRegistration};
use runtime_core::{signal, ScopeAlive, Signal};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

/// Playback time for animations: a reactive number of seconds that advances
/// with the display while playing.
///
/// Read [`time`](AnimationClock::time) inside a `Canvas3d` painter and the
/// view re-renders every frame while the clock plays, and not at all while
/// it's paused. The clock rides the framework's animation clock (the same
/// per-frame tick that drives animated styles: a display link on iOS and
/// macOS, `requestAnimationFrame` on the web), which only runs while
/// something is registered — so a paused or never-started clock costs
/// nothing per frame.
///
/// Create it in a component body; it stops when that scope is torn down.
/// It's `Copy`, like a signal, so `ui!` branches and handlers capture it
/// without cloning.
///
/// ```ignore
/// let clock = AnimationClock::new();
/// clock.play();
/// // in the painter:
/// s.model(&fox, xf).animation(&walk, walk.looped(clock.time()));
/// ```
#[derive(Clone, Copy)]
pub struct AnimationClock {
    id: u64,
    time: Signal<f32>,
    playing: Signal<bool>,
}

/// The non-reactive half of a clock, kept off the handle so the handle can
/// be `Copy`. Removed when the clock's scope is torn down, which drops the
/// tick registration with it.
struct Playback {
    speed: f32,
    /// Bumped whenever playback stops or restarts. A tick closure remembers
    /// the generation it was registered under and retires itself when that's
    /// stale: the framework clock takes a closure out of its registry while
    /// running it and puts it back afterwards, so a closure can outlive the
    /// unregister of its own registration and must not keep running.
    generation: u64,
    tick: Option<TickRegistration>,
    alive: ScopeAlive,
}

thread_local! {
    static PLAYBACK: RefCell<HashMap<u64, Playback>> = RefCell::new(HashMap::new());
    static NEXT_ID: Cell<u64> = const { Cell::new(1) };
}

fn with_playback<R>(id: u64, f: impl FnOnce(&mut Playback) -> R) -> Option<R> {
    // `try_with`: a clock can be touched during thread-local teardown.
    PLAYBACK.try_with(|m| m.borrow_mut().get_mut(&id).map(f)).ok().flatten()
}

impl AnimationClock {
    /// A paused clock at 0 seconds, speed 1.
    pub fn new() -> AnimationClock {
        let id = NEXT_ID.with(|n| {
            let id = n.get();
            n.set(id + 1);
            id
        });
        PLAYBACK.with(|m| {
            m.borrow_mut().insert(
                id,
                Playback { speed: 1.0, generation: 0, tick: None, alive: ScopeAlive::current() },
            )
        });
        runtime_core::on_owned_drop(move || {
            // Take the entry out before dropping it: dropping the
            // registration re-enters the framework clock, never this map.
            let gone = PLAYBACK.try_with(|m| m.borrow_mut().remove(&id)).ok().flatten();
            drop(gone);
        });
        AnimationClock { id, time: signal(0.0), playing: signal(false) }
    }

    /// Start (or resume) advancing. No-op while already playing.
    pub fn play(&self) {
        let id = self.id;
        let time = self.time;
        let Some(Some(generation)) = with_playback(id, |p| {
            if p.tick.is_some() {
                return None;
            }
            p.generation += 1;
            Some(p.generation)
        }) else {
            return;
        };
        self.playing.set(true);
        let registration = register_guarded(Box::new(move |dt| {
            let step = with_playback(id, |p| {
                (p.generation == generation && p.alive.get()).then(|| dt.as_secs_f32() * p.speed)
            });
            let Some(Some(step)) = step else { return false };
            if step != 0.0 {
                time.update(|t| (t + step).max(0.0));
            }
            true
        }));
        // Registering may have started the frame loop, but nothing ran yet:
        // the entry is still ours to fill.
        let stale = with_playback(id, |p| p.tick.replace(registration));
        drop(stale);
    }

    /// Stop advancing; the time stays where it is.
    pub fn pause(&self) {
        let registration = with_playback(self.id, |p| {
            p.generation += 1;
            p.tick.take()
        });
        drop(registration);
        self.playing.set(false);
    }

    /// Jump to `seconds` (clamped at 0). Works while playing or paused.
    pub fn seek(&self, seconds: f32) {
        self.time.set(seconds.max(0.0));
    }

    /// Playback rate: 1 is real time, 0.5 half speed, negative runs backwards
    /// (stopping at 0).
    pub fn set_speed(&self, speed: f32) {
        let speed = if speed.is_finite() { speed } else { 1.0 };
        with_playback(self.id, |p| p.speed = speed);
    }

    pub fn speed(&self) -> f32 {
        with_playback(self.id, |p| p.speed).unwrap_or(0.0)
    }

    /// Elapsed seconds — a reactive read.
    pub fn time(&self) -> f32 {
        self.time.get()
    }

    /// Whether the clock is advancing — a reactive read.
    pub fn is_playing(&self) -> bool {
        self.playing.get()
    }
}

impl Default for AnimationClock {
    fn default() -> Self {
        AnimationClock::new()
    }
}
