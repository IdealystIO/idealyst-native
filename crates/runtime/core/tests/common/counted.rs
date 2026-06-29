//! Fire-counter wrappers around reactive primitives.
//!
//! Reactive bugs typically show up as either too-many-fires (phantom
//! re-runs) or too-few-fires (missed subscriptions). Asserting on the
//! final value catches neither. These helpers wrap a closure in a
//! counter so tests can assert on the exact fire count.
//!
//! ```ignore
//! let s = signal!(0);
//! let (count, _effect) = counted_effect(move || { let _ = s.get(); });
//! s.set(1);
//! s.set(2);
//! assert_eq!(count.get(), 3); // initial + 2 changes
//! ```

#![allow(dead_code)]

use std::cell::Cell;
use std::rc::Rc;

use runtime_core::{memo, watch, Subscription};

/// A shared counter readable from outside the reactive closure.
#[derive(Clone, Default)]
pub struct FireCounter {
    inner: Rc<Cell<usize>>,
}

impl FireCounter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self) -> usize {
        self.inner.get()
    }

    pub fn reset(&self) {
        self.inner.set(0);
    }

    fn bump(&self) {
        self.inner.set(self.inner.get() + 1);
    }
}

/// Wrap an effect body in a fire counter. Returns the counter (shared
/// reference) and the `Subscription` handle. The handle's lifetime should
/// be kept around for the duration of the test — usually with
/// `let (_count, _e) = counted_effect(...);` — so the effect isn't
/// dropped prematurely.
pub fn counted_effect<F>(body: F) -> (FireCounter, Subscription)
where
    F: Fn() + 'static,
{
    let counter = FireCounter::new();
    let c = counter.clone();
    let sub = watch(move || {
        c.bump();
        body();
    });
    (counter, sub)
}

/// Wrap a memo's compute closure in a fire counter. Returns the
/// counter and the `Signal<T>` produced by `memo()`.
///
/// Note: the counter increments on every memo recomputation, NOT on
/// every read of the memoized value. Readers subscribe to the cache;
/// recomputation happens only when a tracked input changes.
pub fn counted_memo<T, F>(compute: F) -> (FireCounter, runtime_core::Signal<T>)
where
    T: Clone + PartialEq + 'static,
    F: Fn() -> T + 'static,
{
    let counter = FireCounter::new();
    let c = counter.clone();
    let r = memo(move || {
        c.bump();
        compute()
    });
    (counter, r)
}
