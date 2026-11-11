//! Exit holds: a subtree asks the region that owns it to keep it mounted a
//! little longer when it is switched out, so it can animate away.
//!
//! A `Dyn` region (`if` / `match` in `ui!`, `when`, `switch`) tears its old
//! subtree down in the same turn the condition flips. Anything inside that
//! wants to fade out has no frame to do it in — which is why an anchored
//! menu shown with `if open.get() { Menu(…) }` could animate in but only
//! ever vanish.
//!
//! [`hold_on_exit`] is the request. Called while a subtree builds, it
//! registers with the NEAREST enclosing `Dyn` region:
//!
//! - when that region swaps the subtree out, it calls `start` (begin the
//!   exit animation) and leaves the subtree's nodes attached and its scope
//!   alive for `ms` milliseconds — the longest hold any part of the
//!   subtree asked for — then detaches the nodes and drops the scope;
//! - the region's NEW content builds and mounts right away, before the old
//!   nodes in child order. Old and new coexist for the hold;
//! - a subtree that leaves any other way is not held: an outer region
//!   swapping, a keyed row going, the whole mount dropping. A hold is for
//!   the one transition the subtree's own condition makes.
//!
//! A held subtree is still live. Its effects run and its handlers fire, so
//! a component that should be inert while it leaves makes itself inert in
//! `start`.
//!
//! # The timer seam
//!
//! The scene has no clock. The layer above installs one with
//! [`install_exit_defer`] (the vocabulary does, at boot); it returns a
//! guard whose drop cancels the pending call. With none installed a hold
//! is not honoured: `start` does not run and teardown is immediate, as if
//! nothing had asked.
//!
//! A region that is destroyed while it holds subtrees drops their guards,
//! which cancels the timers and frees the held scopes at once; the nodes
//! go with whatever is removing the region's parent.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Schedules `f` after `ms` milliseconds. The returned guard cancels the
/// call when dropped. It is only dropped from OUTSIDE `f` (never while `f`
/// runs), so a timer handle that must not be freed inside its own callback
/// is safe to return.
pub type ExitDefer = Rc<dyn Fn(u32, Box<dyn FnOnce()>) -> Box<dyn Any>>;

/// One subtree's request, held by the region it registered with.
pub(crate) struct ExitRequest {
    ms: u32,
    /// Cleared when the scope that asked is dropped: a part of the subtree
    /// that already left (an inner branch, a keyed row) must not be told
    /// to start an exit on state that no longer exists.
    alive: Rc<Cell<bool>>,
    start: Box<dyn FnOnce()>,
}

runtime_tls::thread_local! {
    /// One frame per `Dyn` region currently building its content;
    /// innermost last.
    static COLLECTING: RefCell<Vec<Vec<ExitRequest>>> = const { RefCell::new(Vec::new()) };
    static DEFER: RefCell<Option<ExitDefer>> = const { RefCell::new(None) };
}

/// Install this thread's timer for exit holds (see the module docs).
/// Replaces any earlier one.
pub fn install_exit_defer(defer: impl Fn(u32, Box<dyn FnOnce()>) -> Box<dyn Any> + 'static) {
    DEFER.with(|d| *d.borrow_mut() = Some(Rc::new(defer)));
}

/// Ask the nearest enclosing `Dyn` region to keep this subtree mounted for
/// `ms` milliseconds after it is switched out, calling `start` at that
/// moment. Call it while the subtree builds (a component body). Outside
/// any region's build it does nothing: there is no region to hold it.
///
/// See the module docs for exactly which removals are held.
pub fn hold_on_exit(ms: u32, start: impl FnOnce() + 'static) {
    let alive = Rc::new(Cell::new(true));
    let registered = COLLECTING.with(|c| match c.borrow_mut().last_mut() {
        Some(frame) => {
            frame.push(ExitRequest { ms, alive: alive.clone(), start: Box::new(start) });
            true
        }
        None => false,
    });
    if registered {
        runtime_world::on_scope_drop(move || alive.set(false));
    }
}

/// Pops the collecting frame even if the build unwinds.
struct Frame;

impl Drop for Frame {
    fn drop(&mut self) {
        let _ = COLLECTING.try_with(|c| {
            if let Ok(mut c) = c.try_borrow_mut() {
                c.pop();
            }
        });
    }
}

/// Run a region's content build, collecting the exit holds requested
/// inside it (and not inside a nested region, which collects its own).
pub(crate) fn collecting<R>(build: impl FnOnce() -> R) -> (R, Vec<ExitRequest>) {
    COLLECTING.with(|c| c.borrow_mut().push(Vec::new()));
    let frame = Frame;
    let out = build();
    let requests = COLLECTING.with(|c| c.borrow_mut().pop()).unwrap_or_default();
    std::mem::forget(frame);
    (out, requests)
}

/// A swap is about to remove the subtree that made `requests`. If any of
/// them is still live and a timer is installed, start every exit and
/// return how long to hold, with the timer to hold it on. `None` means
/// tear down now.
pub(crate) fn begin(requests: Vec<ExitRequest>) -> Option<(u32, ExitDefer)> {
    if requests.is_empty() {
        return None;
    }
    let defer = DEFER.with(|d| d.borrow().clone())?;
    let live: Vec<ExitRequest> = requests.into_iter().filter(|r| r.alive.get()).collect();
    let ms = live.iter().map(|r| r.ms).max()?;
    // Untracked: this runs inside the region's driver effect, and a signal
    // an exit animation reads must not become a reason to swap the region.
    runtime_world::untrack(|| {
        for request in live {
            (request.start)();
        }
    });
    Some((ms, defer))
}

/// The subtrees a region is holding: each entry's guard keeps its timer
/// (and through it the held scope) alive; `done` marks one whose timer has
/// fired so it can be dropped from outside the callback.
#[derive(Default)]
pub(crate) struct Held {
    entries: Vec<(Rc<Cell<bool>>, Box<dyn Any>)>,
}

impl Held {
    /// Whether any held subtree's nodes are still attached.
    pub(crate) fn any(&self) -> bool {
        self.entries.iter().any(|(done, _)| !done.get())
    }

    /// Hold a subtree for `ms`, then run `teardown`.
    pub(crate) fn hold(&mut self, ms: u32, defer: &ExitDefer, teardown: impl FnOnce() + 'static) {
        // Spent guards are dropped here, outside any timer callback.
        self.entries.retain(|(done, _)| !done.get());
        let done = Rc::new(Cell::new(false));
        let mark = done.clone();
        let guard = defer(
            ms,
            Box::new(move || {
                teardown();
                mark.set(true);
            }),
        );
        self.entries.push((done, guard));
    }
}
