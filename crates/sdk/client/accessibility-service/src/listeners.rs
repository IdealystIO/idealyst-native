//! The listener list behind [`on_enabled_change`](crate::on_enabled_change)
//! and [`on_window_change`](crate::on_window_change).
//!
//! Each list lives in a UI-thread thread-local of its own module: the
//! platform half reports on the main thread, and a listener typically writes
//! a signal, which belongs to that thread. The platform half reports only
//! while a list is non-empty, so [`add`](Listeners::add) and
//! [`remove`](Listeners::remove) say when it went from or to empty.

use std::rc::Rc;

pub(crate) struct Listeners<T> {
    next_id: u64,
    list: Vec<(u64, Rc<dyn Fn(T)>)>,
}

impl<T: Clone> Listeners<T> {
    pub(crate) const fn new() -> Self {
        Self {
            next_id: 0,
            list: Vec::new(),
        }
    }

    /// Register `f`; `(id, was_empty)`.
    pub(crate) fn add(&mut self, f: impl Fn(T) + 'static) -> (u64, bool) {
        self.next_id += 1;
        let was_empty = self.list.is_empty();
        self.list.push((self.next_id, Rc::new(f)));
        (self.next_id, was_empty)
    }

    /// Unregister `id`; `true` when that emptied the list.
    pub(crate) fn remove(&mut self, id: u64) -> bool {
        let before = self.list.len();
        self.list.retain(|(i, _)| *i != id);
        before != self.list.len() && self.list.is_empty()
    }

    /// The listeners right now. Dispatch calls a snapshot, so a listener may
    /// register or drop subscriptions while it runs.
    pub(crate) fn snapshot(&self) -> Vec<Rc<dyn Fn(T)>> {
        self.list.iter().map(|(_, f)| f.clone()).collect()
    }
}

/// Call every listener in `snapshot` with `value`.
pub(crate) fn deliver<T: Clone>(snapshot: Vec<Rc<dyn Fn(T)>>, value: T) {
    for listener in snapshot {
        listener(value.clone());
    }
}
