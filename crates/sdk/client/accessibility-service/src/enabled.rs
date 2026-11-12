//! Enabled-state change notifications and the listener registry they are
//! delivered to.
//!
//! Listeners live in a UI-thread thread-local: the platform half reports a
//! bind or unbind on the main thread, and a listener typically writes a
//! signal, which belongs to that thread. The platform half reports changes
//! only while at least one listener is registered.

use std::cell::RefCell;
use std::rc::Rc;

type Listener = Rc<dyn Fn(bool)>;

runtime_tls::thread_local! {
    static LISTENERS: RefCell<(u64, Vec<(u64, Listener)>)> = const { RefCell::new((0, Vec::new())) };
}

/// Call `listener` with the new state whenever the service binds (`true`,
/// switched on) or unbinds (`false`, switched off). It is not called with
/// the current state: read [`is_enabled`](crate::is_enabled) for that.
/// Delivery stops when the returned subscription drops. Call on the UI
/// thread. Off Android the listener is kept and never called.
#[must_use = "notifications stop when the subscription is dropped"]
pub fn on_enabled_change(listener: impl Fn(bool) + 'static) -> EnabledSubscription {
    let (id, first) = LISTENERS.with(|l| {
        let mut l = l.borrow_mut();
        l.0 += 1;
        let id = l.0;
        let first = l.1.is_empty();
        l.1.push((id, Rc::new(listener)));
        (id, first)
    });
    if first {
        set_platform_listening(true);
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
        let now_empty = LISTENERS.with(|l| {
            let mut l = l.borrow_mut();
            let before = l.1.len();
            l.1.retain(|(id, _)| *id != self.id);
            before != l.1.len() && l.1.is_empty()
        });
        if now_empty {
            set_platform_listening(false);
        }
    }
}

/// Deliver a state change to every listener. The list is cloned first, so a
/// listener may register or drop subscriptions while it runs.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn dispatch(enabled: bool) {
    let listeners: Vec<Listener> =
        LISTENERS.with(|l| l.borrow().1.iter().map(|(_, f)| f.clone()).collect());
    for listener in listeners {
        listener(enabled);
    }
}

fn set_platform_listening(on: bool) {
    #[cfg(all(not(target_arch = "wasm32"), target_os = "android"))]
    {
        // A failure here (no JVM context yet) means no change is reported;
        // the next first registration retries.
        let method = if on { "attach" } else { "detach" };
        if let Err(e) = crate::android::call(method, &serde_json::json!({})) {
            log::warn!("accessibility_service: {method} failed: {e}");
        }
    }
    #[cfg(not(all(not(target_arch = "wasm32"), target_os = "android")))]
    let _ = on;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

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
