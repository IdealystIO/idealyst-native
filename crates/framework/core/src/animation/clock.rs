//! Per-thread animation tick registry.
//!
//! Every [`AnimatedValue`](crate::animation::AnimatedValue) registers
//! a tick closure with the clock. Once any closure is registered the
//! clock asks the installed
//! [`Scheduler`](crate::scheduling::Scheduler) for a `raf_loop`; on
//! every frame the clock walks its registered closures, hands each
//! one the elapsed wall-clock slice, and unregisters those that
//! report done. When the registry drains the `raf_loop` is dropped
//! and the clock goes back to costing zero per frame.
//!
//! # Threading
//!
//! Single-threaded by design — see the project rationale on why
//! animation doesn't benefit from off-thread work in this
//! architecture. The clock lives in a `thread_local!`; closures
//! capture `Rc<RefCell<…>>` state same as the rest of the
//! framework's reactive surface.
//!
//! # Tests / no-scheduler environments
//!
//! [`tick_for_test`] synchronously drives the registered closures
//! with an author-supplied slice, bypassing the scheduler. Tests
//! use this; nothing else should.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;

use crate::scheduling::{raf_loop, RafLoop};
use crate::time::now_micros;

/// Per-frame closure signature. Returns `true` while the closure
/// wants more ticks, `false` when it's done and can be dropped.
pub type TickFn = Box<dyn FnMut(Duration) -> bool + 'static>;

/// Identifier handed out by [`register`]. Opaque; passed back to
/// [`unregister`].
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct TickId(u64);

/// Maximum slice the clock will hand to a tick closure.
///
/// Echoes [`MAX_FRAME_DT`](crate::animation::MAX_FRAME_DT) but is
/// applied here too as a safety net so that closures *not* derived
/// from `Animator::sample` (custom raf consumers) also benefit from
/// the cap. Animator implementations also clamp internally —
/// double-clamping is cheap and the floor is the same.
const MAX_TICK_DT: Duration = Duration::from_millis(64);

struct ClockState {
    ticks: HashMap<TickId, TickFn>,
    next_id: u64,
    raf_handle: Option<RafLoop>,
    /// Wall-clock reading at the moment we last drove a tick.
    /// `None` between scheduler pauses so the first tick of a new
    /// run produces `dt = 0` rather than a huge gap measured
    /// across whatever the clock was doing before.
    last_tick_micros: Option<u64>,
}

impl ClockState {
    fn new() -> Self {
        Self {
            ticks: HashMap::new(),
            next_id: 0,
            raf_handle: None,
            last_tick_micros: None,
        }
    }
}

thread_local! {
    static CLOCK: RefCell<ClockState> = RefCell::new(ClockState::new());
}

/// Register a tick closure. Returns the id to pass back to
/// [`unregister`] (or hold inside a
/// [`TickRegistration`] guard, which calls
/// `unregister` on drop).
///
/// If this is the first live tick on the calling thread, the clock
/// also installs a `raf_loop` via the
/// [`Scheduler`](crate::scheduling::Scheduler). If no scheduler is
/// installed (native pre-init or wasm32 pre-`install_scheduler`)
/// the `raf_loop` returns an inert handle — tick closures never
/// fire — and the only way to drive them is [`tick_for_test`].
pub fn register(f: TickFn) -> TickId {
    let id = CLOCK.with(|c| {
        let mut c = c.borrow_mut();
        let id = TickId(c.next_id);
        c.next_id += 1;
        c.ticks.insert(id, f);
        id
    });
    ensure_loop_running();
    id
}

/// Stop ticking the closure under `id`. If this was the last
/// registered tick, the clock also drops its `raf_loop` handle,
/// stopping the per-frame work entirely.
pub fn unregister(id: TickId) {
    CLOCK.with(|c| {
        let mut c = c.borrow_mut();
        c.ticks.remove(&id);
        if c.ticks.is_empty() {
            c.raf_handle = None;
            c.last_tick_micros = None;
        }
    });
}

/// RAII registration. `register_guarded(f)` is equivalent to
/// `register(f)` but the returned guard unregisters on `Drop`. Use
/// when the lifetime of the tick is tied to a value handle.
pub struct TickRegistration {
    id: Option<TickId>,
}

impl TickRegistration {
    pub fn id(&self) -> Option<TickId> {
        self.id
    }
}

impl Drop for TickRegistration {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            unregister(id);
        }
    }
}

/// Like [`register`] but returns a guard that unregisters on drop.
pub fn register_guarded(f: TickFn) -> TickRegistration {
    TickRegistration {
        id: Some(register(f)),
    }
}

/// Synchronously drive all registered tick closures with the
/// supplied slice. **Intended only for tests** — production code
/// relies on the scheduler-driven loop.
///
/// Returns the number of closures that survived this tick.
pub fn tick_for_test(dt: Duration) -> usize {
    drive_one_tick(dt.min(MAX_TICK_DT))
}

