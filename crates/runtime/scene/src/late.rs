//! The late-registration mailbox — how code with no registry in hand
//! installs a handler after boot.
//!
//! # The problem
//!
//! [`Registry::register_deferred`](crate::Registry::register_deferred)
//! needs an `&Rc<Registry<H>>`. The code that most wants to call it — a
//! `#[component(lazy)]` body, i.e. the inside of a separately shipped wasm
//! chunk — has no such handle: a component body returns an `Element`, and
//! the registry only appears one frame later, when the returned element is
//! realized (`vocabulary/src/handlers/lazy.rs` evaluates the chunk thunk
//! BEFORE calling `realize`, so not even an ambient-during-realize slot
//! would cover it).
//!
//! # The mailbox
//!
//! [`defer_registration`] boxes a `FnOnce(&Rc<Registry<H>>)`, type-erases
//! it, and queues it on a thread-local keyed by `TypeId::of::<H>()`.
//! [`realize`](crate::realize) drains the queue for ITS host type at the
//! top of every realization (guarded by [`has_pending_registrations`], so
//! the overwhelmingly common empty case costs one thread-local read). By
//! the time the chunk's own element is walked, the handler it queued on
//! the way in is resident — and any item of that kind parked earlier
//! elsewhere in the tree realizes in place as part of the same drain.
//!
//! # Why this keeps the SDK out of `main.wasm`
//!
//! Main only ever names the type-erased `Box<dyn FnOnce(&Rc<Registry<H>>)>`
//! in the drain path. The concrete closure — and the handler plus whatever
//! heavy library it reaches — is constructed only inside the chunk, so
//! wasm-split's reachability analysis places all of it in the chunk and
//! the release data-prune evicts its statics from main.
//! `tests/lazy-payload-split` measures exactly this.
//!
//! Keying by host type (rather than a single global queue) is inherited
//! from the old core's `defer_external_registration` and for the same
//! reason: a process that hosts more than one backend at once (the dev
//! recorder alongside a web boot in one test binary) must never apply one
//! host's registration to another's registry.

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::rc::Rc;

use crate::host::Host;
use crate::registry::Registry;

thread_local! {
    /// `(host TypeId, Box<dyn FnOnce(&Rc<Registry<H>>)> erased as Any)`,
    /// in submission order.
    static PENDING: RefCell<Vec<(TypeId, Box<dyn Any>)>> = const { RefCell::new(Vec::new()) };
}

/// Queue a handler registration to be applied to host `H`'s registry at
/// the next realization on this thread. `apply` typically calls
/// [`Registry::register_deferred`], whose payload kind the app must have
/// declared with [`Registry::defer`] at boot.
///
/// ```ignore
/// // In an SDK's web leaf, called from inside a `#[component(lazy)]` body:
/// pub fn register_from_chunk() {
///     runtime_scene::defer_registration::<backend_web::WebBackend, _>(|registry| {
///         registry.register_deferred::<MapViewProps, _>(mount_map_view);
///     });
/// }
/// ```
///
/// Applying is idempotent from the framework's side (the queue entry is
/// removed before it runs), and a chunk loads once, so the common case
/// queues exactly once.
pub fn defer_registration<H, F>(apply: F)
where
    H: Host,
    F: FnOnce(&Rc<Registry<H>>) + 'static,
{
    let boxed: Box<dyn FnOnce(&Rc<Registry<H>>)> = Box::new(apply);
    PENDING.with(|q| {
        q.borrow_mut()
            .push((TypeId::of::<H>(), Box::new(boxed) as Box<dyn Any>));
    });
}

/// `true` if any registration is queued (for any host). The cheap guard
/// realize consults before paying for a borrow + partition.
pub fn has_pending_registrations() -> bool {
    PENDING
        .try_with(|q| !q.borrow().is_empty())
        .unwrap_or(false)
}

/// Apply and remove every queued registration for host `H`, in submission
/// order; returns how many ran. Registrations queued for OTHER host types
/// are left in place.
///
/// Called by [`realize`](crate::realize) at the top of every realization.
/// Entries are taken out of the queue BEFORE they run, so an `apply` that
/// itself realizes (draining parked items does exactly that) re-enters a
/// drain that finds an empty queue rather than recursing.
pub fn drain_registrations<H: Host>(registry: &Rc<Registry<H>>) -> usize {
    let mine: Vec<Box<dyn Any>> = PENDING.with(|q| {
        let mut q = q.borrow_mut();
        let mut mine = Vec::new();
        let mut rest = Vec::new();
        for (tid, boxed) in q.drain(..) {
            if tid == TypeId::of::<H>() {
                mine.push(boxed);
            } else {
                rest.push((tid, boxed));
            }
        }
        *q = rest;
        mine
    });
    let n = mine.len();
    for boxed in mine {
        // The stored TypeId keyed the entry to `H`, so this downcast
        // cannot fail; guard rather than `expect` so a future keying bug
        // degrades to a dropped registration (visible as a permanently
        // parked placeholder) instead of a panic in the mount path.
        if let Ok(apply) = boxed.downcast::<Box<dyn FnOnce(&Rc<Registry<H>>)>>() {
            (*apply)(registry);
        }
    }
    n
}

/// Drop every queued registration (test hygiene: the queue is
/// thread-local and `cargo test` reuses threads across tests).
#[cfg(test)]
pub(crate) fn clear_pending_registrations() {
    PENDING.with(|q| q.borrow_mut().clear());
}
