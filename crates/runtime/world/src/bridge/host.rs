//! The host half of the bridge: [`HostOps`] over the native arena.
//!
//! Every slot a bundle owns holds a proxy instead of the real thing:
//!
//! - a signal slot holds [`ValueProxy`] — its `commit` (the flush's equality
//!   cut) asks the bundle, which has the value and its `PartialEq`;
//! - an effect slot's body is an [`EffectProxy`] closure that asks the
//!   bundle to run the real body;
//! - a cleanup is a [`CleanupProxy`], context is a [`CtxProxy`].
//!
//! Each proxy's `Drop` tells the bundle to release its side. The native
//! engine already drops value boxes, effect bodies and context values with
//! no arena borrow held (its rule: user `Drop` code never runs under a
//! borrow), so those calls back into the bundle may re-enter the kernel.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::marker::PhantomData;
use std::rc::Rc;

use rustc_hash::FxHashMap;

use super::{GuestHooks, Handle, HostOps, Id, StageMode};
use crate::engine::{AnySignal, EffectClass, Engine, WorldId};
use crate::native::{self, CtxKey, EffectFrames, Native, OwnedItem, SavedCollectors, WorldArena};

/// The host side, generic over how it reaches the bundle.
pub struct Host<G>(PhantomData<fn() -> G>);

/// Host-side state the native engine has no slot for: scopes handed to the
/// bundle by id, and the stacks `unscoped` / `unanchored` suspended (their
/// closure forms keep these on the Rust stack; across the bridge the begin
/// and end are separate calls).
#[derive(Default)]
struct HostState {
    scopes: FxHashMap<u32, Vec<OwnedItem>>,
    next_scope: u32,
    unscoped: Vec<SavedCollectors>,
    unanchored: Vec<EffectFrames>,
    /// Host-owned signals handed to bundles (props), by slot.
    exports: FxHashMap<Handle, Rc<dyn Exported>>,
    /// Host context declared to bundles, by name.
    contexts: FxHashMap<String, ContextFetch>,
}

/// Encodes the host context declared under a name; `false` for none.
pub(crate) type ContextFetch = Rc<dyn Fn(&mut Vec<u8>) -> bool>;

/// A host-owned signal a bundle may read (and, if exported two-way, write).
/// Implemented by the typed layer, which holds the codec and knows `T`.
pub(crate) trait Exported {
    fn fetch(&self, staged: bool, out: &mut Vec<u8>) -> bool;
    fn stage(&self, bytes: &[u8], mode: StageMode);
}

/// Registers an export; unregisters it on drop. The host ties it to
/// whatever mounted the bundle component the signal was passed to.
#[must_use = "dropping the guard withdraws the export"]
pub struct ExportGuard {
    kind: GuardKind,
}

enum GuardKind {
    Signal(Handle),
    Context(String),
}

impl Drop for ExportGuard {
    fn drop(&mut self) {
        // Out of the table first, dropped after (a codec's captures may run
        // user `Drop` code).
        let removed: Option<Box<dyn Any>> = try_host(|h| match &self.kind {
            GuardKind::Signal(handle) => h.exports.remove(handle).map(|e| Box::new(e) as Box<dyn Any>),
            GuardKind::Context(name) => h.contexts.remove(name).map(|f| Box::new(f) as Box<dyn Any>),
        })
        .flatten();
        drop(removed);
    }
}

pub(crate) fn register_export(handle: Handle, export: Rc<dyn Exported>) -> ExportGuard {
    with_host(|h| h.exports.insert(handle, export));
    ExportGuard { kind: GuardKind::Signal(handle) }
}

pub(crate) fn register_context(name: &str, fetch: ContextFetch) -> ExportGuard {
    with_host(|h| h.contexts.insert(name.to_string(), fetch));
    ExportGuard { kind: GuardKind::Context(name.to_string()) }
}

thread_local! {
    // A thread-local of its own rather than a field of the native `Tls`:
    // it is touched only when a bundle is bridged, and `thread_local!`
    // claims a key lazily on first access, so apps without remote
    // components pay nothing (Android caps pthread TLS keys at 128 — see
    // the single-TLS note in `native.rs`).
    static HOST: RefCell<HostState> = RefCell::new(HostState::default());
}

fn with_host<R>(f: impl FnOnce(&mut HostState) -> R) -> R {
    HOST.with(|h| f(&mut h.borrow_mut()))
}