fn ensure_loop_running() {
    let needs_install = CLOCK.with(|c| {
        let c = c.borrow();
        c.raf_handle.is_none() && !c.ticks.is_empty()
    });
    if !needs_install {
        return;
    }
    let handle = raf_loop(|| {
        let dt = read_dt();
        drive_one_tick(dt.min(MAX_TICK_DT));
    });
    CLOCK.with(|c| {
        c.borrow_mut().raf_handle = Some(handle);
    });
}

fn read_dt() -> Duration {
    let now = now_micros();
    CLOCK.with(|c| {
        let mut c = c.borrow_mut();
        let dt = match c.last_tick_micros {
            Some(last) => Duration::from_micros(now.saturating_sub(last)),
            None => Duration::ZERO,
        };
        c.last_tick_micros = Some(now);
        dt
    })
}

/// Drive registered closures once. Returns the count of survivors
/// (used by [`tick_for_test`]). Closures that return `false` are
/// dropped; if the registry empties we also drop the raf handle.
fn drive_one_tick(dt: Duration) -> usize {
    // Collect ids to tick under one borrow, then re-borrow per
    // closure so callbacks that themselves register or unregister
    // ticks don't trip the RefCell.
    let ids: Vec<TickId> = CLOCK.with(|c| c.borrow().ticks.keys().copied().collect());

    let mut to_drop: Vec<TickId> = Vec::new();
    for id in ids {
        let mut taken = CLOCK.with(|c| c.borrow_mut().ticks.remove(&id));
        if let Some(mut f) = taken.take() {
            let keep = f(dt);
            if keep {
                // Put it back unless something else dropped it
                // meanwhile (e.g., the closure body unregistered
                // its own id and another tick recycled the slot —
                // unlikely but cheap to guard).
                CLOCK.with(|c| {
                    let mut c = c.borrow_mut();
                    c.ticks.entry(id).or_insert(f);
                });
            } else {
                to_drop.push(id);
                drop(f);
            }
        }
    }

    let remaining = CLOCK.with(|c| {
        let mut c = c.borrow_mut();
        for id in &to_drop {
            c.ticks.remove(id);
        }
        if c.ticks.is_empty() {
            c.raf_handle = None;
            c.last_tick_micros = None;
        }
        c.ticks.len()
    });
    remaining
}

/// Test-only: number of currently-registered tick closures.
#[doc(hidden)]
pub fn registered_count() -> usize {
    CLOCK.with(|c| c.borrow().ticks.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    /// Ensure each test starts with an empty registry — tests run
    /// in the same thread and a leftover tick from a prior test
    /// would skew counts.
    fn reset() {
        CLOCK.with(|c| {
            let mut c = c.borrow_mut();
            c.ticks.clear();
            c.raf_handle = None;
            c.last_tick_micros = None;
        });
    }

    #[test]
    fn register_and_unregister_round_trip() {
        reset();
        let id = register(Box::new(|_| true));
        assert_eq!(registered_count(), 1);
        unregister(id);
        assert_eq!(registered_count(), 0);
    }

    #[test]
    fn tick_calls_closures_with_slice() {
        reset();
        let received = Rc::new(Cell::new(Duration::ZERO));
        let r = received.clone();
        let id = register(Box::new(move |dt| {
            r.set(dt);
            true
        }));
        tick_for_test(Duration::from_millis(16));
        assert_eq!(received.get(), Duration::from_millis(16));
        unregister(id);
    }

    #[test]
    fn closures_returning_false_get_dropped() {
        reset();
        let _id = register(Box::new(|_| false));
        assert_eq!(registered_count(), 1);
        tick_for_test(Duration::from_millis(16));
        assert_eq!(registered_count(), 0);
    }

    #[test]
    fn tick_with_oversized_slice_is_clamped() {
        reset();
        let received = Rc::new(Cell::new(Duration::ZERO));
        let r = received.clone();
        let id = register(Box::new(move |dt| {
            r.set(dt);
            true
        }));
        tick_for_test(Duration::from_secs(5));
        assert!(received.get() <= MAX_TICK_DT);
        unregister(id);
    }

    #[test]
    fn multiple_closures_all_get_ticked() {
        reset();
        let a = Rc::new(Cell::new(0u32));
        let b = Rc::new(Cell::new(0u32));
        let aa = a.clone();
        let bb = b.clone();
        let id_a = register(Box::new(move |_| {
            aa.set(aa.get() + 1);
            true
        }));
        let id_b = register(Box::new(move |_| {
            bb.set(bb.get() + 1);
            true
        }));
        tick_for_test(Duration::from_millis(16));
        assert_eq!(a.get(), 1);
        assert_eq!(b.get(), 1);
        unregister(id_a);
        unregister(id_b);
    }

    #[test]
    fn guard_unregisters_on_drop() {
        reset();
        {
            let _g = register_guarded(Box::new(|_| true));
            assert_eq!(registered_count(), 1);
        }
        assert_eq!(registered_count(), 0);
    }
}
