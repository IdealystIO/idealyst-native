//! The bundle half of the bridge: [`Bridged`], an [`Engine`] that keeps
//! values and closures here and forwards the graph to a host through `H`.
//!
//! Local tables, keyed by ids this side assigns and never reuses (so a
//! host callback can never land on a different value than the one it was
//! created for):
//!
//! - signal values, with the reverse map from the host's slot handle;
//! - effect bodies;
//! - cleanups;
//! - context values, plus the interning of this side's `TypeId`s into the
//!   `u32` context keys the host stores (a `TypeId` means nothing outside
//!   the binary that minted it).
//!
//! Re-entrancy follows the native engine's discipline: a value or body is
//! MOVED OUT of its table for the duration of user code and put back after,
//! so no table borrow is ever held across user code, and touching the same
//! signal from inside its own operation finds it gone — the same
//! `reentrant-signal-read` diagnostic the native arena gives.

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::marker::PhantomData;

use rustc_hash::FxHashMap;

use super::{GuestHooks, Handle, HostOps, Id};
use crate::engine::{Access, AnySignal, EffectClass, Engine, WorldId};
use crate::native;
use crate::SiteLoc;

/// The bridged engine, generic over its transport to the host.
pub(crate) struct Bridged<H>(PhantomData<fn() -> H>);

/// This side's hooks — what the host's proxies call. Stateless: the state is
/// the thread-local below, shared with [`Bridged`].
pub(crate) struct Local;

struct ValueEntry {
    handle: Handle,
    /// `None` while user code holds it (see the module docs).
    data: Option<Box<dyn AnySignal>>,
    /// The host freed the slot while `data` was out; drop it on return.
    freed: bool,
    /// The author's creation site, for the staged-read warning. Kept here
    /// because a source location cannot cross to the host. Read only by the
    /// debug-build diagnostic (in release `SiteLoc` is `()`).
    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    site: SiteLoc,
}

enum Body {
    Ready(Box<dyn FnMut()>),
    /// Taken out for its own run; `freed` records a `drop_effect` that
    /// arrived mid-run (an effect freeing itself), honoured on return.
    Running { freed: bool },
}

#[derive(Default)]
struct LocalState {
    next_id: Id,
    values: FxHashMap<Id, ValueEntry>,
    by_handle: FxHashMap<Handle, Id>,
    effects: FxHashMap<Id, Body>,
    cleanups: FxHashMap<Id, Box<dyn FnOnce()>>,
    ctx: FxHashMap<Id, Box<dyn Any>>,
    ctx_keys: FxHashMap<TypeId, Id>,
}

impl LocalState {
    fn next_id(&mut self) -> Id {
        self.next_id = self.next_id.checked_add(1).expect("bridge: local ids exhausted");
        self.next_id
    }

    fn ctx_key(&mut self, key: TypeId) -> Id {
        let next = self.ctx_keys.len() as Id + 1;
        *self.ctx_keys.entry(key).or_insert(next)
    }
}

thread_local! {
    static LOCAL: RefCell<LocalState> = RefCell::new(LocalState::default());
}

/// Short-lived access. Never run user code inside `f`.
fn with_local<R>(f: impl FnOnce(&mut LocalState) -> R) -> R {
    LOCAL.with(|l| f(&mut l.borrow_mut()))
}

/// [`with_local`] for the host's callbacks, which can arrive during THREAD
/// TEARDOWN: a world dropped after this thread-local was destroyed still
/// frees its slots and runs its cleanups. `None` then — and there is nothing
/// left to do, because the values and closures the table held were
/// destroyed with it. The native engine guards the same paths with
/// `try_with` for the same reason.
fn try_local<R>(f: impl FnOnce(&mut LocalState) -> R) -> Option<R> {
    LOCAL.try_with(|l| f(&mut l.borrow_mut())).ok()
}

fn ctx_key(key: TypeId) -> Id {
    with_local(|l| l.ctx_key(key))
}

