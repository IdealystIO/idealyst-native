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
use std::rc::Rc;
use std::marker::PhantomData;

use rustc_hash::FxHashMap;

use super::{GuestHooks, Handle, HostOps, Id, ImportSync, StageMode};
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
    /// `Some` for an IMPORTED value: the host owns the slot and the real
    /// value; `data` is a mirror refreshed around every operation (see
    /// [`with_value`]). `None` for a value this bundle created.
    sync: Option<Rc<dyn ImportSync>>,
    /// Set by [`Bridged::offer`] on a value this bundle created: the codec
    /// to hand it to the host with if the host promotes it (see
    /// [`GuestHooks::promote`]).
    promotable: Option<Rc<dyn ImportSync>>,
    /// Live imports of this slot ([`Bridged::import`]). One entry per host
    /// slot: a second import of the same slot (two props bound to one app
    /// signal, a remount overlapping the old tree) shares it, and the
    /// entry goes when the last import is released — keyed per import, the
    /// first release unmapped the slot under the others.
    imports: u32,
    /// The host frees this entry ([`GuestHooks::drop_value`]): a value
    /// this bundle created, promoted or not. Releasing imports of it never
    /// removes it.
    owned: bool,
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
    /// Host context types this bundle may inject, by type: the name the
    /// host declared them under, and how to decode one.
    remote_ctx: FxHashMap<TypeId, (&'static str, Rc<RemoteDecode>)>,
}

/// Decodes a host context value into this bundle's type, boxed.
pub(crate) type RemoteDecode = dyn Fn(&[u8]) -> Option<Box<dyn Any>>;

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

