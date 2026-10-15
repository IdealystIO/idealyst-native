//! `ViewHandle::on_layout` for the Windows backend — the analog of the
//! web's `ResizeObserver` and the Apple backends' `layout_subs`.
//!
//! A component that sizes its own content from its laid-out box (charts
//! size their plot this way, overlays anchor to `rect()`, container
//! queries restyle on it) subscribes through its view handle. Without
//! this the handle's `subscribe_layout` was the trait's no-op and
//! `rect()` the zero rect, so on Windows every chart drew nothing.
//!
//! Semantics match the web's: a subscriber hears the view's current size
//! once right after subscribing, then again whenever a layout pass leaves
//! the view at a different size. Sizes are logical pixels.
//!
//! Callbacks are author code that writes signals and may read handles
//! back, so they never run inside a layout pass (which holds the backend
//! borrow): the pass collects what is due ([`due`]) and the backend runs
//! it on a fresh scheduler turn ([`fire`]), followed by the dispatch hook
//! so the staged writes commit (the same contract as
//! `backend_apple_core::layout_subs::fire`).
//!
//! Main-thread only, like the backend itself, so a thread-local is sound.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use runtime_shared::LayoutSubscription;

type Callback = Rc<dyn Fn(f32, f32)>;

struct Sub {
    node: u64,
    id: u64,
    callback: Callback,
    /// The size this subscriber was last told; `None` before its first.
    told: Option<(f32, f32)>,
}

thread_local! {
    static SUBS: RefCell<Vec<Sub>> = const { RefCell::new(Vec::new()) };
    static NEXT: Cell<u64> = const { Cell::new(1) };
}

/// Register `callback` for `node`. Dropping the subscription removes it.
pub(crate) fn subscribe(node: u64, callback: Box<dyn Fn(f32, f32)>) -> LayoutSubscription {
    let id = NEXT.with(|n| {
        let id = n.get();
        n.set(id + 1);
        id
    });
    SUBS.with(|s| s.borrow_mut().push(Sub { node, id, callback: Rc::from(callback), told: None }));
    LayoutSubscription::new(move || {
        SUBS.with(|s| s.borrow_mut().retain(|sub| sub.id != id));
    })
}

/// Whether any view is subscribed (a layout pass skips the work if not).
pub(crate) fn any() -> bool {
    SUBS.with(|s| !s.borrow().is_empty())
}

/// Sizes owed: every subscriber whose node `size_of` reports at a size it
/// has not been told yet, by subscription id. Marks them told.
pub(crate) type Due = Vec<(u64, (f32, f32))>;

pub(crate) fn due(size_of: impl Fn(u64) -> Option<(f32, f32)>) -> Due {
    SUBS.with(|s| {
        let mut out = Vec::new();
        for sub in s.borrow_mut().iter_mut() {
            let Some(size) = size_of(sub.node) else { continue };
            if sub.told != Some(size) {
                sub.told = Some(size);
                out.push((sub.id, size));
            }
        }
        out
    })
}

/// Deliver `due` sizes to the subscriptions that still exist, then fire
/// the dispatch hook if any ran.
///
/// The callbacks are looked up NOW, not when the sizes were collected: a
/// size collected in a layout pass is delivered on a later turn, and a
/// component unmounted in between (a chart swapping kinds) has dropped its
/// subscription and freed the signals its callback writes — calling it
/// anyway aborted on a stale signal handle.
pub(crate) fn fire(due: Due) {
    let callbacks: Vec<(Callback, (f32, f32))> = SUBS.with(|s| {
        let subs = s.borrow();
        due.into_iter()
            .filter_map(|(id, size)| subs.iter().find(|sub| sub.id == id).map(|sub| (sub.callback.clone(), size)))
            .collect()
    });
    if callbacks.is_empty() {
        return;
    }
    for (callback, (w, h)) in callbacks {
        callback(w, h);
    }
    crate::dispatch_hook::fire_dispatch_hook();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A size collected by a layout pass was delivered on a later turn even
    /// if the subscriber had unmounted in between; its callback wrote freed
    /// signals and the app aborted (the charts demo, switching chart kind).
    #[test]
    fn regression_a_size_owed_to_a_dropped_subscription_is_not_delivered() {
        let called = Rc::new(Cell::new(false));
        let c = called.clone();
        let sub = subscribe(77, Box::new(move |_, _| c.set(true)));
        let owed = due(|id| (id == 77).then_some((10.0, 10.0)));
        drop(sub);
        fire(owed);
        assert!(!called.get());
    }

    #[test]
    fn a_subscriber_hears_its_size_once_then_only_changes() {
        let seen: Rc<RefCell<Vec<(f32, f32)>>> = Default::default();
        let s = seen.clone();
        let sub = subscribe(42, Box::new(move |w, h| s.borrow_mut().push((w, h))));
        let size = Cell::new((100.0, 40.0));
        let pass = || fire(due(|id| (id == 42).then(|| size.get())));
        pass();
        pass();
        size.set((120.0, 40.0));
        pass();
        assert_eq!(*seen.borrow(), [(100.0, 40.0), (120.0, 40.0)]);
        drop(sub);
        size.set((10.0, 10.0));
        pass();
        assert_eq!(seen.borrow().len(), 2, "a dropped subscription hears nothing");
    }
}