/// Run `f` against the local value behind `handle`, moved out for the call.
/// The host has already vouched for the handle (live world, live slot).
fn with_value<R>(handle: Handle, f: impl FnOnce(&mut dyn AnySignal) -> R) -> R {
    enum Taken {
        Got(Id, Box<dyn AnySignal>),
        Reentrant,
        Missing,
    }
    // Decide under the borrow, panic outside it (the native engine's rule).
    let taken = with_local(|l| match l.by_handle.get(&handle).copied() {
        None => Taken::Missing,
        Some(id) => match l.values.get_mut(&id).and_then(|e| e.data.take()) {
            Some(data) => Taken::Got(id, data),
            None => Taken::Reentrant,
        },
    });
    let (id, mut data) = match taken {
        Taken::Got(id, data) => (id, data),
        Taken::Reentrant => native::reentrant_signal_panic(handle.0, handle.1),
        Taken::Missing => panic!(
            "runtime-world bridge: signal (world {}, slot {}) is live on the host but has no \
             value on this side — a host-owned signal reached the bundle without being \
             imported, or a bridge bug",
            handle.0, handle.1
        ),
    };
    let result = f(&mut *data);
    // Put back, unless the host freed the slot during `f`; then the value
    // drops here, outside the borrow.
    let leftover = with_local(|l| match l.values.get_mut(&id) {
        Some(e) if !e.freed => {
            e.data = Some(data);
            None
        }
        Some(_) => {
            let e = l.values.remove(&id).expect("present");
            l.by_handle.remove(&e.handle);
            Some(data)
        }
        None => Some(data),
    });
    drop(leftover);
    result
}

impl GuestHooks for Local {
    fn commit(value: Id, forced: bool) -> bool {
        // The native flush moves a slot's box out to commit it; a box that is
        // already out (accessed from inside its own operation) is skipped —
        // `continue` there, "nothing changed" here, identical downstream.
        let Some(mut data) = try_local(|l| l.values.get_mut(&value).and_then(|e| e.data.take())).flatten() else {
            return false;
        };
        let changed = data.commit(forced);
        let leftover = try_local(|l| match l.values.get_mut(&value) {
            Some(e) if !e.freed => {
                e.data = Some(data);
                None
            }
            Some(_) => {
                let e = l.values.remove(&value).expect("present");
                l.by_handle.remove(&e.handle);
                Some(data)
            }
            None => Some(data),
        });
        drop(leftover);
        changed
    }

    fn drop_value(value: Id) {
        let removed = try_local(|l| match l.values.get_mut(&value) {
            // Out for an operation: mark, and the operation drops it.
            Some(e) if e.data.is_none() => {
                e.freed = true;
                None
            }
            Some(_) => {
                let e = l.values.remove(&value).expect("present");
                l.by_handle.remove(&e.handle);
                Some(e)
            }
            None => None,
        });
        drop(removed);
    }

    fn run_effect(effect: Id) {
        let body = try_local(|l| match l.effects.get_mut(&effect) {
            Some(slot) if matches!(slot, Body::Ready(_)) => {
                match std::mem::replace(slot, Body::Running { freed: false }) {
                    Body::Ready(b) => Some(b),
                    Body::Running { .. } => None,
                }
            }
            _ => None,
        });
        // A body that is gone or already running is the native engine's
        // `None` body: skipped.
        let Some(mut body) = body.flatten() else { return };
        body();
        let leftover = try_local(|l| match l.effects.get_mut(&effect) {
            Some(Body::Running { freed: false }) => {
                l.effects.insert(effect, Body::Ready(body));
                None
            }
            _ => {
                l.effects.remove(&effect);
                Some(body)
            }
        });
        drop(leftover);
    }

    fn drop_effect(effect: Id) {
        let removed = try_local(|l| match l.effects.get_mut(&effect) {
            Some(Body::Running { freed }) => {
                *freed = true;
                None
            }
            Some(Body::Ready(_)) => l.effects.remove(&effect),
            None => None,
        });
        drop(removed);
    }

    fn run_cleanup(cleanup: Id) {
        if let Some(f) = try_local(|l| l.cleanups.remove(&cleanup)).flatten() {
            f();
        }
    }

    fn drop_cleanup(cleanup: Id) {
        let removed = try_local(|l| l.cleanups.remove(&cleanup));
        drop(removed);
    }

    fn drop_context(ctx: Id) {
        let removed = try_local(|l| l.ctx.remove(&ctx));
        drop(removed);
    }
}

