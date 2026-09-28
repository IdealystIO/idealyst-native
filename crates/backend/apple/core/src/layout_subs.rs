//! Per-view `on_layout` subscriber registry shared by the UIKit and
//! AppKit backends — the Apple analog of the web `ResizeObserver`.
//!
//! `ViewOps::subscribe_layout` registers a callback against a view key
//! (the view pointer, the same `usize` both backends key
//! `view_to_layout` by); each backend's frame-apply pass calls
//! [`fire`] with the view's resolved size.
//!
//! # Why [`fire`] fires the dispatch hook (FRAMEWORK-NOTES #103)
//!
//! The layout pass is an author-code entry point: `on_layout` bodies
//! write signals (`natural_height.set(h)`), and the new core only
//! STAGES writes until the flush driver commits them. The pass runs
//! from surfaces nothing else hooks — the coalesced layout microtask
//! (deliberately un-hooked, see [`crate::dispatch_hook`]), a window
//! resize, a navigator's synchronous pass — so without a flush here
//! the measured value sat uncommitted until some unrelated event
//! flushed. Web had the identical omission in its ResizeObserver
//! closure; all three now schedule the flush the same way.
//!
//! Shared (instead of one copy per backend, as before) so the two Apple
//! backends cannot drift, and NOT OS-gated — pure `std` state, so the
//! fire-then-flush contract has a host-run regression test.
//! Main-thread only on both platforms, so a thread-local is sound.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_shared::LayoutSubscription;

type LayoutCallback = Rc<dyn Fn(f32, f32)>;

thread_local! {
    static LAYOUT_SUBS: RefCell<Vec<(usize, LayoutCallback)>> =
        const { RefCell::new(Vec::new()) };
}

/// Register `callback` for the view keyed `view_key`. Dropping the
/// returned subscription removes exactly this callback (matched by key
/// + `Rc` identity), so a container unmount tears it down.
pub fn subscribe(view_key: usize, callback: Box<dyn Fn(f32, f32)>) -> LayoutSubscription {
    let cb: LayoutCallback = Rc::from(callback);
    let cb_id = Rc::as_ptr(&cb) as *const () as usize;
    LAYOUT_SUBS.with(|m| m.borrow_mut().push((view_key, cb)));
    LayoutSubscription::new(move || {
        LAYOUT_SUBS.with(|m| {
            m.borrow_mut().retain(|(k, c)| {
                !(*k == view_key && Rc::as_ptr(c) as *const () as usize == cb_id)
            })
        });
    })
}

/// Fire every callback registered for `view_key` with the view's
/// resolved inline-size (`w`) and block-size (`h`), then — if any ran —
/// fire the post-dispatch hook so their staged writes commit (module
/// docs). The callbacks are snapshotted first so one may subscribe /
/// unsubscribe re-entrantly. Author callbacks change-guard, so a
/// re-fire at an unchanged size is a no-op and the flush it schedules
/// commits nothing — the container-query restyle→relayout loop stays
/// convergent.
pub fn fire(view_key: usize, w: f32, h: f32) {
    let cbs: Vec<LayoutCallback> = LAYOUT_SUBS.with(|m| {
        m.borrow()
            .iter()
            .filter(|(k, _)| *k == view_key)
            .map(|(_, c)| c.clone())
            .collect()
    });
    if cbs.is_empty() {
        return;
    }
    for c in cbs {
        c(w, h);
    }
    crate::dispatch_hook::fire_dispatch_hook();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch_hook::{clear_dispatch_hook, install_dispatch_hook};
    use std::cell::Cell;

    thread_local! {
        static FLUSHES: Cell<u32> = const { Cell::new(0) };
    }

    fn count_flush() {
        FLUSHES.with(|f| f.set(f.get() + 1));
    }

    /// Regression (FRAMEWORK-NOTES #103): an `on_layout` callback fired
    /// by the frame-apply pass must be followed by the post-dispatch
    /// hook (the new-core flush driver), or the signal it wrote stays
    /// staged until an unrelated event flushes. Fails on the old
    /// per-backend `fire_layout_for_view`, which ran the callbacks and
    /// returned.
    #[test]
    fn regression_on_layout_fire_schedules_flush() {
        clear_dispatch_hook();
        FLUSHES.with(|f| f.set(0));
        install_dispatch_hook(count_flush);

        let seen = Rc::new(Cell::new((0.0f32, 0.0f32)));
        let s = seen.clone();
        let sub = subscribe(0xA11, Box::new(move |w, h| s.set((w, h))));

        fire(0xA11, 120.0, 37.0);
        assert_eq!(seen.get(), (120.0, 37.0), "callback received the size");
        assert_eq!(FLUSHES.with(|f| f.get()), 1, "a fired callback schedules one flush");

        // A view with no subscribers (the overwhelmingly common case in a
        // layout pass) must not schedule anything.
        fire(0xB22, 1.0, 1.0);
        assert_eq!(FLUSHES.with(|f| f.get()), 1, "no subscribers → no flush");

        // Dropping the subscription unregisters it.
        drop(sub);
        fire(0xA11, 5.0, 5.0);
        assert_eq!(seen.get(), (120.0, 37.0), "dropped subscription no longer fires");
        assert_eq!(FLUSHES.with(|f| f.get()), 1, "…and schedules no flush");

        clear_dispatch_hook();
    }
}