/// Remove `handle`'s mapping if it still names entry `id`.
fn unmap(l: &mut LocalState, handle: Handle, id: Id) {
    if l.by_handle.get(&handle) == Some(&id) {
        l.by_handle.remove(&handle);
    }
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

/// What an operation does to a value — decides how an IMPORTED value's
/// mirror is synced with its host slot around it.
#[derive(Clone, Copy)]
enum Op {
    /// A read: refresh the committed value.
    Read,
    /// A staged write (`set` / `update`): refresh committed AND staged (so
    /// updates compose on the host's staged value), then send the result.
    Write { force: bool },
    /// Raw access (`set_untracked`): refresh committed; send it back if the
    /// operation changed it.
    Access,
}

/// Run `f` against the local value behind `handle`, moved out for the call.
/// The host has already vouched for the handle (live world, live slot).
fn with_value<H: HostOps, R>(handle: Handle, op: Op, f: impl FnOnce(&mut dyn AnySignal) -> R) -> R {
    enum Taken {
        Got(Id, Box<dyn AnySignal>, Option<Rc<dyn ImportSync>>),
        Reentrant,
        Missing,
    }
    // Decide under the borrow, panic outside it (the native engine's rule).
    let taken = with_local(|l| match l.by_handle.get(&handle).copied() {
        None => Taken::Missing,
        Some(id) => match l.values.get_mut(&id) {
            Some(e) => match e.data.take() {
                Some(data) => Taken::Got(id, data, e.sync.clone()),
                None => Taken::Reentrant,
            },
            None => Taken::Missing,
        },
    });
    let (id, data, sync) = match taken {
        Taken::Got(id, data, sync) => (id, data, sync),
        Taken::Reentrant => native::reentrant_signal_panic(handle.0, handle.1),
        Taken::Missing => panic!(
            "runtime-world bridge: signal (world {}, slot {}) is live on the host but has no \
             value on this side — a host-owned signal reached the bundle without being \
             imported, or a bridge bug",
            handle.0, handle.1
        ),
    };
    // From here the value is out of its table. It goes back when this
    // guard drops — on return, AND if anything below panics (an export
    // gone, a refused write, a panic in `f`): left out, every later access
    // would report a misleading re-entrancy error instead.
    let mut taken = PutBack { id, data: Some(data) };
    let data: &mut dyn AnySignal = &mut **taken.data.as_mut().expect("just set");
    // An imported value: refresh the mirror from the host first.
    let mut pulled = Vec::new();
    if let Some(sync) = &sync {
        if !H::value_fetch(handle, false, &mut pulled) {
            panic!(
                "kernel bridge: host signal (world {}, slot {}) is no longer exported to this bundle",
                handle.0, handle.1
            );
        }
        let mut staged = Vec::new();
        let has_staged = matches!(op, Op::Write { .. }) && H::value_fetch(handle, true, &mut staged);
        sync.pull(&mut *data, &pulled, has_staged.then_some(&staged[..]));
    }
    let result = f(&mut *data);
    // ...and send what the operation wrote back to it.
    if let Some(sync) = &sync {
        let mut out = Vec::new();
        match op {
            Op::Read => {}
            Op::Write { force } => {
                if sync.take_next(&mut *data, &mut out) {
                    H::value_stage(handle, &out, if force { StageMode::SetAlways } else { StageMode::Set });
                }
            }
            Op::Access => {
                sync.encode_value(&mut *data, &mut out);
                if out != pulled {
                    H::value_stage(handle, &out, StageMode::Untracked);
                }
            }
        }
    }
    drop(taken);
    result
}

/// Puts a value [`with_value`] took out back into its entry when dropped,
/// unless the host freed the slot meanwhile; then the value drops here,
/// outside the borrow.
struct PutBack {
    id: Id,
    data: Option<Box<dyn AnySignal>>,
}

impl Drop for PutBack {
    fn drop(&mut self) {
        let Some(data) = self.data.take() else { return };
        let id = self.id;
        let leftover = try_local(|l| match l.values.get_mut(&id) {
            Some(e) if !e.freed => {
                e.data = Some(data);
                None
            }
            Some(_) => {
                let e = l.values.remove(&id).expect("present");
                unmap(l, e.handle, id);
                Some(data)
            }
            None => Some(data),
        })
        .flatten();
        drop(leftover);
    }
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
                unmap(l, e.handle, value);
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
                unmap(l, e.handle, value);
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

    fn promote(value: Id, committed: &mut Vec<u8>, staged: &mut Vec<u8>) -> Option<bool> {
        // Taken out under the borrow, encoded outside it (a codec is user
        // code), put back unchanged.
        let (mut data, sync) = with_local(|l| {
            let e = l.values.get_mut(&value)?;
            let sync = e.promotable.clone()?;
            Some((e.data.take()?, sync))
        })?;
        sync.encode_value(&mut *data, committed);
        let has_staged = sync.encode_next(&mut *data, staged);
        with_local(|l| {
            if let Some(e) = l.values.get_mut(&value) {
                e.data = Some(data);
            }
        });
        Some(has_staged)
    }

    fn promote_finish(value: Id) {
        let data = with_local(|l| {
            let e = l.values.get_mut(&value)?;
            // From now on the host owns the value; this entry mirrors it.
            e.sync = e.promotable.take();
            e.data.take().map(|d| (d, e.sync.clone()))
        });
        if let Some((mut data, sync)) = data {
            // The staged write moved to the host with the value.
            if let Some(sync) = &sync {
                sync.take_next(&mut *data, &mut Vec::new());
            }
            with_local(|l| {
                if let Some(e) = l.values.get_mut(&value) {
                    e.data = Some(data);
                }
            });
        }
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
        // In one process (`loopback-engine`) this table and the host
        // kernel's are both thread-locals, destroyed in reverse order of
        // first use. Touch this one first so it outlives the kernel's: the
        // kernel's teardown runs live effects' cleanups, which call back
        // here for the closures. (In a wasm bundle the table lives in the
        // bundle's instance; the order is moot.)
        with_local(|_| ());
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
            l.values.insert(
                id,
                ValueEntry {
                    handle: (0, 0, 0),
                    data: Some(data),
                    freed: false,
                    sync: None,
                    promotable: None,
                    imports: 0,
                    owned: true,
                    site,
                },
            );
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
        Access::Done(with_value::<H, R>(h, Op::Access, f))
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
            Some(subscribed) => Access::Done(with_value::<H, R>(h, Op::Read, |d| f(d, subscribed))),
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
        let r = with_value::<H, R>(h, Op::Write { force }, f);
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
        // This bundle's own provisions first, exactly like native shadowing:
        // the innermost provider of a type wins, and a bundle providing a
        // type shadows the host's for the bundle's subtree.
        if let Some(ctx) = H::ctx_inject(ctx_key(key)) {
            return read_ctx(ctx, f);
        }
        // Then host context the HOST declared to bundles, by name.
        let (name, decode) = with_local(|l| l.remote_ctx.get(&key).cloned())?;
        let mut bytes = Vec::new();
        if !H::ctx_fetch(name, &mut bytes) {
            return None;
        }
        // Decoding may import host signals (a context holding a
        // `ReadSignal`), which touches the local tables — so no borrow here.
        let value = decode(&bytes)?;
        Some(f(&*value))
    }
}

// ---------------------------------------------------------------------------
// Host-owned values in this bundle (props and context)
// ---------------------------------------------------------------------------

// Used by the bundle-side import API (`remote_guest`), which only exists
// where the bridged engine is the active one.
#[cfg_attr(not(any(idealyst_stream_guest, feature = "loopback-engine")), allow(dead_code))]
impl<H: HostOps> Bridged<H> {
    /// Encode a host-owned signal's committed value; `false` when it was not
    /// exported to this bundle.
    pub(crate) fn fetch(h: Handle, out: &mut Vec<u8>) -> bool {
        H::value_fetch(h, false, out)
    }

    /// Make host-owned slot `h` addressable from this bundle: `mirror` is the
    /// typed layer's storage holding the value as last fetched, `sync` keeps
    /// it in step with the host. Returns the local id to release it with.
    ///
    /// A slot already addressable here (imported before, or this bundle's
    /// own promoted value handed back as a prop) is shared: its entry
    /// counts one more import and `mirror` is dropped. A slot holds one
    /// type, so every import of it carries the same `T`.
    pub(crate) fn import(h: Handle, mirror: Box<dyn AnySignal>, sync: Rc<dyn ImportSync>, site: SiteLoc) -> Id {
        let (id, unused) = with_local(|l| {
            if let Some(id) = l.by_handle.get(&h).copied() {
                if let Some(e) = l.values.get_mut(&id) {
                    e.imports += 1;
                    return (id, Some(mirror));
                }
            }
            let id = l.next_id();
            l.values.insert(
                id,
                ValueEntry {
                    handle: h,
                    data: Some(mirror),
                    freed: false,
                    sync: Some(sync),
                    promotable: None,
                    imports: 1,
                    owned: false,
                    site,
                },
            );
            l.by_handle.insert(h, id);
            (id, None)
        });
        // Typed storage: dropped outside the borrow.
        drop(unused);
        id
    }

    /// Allow the value behind `h` to be promoted with `sync` — the bundle
    /// is handing it to native code, which may take it over. A no-op for a
    /// value the host already owns (an import).
    pub(crate) fn offer(h: Handle, sync: Rc<dyn ImportSync>) {
        with_local(|l| {
            let Some(id) = l.by_handle.get(&h).copied() else { return };
            if let Some(e) = l.values.get_mut(&id) {
                if e.sync.is_none() {
                    e.promotable = Some(sync);
                }
            }
        });
    }

    /// Forget import `id` (the importing scope ended). The host slot is the
    /// host's and is untouched.
    pub(crate) fn release_import(id: Id) {
        let removed = try_local(|l| {
            let e = l.values.get_mut(&id)?;
            e.imports = e.imports.saturating_sub(1);
            if e.imports > 0 || e.owned {
                return None;
            }
            if e.data.is_none() {
                // Out for an operation (a cleanup releasing an import from
                // inside one): the operation drops it on return.
                e.freed = true;
                return None;
            }
            let e = l.values.remove(&id).expect("present");
            unmap(l, e.handle, id);
            Some(e)
        })
        .flatten();
        drop(removed);
    }

    /// Declare that `inject::<T>()` may fall back to the host context the
    /// host declared under `name`.
    pub(crate) fn register_remote_context(key: TypeId, name: &'static str, decode: Rc<RemoteDecode>) {
        with_local(|l| l.remote_ctx.insert(key, (name, decode)));
    }
}