fn store_ctx(value: Box<dyn Any>) -> Id {
    with_local(|l| {
        let id = l.next_id();
        l.ctx.insert(id, value);
        id
    })
}

/// `f` sees bundle-side context value `ctx`. Held under the table borrow,
/// as the native engine holds its context borrow across the same `f` (a
/// downcast and a `Clone`).
fn read_ctx<R>(ctx: Id, f: impl FnOnce(&dyn Any) -> R) -> Option<R> {
    LOCAL.with(|l| l.borrow().ctx.get(&ctx).map(|v| f(&**v)))
}

impl<H: HostOps> Engine for Bridged<H> {
    type Scope = Option<u32>;

    fn world_new() -> WorldId {
        H::world_new()
    }
    fn world_drop(world: WorldId) {
        H::world_drop(world)
    }
    fn world_enter<R>(world: WorldId, f: impl FnOnce() -> R) -> R {
        H::enter_push(world);
        struct Guard<H: HostOps>(PhantomData<fn() -> H>);
        impl<H: HostOps> Drop for Guard<H> {
            fn drop(&mut self) {
                H::enter_pop();
            }
        }
        let _guard = Guard::<H>(PhantomData);
        f()
    }
    fn world_flush(world: WorldId) {
        H::world_flush(world)
    }
    fn world_is_flushing(world: WorldId) -> bool {
        H::world_is_flushing(world)
    }
    fn world_provide(world: WorldId, key: TypeId, value: Box<dyn Any>) {
        let ctx = store_ctx(value);
        H::world_provide(world, ctx_key(key), ctx)
    }
    fn world_inject<R>(world: WorldId, key: TypeId, f: impl FnOnce(&dyn Any) -> R) -> Option<R> {
        let ctx = H::world_inject(world, ctx_key(key))?;
        read_ctx(ctx, f)
    }

    fn is_flushing() -> bool {
        H::is_flushing()
    }
    fn is_entered() -> bool {
        H::is_entered()
    }
    fn in_effect() -> bool {
        H::in_effect()
    }
    fn effect_depth() -> usize {
        H::effect_depth() as usize
    }
    fn current_effect() -> Option<(WorldId, u32, u32)> {
        H::current_effect()
    }
    fn in_collector() -> bool {
        H::in_collector()
    }

    fn signal_create(world: Option<WorldId>, data: Box<dyn AnySignal>, site: SiteLoc) -> (WorldId, u32, u32, bool) {
        let id = with_local(|l| {
            let id = l.next_id();
            l.values.insert(id, ValueEntry { handle: (0, 0, 0), data: Some(data), freed: false, site });
            id
        });
        let (handle, collected) = H::signal_create(world, id);
        with_local(|l| {
            if let Some(e) = l.values.get_mut(&id) {
                e.handle = handle;
            }
            l.by_handle.insert(handle, id);
        });
        (handle.0, handle.1, handle.2, collected)
    }
    fn signal_access<R>(world: WorldId, slot: u32, gen: u32, f: impl FnOnce(&mut dyn AnySignal) -> R) -> Access<R> {
        let h = (world, slot, gen);
        if !H::signal_check(h) {
            return Access::DeadWorld;
        }
        Access::Done(with_value(h, f))
    }
    fn signal_read<R>(
        world: WorldId,
        slot: u32,
        gen: u32,
        track: bool,
        f: impl FnOnce(&mut dyn AnySignal, bool) -> R,
    ) -> Access<R> {
        let h = (world, slot, gen);
        match H::signal_read_check(h, track) {
            None => Access::DeadWorld,
            Some(subscribed) => Access::Done(with_value(h, |d| f(d, subscribed))),
        }
    }
    fn signal_write<R>(
        world: WorldId,
        slot: u32,
        gen: u32,
        force: bool,
        f: impl FnOnce(&mut dyn AnySignal) -> R,
    ) -> Access<R> {
        let h = (world, slot, gen);
        if !H::signal_check(h) {
            return Access::DeadWorld;
        }
        let r = with_value(h, f);
        H::signal_enqueue(h, force);
        Access::Done(r)
    }
    fn signal_touch(world: WorldId, slot: u32, gen: u32) {
        H::signal_touch((world, slot, gen))
    }
    fn signal_is_alive(world: WorldId, slot: u32, gen: u32) -> bool {
        H::signal_is_alive((world, slot, gen))
    }
    fn signal_subscriber_count(world: WorldId, slot: u32, gen: u32) -> usize {
        H::signal_subscriber_count((world, slot, gen)) as usize
    }
    #[cfg(debug_assertions)]
    fn signal_created_at(world: WorldId, slot: u32) -> Option<SiteLoc> {
        with_local(|l| {
            l.by_handle
                .iter()
                .find(|((w, s, _), _)| *w == world && *s == slot)
                .and_then(|(_, id)| l.values.get(id))
                .map(|e| e.site)
        })
    }