/// [`with_host`] for the paths that run from drop guards and so can run
/// during thread teardown (`None` once the thread-local is destroyed).
fn try_host<R>(f: impl FnOnce(&mut HostState) -> R) -> Option<R> {
    HOST.try_with(|h| f(&mut h.borrow_mut())).ok()
}

// ---------------------------------------------------------------------------
// Proxies
// ---------------------------------------------------------------------------

/// A bundle-owned signal value, as its host slot sees it.
///
/// Concrete (the bundle's hooks as fn pointers, not a `G` type parameter)
/// so the TYPED host layer can recognise a bundle-owned slot by downcast —
/// promotion (`remote::receive_signal`) has `T` but no `G`.
pub(crate) struct ValueProxy {
    pub(crate) value: Id,
    commit: fn(Id, bool) -> bool,
    pub(crate) drop_value: fn(Id),
    pub(crate) promote: fn(Id, &mut Vec<u8>, &mut Vec<u8>) -> Option<bool>,
    pub(crate) promote_finish: fn(Id),
    /// Set when the slot is promoted: the bundle's entry lives on as an
    /// import, and the promoted value releases it instead.
    pub(crate) defused: bool,
}

impl AnySignal for ValueProxy {
    fn commit(&mut self, forced: bool) -> bool {
        (self.commit)(self.value, forced)
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    #[cfg(feature = "hot-reload")]
    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
    #[cfg(feature = "hot-reload")]
    fn value_type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Self>()
    }
}

impl Drop for ValueProxy {
    fn drop(&mut self) {
        if !self.defused {
            (self.drop_value)(self.value);
        }
    }
}

/// Releases a bundle-owned effect body when the host frees the effect.
struct EffectProxy<G: GuestHooks> {
    effect: Id,
    _g: PhantomData<fn() -> G>,
}

impl<G: GuestHooks> Drop for EffectProxy<G> {
    fn drop(&mut self) {
        G::drop_effect(self.effect);
    }
}

/// A bundle-owned cleanup: runs it, or releases it if the host drops the
/// cleanup unrun (an effect freed before its next re-run).
struct CleanupProxy<G: GuestHooks> {
    cleanup: Id,
    ran: Cell<bool>,
    _g: PhantomData<fn() -> G>,
}

impl<G: GuestHooks> CleanupProxy<G> {
    fn run(self) {
        self.ran.set(true);
        G::run_cleanup(self.cleanup);
    }
}

impl<G: GuestHooks> Drop for CleanupProxy<G> {
    fn drop(&mut self) {
        if !self.ran.get() {
            G::drop_cleanup(self.cleanup);
        }
    }
}

/// A bundle-owned context value, as the host's context stack holds it.
struct CtxProxy<G: GuestHooks> {
    ctx: Id,
    _g: PhantomData<fn() -> G>,
}

impl<G: GuestHooks> Drop for CtxProxy<G> {
    fn drop(&mut self) {
        G::drop_context(self.ctx);
    }
}

fn ctx_proxy<G: GuestHooks>(ctx: Id) -> Box<dyn Any> {
    Box::new(CtxProxy::<G> { ctx, _g: PhantomData })
}

fn ctx_id<G: GuestHooks>(v: &dyn Any) -> Option<Id> {
    v.downcast_ref::<CtxProxy<G>>().map(|p| p.ctx)
}

// ---------------------------------------------------------------------------
// HostOps
// ---------------------------------------------------------------------------

/// Resolve `world` (`None` = ambient) the way `Native::signal_create` does,
/// including its dead-world panic.
fn resolve<R>(world: Option<WorldId>, what: &str, f: impl FnOnce(&Rc<WorldArena>) -> R) -> R {
    match world {
        None => native::with_ambient(f),
        Some(world) => match native::arena_of(world) {
            Some(arena) => f(&arena),
            None => panic!("runtime-world: {what} created in a dead world {world}"),
        },
    }
}

/// The stale-handle check every bundle-side signal access starts with.
/// `false` for a dead world.
fn check_live((world, slot, gen): Handle) -> bool {
    if native::arena_of(world).is_none() {
        return false;
    }
    if !Native::signal_is_alive(world, slot, gen) {
        native::stale_signal_panic(world, slot);
    }
    true
}

