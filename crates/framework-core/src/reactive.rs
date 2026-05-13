//! Single-threaded fine-grained reactivity.
//!
//! Spike scope:
//! - `Signal<T>` holds a value and a subscriber list.
//! - `Effect` runs a closure with a thread-local "current effect" set;
//!   any `Signal::get()` during that run registers a subscription.
//! - `Signal::set()` re-runs every live subscriber.
//!
//! All UI work happens on a single thread on every target we care about,
//! so we use `Rc`/`RefCell` + thread-locals rather than `Arc`/`Mutex`.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

thread_local! {
    static CURRENT: RefCell<Option<Weak<EffectInner>>> = const { RefCell::new(None) };
}

struct EffectInner {
    run: RefCell<Box<dyn FnMut()>>,
}

fn run_effect(inner: &Rc<EffectInner>) {
    let weak = Rc::downgrade(inner);
    let prev = CURRENT.with(|c| c.replace(Some(weak)));
    (inner.run.borrow_mut())();
    CURRENT.with(|c| *c.borrow_mut() = prev);
}

/// Runs `f` with subscription tracking disabled. Any `Signal::get()` calls
/// inside `f` will return their current value without subscribing the
/// enclosing effect. Use this when you need to read a signal during effect
/// setup without taking a dependency on it (for example, when constructing
/// a child subtree whose own effects will re-subscribe independently).
pub fn untrack<R, F: FnOnce() -> R>(f: F) -> R {
    let prev = CURRENT.with(|c| c.borrow_mut().take());
    let result = f();
    CURRENT.with(|c| *c.borrow_mut() = prev);
    result
}

/// Handle to a reactive effect. Drop it to stop the effect from re-running.
pub struct Effect {
    _inner: Rc<EffectInner>,
}

impl Effect {
    /// Creates an effect and runs it once. Any signals read during the run
    /// will fire the effect again when they change.
    pub fn new<F: FnMut() + 'static>(f: F) -> Self {
        let inner = Rc::new(EffectInner {
            run: RefCell::new(Box::new(f)),
        });
        run_effect(&inner);
        Effect { _inner: inner }
    }
}

struct SignalInner<T> {
    value: RefCell<T>,
    subscribers: RefCell<Vec<Weak<EffectInner>>>,
}

/// Reactive value. `get()` reads and subscribes the current effect (if any);
/// `set()` writes and re-runs subscribed effects.
pub struct Signal<T> {
    inner: Rc<SignalInner<T>>,
}

impl<T> Clone for Signal<T> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

impl<T: Clone + 'static> Signal<T> {
    pub fn new(value: T) -> Self {
        Self {
            inner: Rc::new(SignalInner {
                value: RefCell::new(value),
                subscribers: RefCell::new(Vec::new()),
            }),
        }
    }

    pub fn get(&self) -> T {
        CURRENT.with(|c| {
            if let Some(weak) = c.borrow().as_ref() {
                let mut subs = self.inner.subscribers.borrow_mut();
                if !subs.iter().any(|w| w.ptr_eq(weak)) {
                    subs.push(weak.clone());
                }
            }
        });
        self.inner.value.borrow().clone()
    }

    pub fn set(&self, value: T) {
        *self.inner.value.borrow_mut() = value;
        self.notify();
    }

    pub fn update<F: FnOnce(&mut T)>(&self, f: F) {
        f(&mut self.inner.value.borrow_mut());
        self.notify();
    }

    fn notify(&self) {
        let snapshot: Vec<Weak<EffectInner>> = self.inner.subscribers.borrow().clone();
        let mut still_alive = Vec::with_capacity(snapshot.len());
        for weak in snapshot {
            if let Some(strong) = weak.upgrade() {
                still_alive.push(Rc::downgrade(&strong));
                run_effect(&strong);
            }
        }
        *self.inner.subscribers.borrow_mut() = still_alive;
    }
}