    fn effect_create(world: Option<WorldId>, class: EffectClass, body: Box<dyn FnMut()>) -> (WorldId, u32, u32) {
        let id = with_local(|l| {
            let id = l.next_id();
            l.effects.insert(id, Body::Ready(body));
            id
        });
        // The host runs the body once, right now, through `run_effect` —
        // no table borrow is held across this call.
        H::effect_create(world, class, id)
    }
    fn effect_is_alive(world: WorldId, slot: u32, gen: u32) -> bool {
        H::effect_is_alive((world, slot, gen))
    }
    fn on_cleanup(f: Box<dyn FnOnce()>) {
        let id = with_local(|l| {
            let id = l.next_id();
            l.cleanups.insert(id, f);
            id
        });
        H::on_cleanup(id)
    }

    fn untrack<R>(f: impl FnOnce() -> R) -> R {
        H::untrack_push();
        struct Guard<H: HostOps>(PhantomData<fn() -> H>);
        impl<H: HostOps> Drop for Guard<H> {
            fn drop(&mut self) {
                H::untrack_pop();
            }
        }
        let _guard = Guard::<H>(PhantomData);
        f()
    }
    fn unscoped<R>(f: impl FnOnce() -> R) -> R {
        H::unscoped_begin();
        struct Guard<H: HostOps>(PhantomData<fn() -> H>);
        impl<H: HostOps> Drop for Guard<H> {
            fn drop(&mut self) {
                H::unscoped_end();
            }
        }
        let _guard = Guard::<H>(PhantomData);
        f()
    }
    fn unanchored<R>(f: impl FnOnce() -> R) -> R {
        H::unanchored_begin();
        struct Guard<H: HostOps>(PhantomData<fn() -> H>);
        impl<H: HostOps> Drop for Guard<H> {
            fn drop(&mut self) {
                H::unanchored_end();
            }
        }
        let _guard = Guard::<H>(PhantomData);
        f()
    }

    fn collect<R>(f: impl FnOnce() -> R) -> (R, Self::Scope) {
        H::collect_begin();
        struct Guard<H: HostOps> {
            armed: bool,
            _h: PhantomData<fn() -> H>,
        }
        impl<H: HostOps> Drop for Guard<H> {
            fn drop(&mut self) {
                if self.armed {
                    // Panic path: free what the collection gathered.
                    H::collect_abort();
                }
            }
        }
        let mut guard = Guard::<H> { armed: true, _h: PhantomData };
        let result = f();
        guard.armed = false;
        let scope = H::collect_end();
        (result, (scope != 0).then_some(scope))
    }
    fn scope_merge(into: &mut Self::Scope, other: Self::Scope) {
        match (*into, other) {
            (_, None) => {}
            (None, Some(b)) => *into = Some(b),
            (Some(a), Some(b)) => H::scope_merge(a, b),
        }
    }
    fn scope_len(scope: &Self::Scope) -> usize {
        scope.map_or(0, |s| H::scope_len(s) as usize)
    }
    fn scope_drop(scope: &mut Self::Scope) {
        if let Some(s) = scope.take() {
            H::scope_drop(s);
        }
    }

    fn ctx_provide(key: TypeId, value: Box<dyn Any>) {
        let ctx = store_ctx(value);
        H::ctx_provide(ctx_key(key), ctx)
    }
    fn ctx_inject<R>(key: TypeId, f: impl FnOnce(&dyn Any) -> R) -> Option<R> {
        let ctx = H::ctx_inject(ctx_key(key))?;
        read_ctx(ctx, f)
    }
}