impl<G: GuestHooks> HostOps for Host<G> {
    fn world_new() -> WorldId {
        Native::world_new()
    }
    fn world_drop(world: WorldId) {
        Native::world_drop(world)
    }
    fn world_flush(world: WorldId) {
        Native::world_flush(world)
    }
    fn world_is_flushing(world: WorldId) -> bool {
        Native::world_is_flushing(world)
    }
    fn world_provide(world: WorldId, key: Id, ctx: Id) {
        native::world_provide(world, CtxKey::Foreign(key), ctx_proxy::<G>(ctx))
    }
    fn world_inject(world: WorldId, key: Id) -> Option<Id> {
        let arena = native::arena_of(world)?;
        native::context_top(&arena, CtxKey::Foreign(key), ctx_id::<G>).flatten()
    }
    fn enter_push(world: WorldId) {
        native::enter_push(world)
    }
    fn enter_pop() {
        native::enter_pop()
    }

    fn is_flushing() -> bool {
        Native::is_flushing()
    }
    fn is_entered() -> bool {
        Native::is_entered()
    }
    fn in_effect() -> bool {
        Native::in_effect()
    }
    fn effect_depth() -> u32 {
        Native::effect_depth() as u32
    }
    fn current_effect() -> Option<Handle> {
        Native::current_effect()
    }
    fn in_collector() -> bool {
        Native::in_collector()
    }

    fn signal_create(world: Option<WorldId>, value: Id) -> (Handle, bool) {
        let proxy: Box<dyn AnySignal> = Box::new(ValueProxy {
            value,
            commit: G::commit,
            drop_value: G::drop_value,
            promote: G::promote,
            promote_finish: G::promote_finish,
            defused: false,
        });
        resolve(world, "signal", |arena| {
            // The host slot's `created_at` is this line: the bundle keeps the
            // author's site itself (`Engine::signal_created_at`), since a
            // source location cannot cross a wasm boundary.
            let (slot, gen, collected) = native::create_signal(arena, proxy, crate::caller_site());
            ((arena.id, slot, gen), collected)
        })
    }
    fn signal_check(h: Handle) -> bool {
        check_live(h)
    }
    fn signal_read_check(h: Handle, track: bool) -> Option<bool> {
        // The native read's order: dead-world check, subscribe, stale check.
        native::arena_of(h.0)?;
        let subscribed = track && native::maybe_subscribe(h.0, h.1, h.2);
        check_live(h).then_some(subscribed)
    }
    fn signal_enqueue((world, slot, gen): Handle, force: bool) {
        if let Some(arena) = native::arena_of(world) {
            native::enqueue(&arena, slot, gen, force);
        }
    }
    fn signal_touch((world, slot, gen): Handle) {
        Native::signal_touch(world, slot, gen)
    }
    fn signal_is_alive((world, slot, gen): Handle) -> bool {
        Native::signal_is_alive(world, slot, gen)
    }
    fn signal_subscriber_count((world, slot, gen): Handle) -> u32 {
        Native::signal_subscriber_count(world, slot, gen) as u32
    }

    fn effect_create(world: Option<WorldId>, class: EffectClass, effect: Id) -> Handle {
        let proxy = EffectProxy::<G> { effect, _g: PhantomData };
        // `&proxy` makes the closure capture the WHOLE proxy (a bare
        // `proxy.effect` would capture only the `u32` under disjoint
        // capture, dropping the proxy — and releasing the bundle's body —
        // right here). The body dies with the closure, i.e. with the slot.
        let body: Box<dyn FnMut()> = Box::new(move || {
            let p = &proxy;
            G::run_effect(p.effect);
        });
        resolve(world, "effect", |arena| {
            let (slot, gen) = native::create_effect(arena, class, body);
            (arena.id, slot, gen)
        })
    }
    fn effect_is_alive((world, slot, gen): Handle) -> bool {
        Native::effect_is_alive(world, slot, gen)
    }
    fn on_cleanup(cleanup: Id) {
        let proxy = CleanupProxy::<G> { cleanup, ran: Cell::new(false), _g: PhantomData };
        native::on_cleanup(Box::new(move || proxy.run()))
    }

