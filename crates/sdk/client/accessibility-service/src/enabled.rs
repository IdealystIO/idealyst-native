//! Enabled-state change notifications.

use std::cell::RefCell;

use crate::listeners::{deliver, Listeners};

runtime_tls::thread_local! {
    static LISTENERS: RefCell<Listeners<bool>> = const { RefCell::new(Listeners::new()) };
}

/// Call `listener` with the new state whenever the service binds (`true`,
/// switched on) or unbinds (`false`, switched off). It is not called with
/// the current state: read [`is_enabled`](crate::is_enabled) for that.
/// Delivery stops when the returned subscription drops. Call on the UI
/// thread. Off Android the listener is kept and never called.
#[must_use = "notifications stop when the subscription is dropped"]
pub fn on_enabled_change(listener: impl Fn(bool) + 'static) -> EnabledSubscription {
    let (id, first) = LISTENERS.with(|l| l.borrow_mut().add(listener));
    if first {
        crate::platform_listening("enabled", true);
    }
    EnabledSubscription { id }
}

/// Keeps an [`on_enabled_change`] listener registered; dropping it
/// unregisters.
#[derive(Debug)]
pub struct EnabledSubscription {
    id: u64,
}

impl Drop for EnabledSubscription {
    fn drop(&mut self) {
        if LISTENERS.with(|l| l.borrow_mut().remove(self.id)) {
            crate::platform_listening("enabled", false);
        }
    }
}

/// Deliver a state change to every listener.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn dispatch(enabled: bool) {
    deliver(LISTENERS.with(|l| l.borrow().snapshot()), enabled);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn listeners_hear_changes_until_their_subscription_drops() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let a = {
            let seen = seen.clone();
            on_enabled_change(move |on| seen.borrow_mut().push(("a", on)))
        };
        let b = {
            let seen = seen.clone();
            on_enabled_change(move |on| seen.borrow_mut().push(("b", on)))
        };
        dispatch(true);
        drop(b);
        dispatch(false);
        drop(a);
        dispatch(true);
        assert_eq!(*seen.borrow(), vec![("a", true), ("b", true), ("a", false)]);
    }

    #[test]
    fn a_listener_may_unsubscribe_another_while_dispatching() {
        let held: Rc<RefCell<Option<EnabledSubscription>>> = Rc::new(RefCell::new(None));
        let ran = Rc::new(Cell::new(false));
        let first = {
            let held = held.clone();
            on_enabled_change(move |_| {
                held.borrow_mut().take();
            })
        };
        *held.borrow_mut() = Some({
            let ran = ran.clone();
            on_enabled_change(move |_| ran.set(true))
        });
        dispatch(true);
        // The list was snapshotted, so the second listener still ran once.
        assert!(ran.get());
        assert!(held.borrow().is_none());
        drop(first);
    }
}