    fn untrack_push() {
        native::untrack_push()
    }
    fn untrack_pop() {
        native::untrack_pop()
    }
    fn unscoped_begin() {
        let saved = native::unscoped_begin();
        with_host(|h| h.unscoped.push(saved));
    }
    fn unscoped_end() {
        // A torn-down thread has nothing left to restore.
        if let Some(saved) = try_host(|h| h.unscoped.pop()).flatten() {
            native::unscoped_end(saved);
        }
    }
    fn unanchored_begin() {
        let saved = native::unanchored_begin();
        with_host(|h| h.unanchored.push(saved));
    }
    fn unanchored_end() {
        if let Some(saved) = try_host(|h| h.unanchored.pop()).flatten() {
            native::unanchored_end(saved);
        }
    }

    fn collect_begin() {
        native::collect_begin()
    }
    fn collect_end() -> u32 {
        let items = native::collect_end().expect("collector stack imbalance");
        if items.is_empty() {
            return 0;
        }
        with_host(|h| {
            // Ids start at 1: 0 is "collected nothing".
            h.next_scope = h.next_scope.checked_add(1).expect("bridge: scope ids exhausted");
            let id = h.next_scope;
            h.scopes.insert(id, items);
            id
        })
    }
    fn collect_abort() {
        native::drop_items(native::collect_end().unwrap_or_default());
    }
    fn scope_merge(into: u32, other: u32) {
        with_host(|h| {
            let mut moved = h.scopes.remove(&other).unwrap_or_default();
            h.scopes.entry(into).or_default().append(&mut moved);
        })
    }
    fn scope_len(scope: u32) -> u32 {
        with_host(|h| h.scopes.get(&scope).map_or(0, |s| s.len() as u32))
    }
    fn scope_drop(scope: u32) {
        // Out of the table first, freed after: freeing runs cleanups and
        // value drops, which may re-enter the bridge.
        let items = try_host(|h| h.scopes.remove(&scope)).flatten().unwrap_or_default();
        native::drop_items(items);
    }

    fn ctx_provide(key: Id, ctx: Id) {
        native::ctx_provide(CtxKey::Foreign(key), ctx_proxy::<G>(ctx))
    }
    fn ctx_inject(key: Id) -> Option<Id> {
        native::with_ambient(|arena| native::context_top(arena, CtxKey::Foreign(key), ctx_id::<G>)).flatten()
    }

    fn value_fetch(h: Handle, staged: bool, out: &mut Vec<u8>) -> bool {
        out.clear();
        // The `Rc` comes out of the table before the codec runs.
        let Some(export) = with_host(|s| s.exports.get(&h).cloned()) else { return false };
        export.fetch(staged, out)
    }
    fn value_stage(h: Handle, bytes: &[u8], mode: StageMode) {
        let export = with_host(|s| s.exports.get(&h).cloned()).unwrap_or_else(|| {
            panic!(
                "kernel bridge: a bundle wrote host signal (world {}, slot {}), which was not \
                 exported to it",
                h.0, h.1
            )
        });
        export.stage(bytes, mode)
    }
    fn ctx_fetch(name: &str, out: &mut Vec<u8>) -> bool {
        out.clear();
        let Some(fetch) = with_host(|s| s.contexts.get(name).cloned()) else { return false };
        fetch(out)
    }
}

/// Take ownership of bundle scope `id` out of the table — the host claiming a
/// remote component's scope as its own `Owned` (see
/// `remote::claim_scope`). Empty for `0` or an unknown id.
#[cfg_attr(any(feature = "loopback-engine", idealyst_stream_guest), allow(dead_code))]
pub(crate) fn take_scope(id: u32) -> Vec<OwnedItem> {
    if id == 0 {
        return Vec::new();
    }
    with_host(|h| h.scopes.remove(&id)).unwrap_or_default()
}

/// Scopes a bundle collected that nobody has claimed or dropped yet. Zero
/// whenever every remote tree has been claimed — what a leak check asserts.
pub(crate) fn pending_scopes() -> usize {
    try_host(|h| h.scopes.len()).unwrap_or(0)
}

/// Register `export` for slot `h` with no guard: a PROMOTED slot's value
/// withdraws it itself, when the slot is freed ([`unregister_export`]).
pub(crate) fn register_export_owned(h: Handle, export: Rc<dyn Exported>) {
    with_host(|s| s.exports.insert(h, export));
}

pub(crate) fn unregister_export(h: Handle) {
    let removed = try_host(|s| s.exports.remove(&h)).flatten();
    drop(removed);
}
