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
/// bundle by id, and the frames bundles have open on the kernel's stacks.
#[derive(Default)]
struct HostState {
    scopes: FxHashMap<u32, Vec<OwnedItem>>,
    next_scope: u32,
    /// Every frame a bundle has open on the native kernel's stacks,
    /// innermost last. Natively each is a closure (`enter`, `untrack`,
    /// `collect_owned`, …) whose guard pops it, even on panic; across the
    /// bridge the begin and end are separate calls, and a bundle that
    /// TRAPS between them never makes the end call (a wasm panic aborts:
    /// no destructor in the bundle runs). Journaled, the app unwinds them
    /// itself ([`unwind_frames`]) instead of running on with a world still
    /// entered, tracking off, or its creations collected into a dead scope.
    frames: Vec<Frame>,
    /// Bundle requests are untrusted input (see [`fault`]): `true` once a
    /// host that can stop a bundle (a wasm one) has said so.
    trap_faults: bool,
    /// The first fault since the host last asked ([`take_fault`]).
    fault: Option<String>,
    /// Host-owned signals handed to bundles (props), by slot: EVERY live
    /// registration, each under its guard's token. The same signal is
    /// exported once per mount it's passed to (two remote components, a
    /// remount before the old one drops), so a guard withdraws only its
    /// own registration — keyed by slot alone, the first unmount withdrew
    /// the export the other mount still used.
    exports: FxHashMap<Handle, Vec<Registration<Rc<dyn Exported>>>>,
    /// Host context declared to bundles, by name (the same rule).
    contexts: FxHashMap<String, Vec<Registration<ContextFetch>>>,
    next_token: u64,
}

/// A frame a bundle opened on the native kernel's stacks.
enum Frame {
    Enter,
    Untrack,
    /// The collector stack `unscoped` suspended.
    Unscoped(SavedCollectors),
    /// The effect frames `unanchored` suspended.
    Unanchored(EffectFrames),
    Collect,
}

impl Frame {
    fn is(&self, other: &Frame) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }

    /// End it, as the bundle's end call would have.
    fn end(self) {
        match self {
            Frame::Enter => native::enter_pop(),
            Frame::Untrack => native::untrack_pop(),
            Frame::Unscoped(saved) => native::unscoped_end(saved),
            Frame::Unanchored(saved) => native::unanchored_end(saved),
            // Abandoned: free what it collected (the bundle that made them
            // is stopped).
            Frame::Collect => native::drop_items(native::collect_end().unwrap_or_default()),
        }
    }
}

fn push_frame(frame: Frame) {
    with_host(|h| h.frames.push(frame));
}

/// Close the innermost open frame of `kind`'s kind and return it. Bundle
/// frames nest (each is a closure in the bundle), so it is the top one.
fn pop_frame(kind: Frame) -> Option<Frame> {
    try_host(|h| {
        let at = h.frames.iter().rposition(|f| f.is(&kind))?;
        Some(h.frames.remove(at))
    })
    .flatten()
}

/// A bundle asked the app's kernel for something invalid: a pop with no
/// matching push, a signal created outside any world, a write to a value
/// it was not given (or was given read-only), bytes that do not decode.
/// Natively each is an author bug that panics; here it is input from code
/// the app did not compile, and with `panic = "abort"` a panic would take
/// the app down with it. A wasm host turns the fault into a TRAP in the
/// bundle that made the request, which stops that bundle and nothing else;
/// the request itself is not carried out. In one process (the loopback
/// tests) there is no bundle to stop, so it panics, as natively.
/// Whether [`HostState::fault`] holds one: checked on EVERY kernel import
/// ([`take_fault`]), so the common case — none — is one load, not a
/// thread-local borrow.
static FAULT_PENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn fault(msg: String) {
    let trapped = try_host(|h| {
        if !h.trap_faults {
            return false;
        }
        h.fault.get_or_insert(msg.clone());
        FAULT_PENDING.store(true, std::sync::atomic::Ordering::Relaxed);
        true
    })
    .unwrap_or(false);
    if !trapped {
        panic!("{msg}");
    }
}

pub(crate) fn export_registrations() -> usize {
    try_host(|h| h.exports.values().map(Vec::len).sum()).unwrap_or(0)
}

/// Report faults from now on rather than panicking (see [`fault`]).
pub(crate) fn trap_faults() {
    with_host(|h| h.trap_faults = true);
}

/// The fault the last bundle request raised, if any (see [`fault`]).
pub(crate) fn take_fault() -> Option<String> {
    if !FAULT_PENDING.swap(false, std::sync::atomic::Ordering::Relaxed) {
        return None;
    }
    try_host(|h| h.fault.take()).flatten()
}

/// Close a bundle frame of `kind`'s kind, faulting if it has none open:
/// a bundle only ends what it began.
fn end_frame(kind: Frame, what: &str) -> Option<Frame> {
    let frame = pop_frame(kind);
    if frame.is_none() {
        fault(format!("kernel bridge: a bundle ended a `{what}` it never began"));
    }
    frame
}

/// How many bundle frames are open: taken before calling into a bundle, so
/// [`unwind_frames`] can close what the call left open if it traps.
pub(crate) fn frames_mark() -> usize {
    try_host(|h| h.frames.len()).unwrap_or(0)
}

/// End every bundle frame opened since `mark`, innermost first, as the
/// bundle's own end calls would have. One at a time with no borrow held:
/// freeing a collected scope runs cleanups, which may enter the bridge.
pub(crate) fn unwind_frames(mark: usize) {
    while let Some(frame) = try_host(|h| (h.frames.len() > mark).then(|| h.frames.pop()).flatten()).flatten() {
        frame.end();
    }
}

/// One registration of an export: its guard's token, and the export.
struct Registration<T> {
    token: u64,
    value: T,
}

/// The token of a PROMOTED slot's own export ([`register_export_owned`]):
/// a slot is promoted at most once, and its value withdraws it.
const OWNED: u64 = 0;

impl HostState {
    fn token(&mut self) -> u64 {
        self.next_token += 1;
        self.next_token
    }

    /// The export bundles reach slot `h` through: a writable one if any
    /// registration is (a read-only re-export of a signal must not take
    /// away a bundle's right to write it), else the latest.
    fn export(&self, h: &Handle) -> Option<Rc<dyn Exported>> {
        let regs = self.exports.get(h)?;
        regs.iter().find(|r| r.value.writable()).or(regs.last()).map(|r| r.value.clone())
    }

    fn remove_export(&mut self, h: &Handle, token: u64) -> Option<Rc<dyn Exported>> {
        let regs = self.exports.get_mut(h)?;
        let at = regs.iter().position(|r| r.token == token)?;
        let removed = regs.remove(at).value;
        if regs.is_empty() {
            self.exports.remove(h);
        }
        Some(removed)
    }
}

/// Encodes the host context declared under a name; `false` for none.
pub(crate) type ContextFetch = Rc<dyn Fn(&mut Vec<u8>) -> bool>;

/// A host-owned signal a bundle may read (and, if exported two-way, write).
/// Implemented by the typed layer, which holds the codec and knows `T`.
pub(crate) trait Exported {
    fn fetch(&self, staged: bool, out: &mut Vec<u8>) -> bool;
    /// `Err` (a [`fault`]) when it is read-only or `bytes` don't decode.
    fn stage(&self, bytes: &[u8], mode: StageMode) -> Result<(), String>;
    /// Whether a bundle may write through it (a two-way prop, or a
    /// promoted slot's own export).
    fn writable(&self) -> bool;
}

/// Registers an export; unregisters it on drop. The host ties it to
/// whatever mounted the bundle component the signal was passed to.
#[must_use = "dropping the guard withdraws the export"]
pub struct ExportGuard {
    kind: GuardKind,
}

enum GuardKind {
    Signal(Handle, u64),
    Context(String, u64),
}

impl Drop for ExportGuard {
    fn drop(&mut self) {
        // Out of the table first, dropped after (a codec's captures may run
        // user `Drop` code).
        let removed: Option<Box<dyn Any>> = try_host(|h| match &self.kind {
            GuardKind::Signal(handle, token) => h.remove_export(handle, *token).map(|e| Box::new(e) as Box<dyn Any>),
            GuardKind::Context(name, token) => {
                let regs = h.contexts.get_mut(name)?;
                let at = regs.iter().position(|r| r.token == *token)?;
                let removed = regs.remove(at).value;
                if regs.is_empty() {
                    h.contexts.remove(name);
                }
                Some(Box::new(removed) as Box<dyn Any>)
            }
        })
        .flatten();
        drop(removed);
    }
}

pub(crate) fn register_export(handle: Handle, export: Rc<dyn Exported>) -> ExportGuard {
    let token = with_host(|h| {
        let token = h.token();
        h.exports.entry(handle).or_default().push(Registration { token, value: export });
        token
    });
    ExportGuard { kind: GuardKind::Signal(handle, token) }
}

pub(crate) fn register_context(name: &str, fetch: ContextFetch) -> ExportGuard {
    let token = with_host(|h| {
        let token = h.token();
        h.contexts.entry(name.to_string()).or_default().push(Registration { token, value: fetch });
        token
    });
    ExportGuard { kind: GuardKind::Context(name.to_string(), token) }
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
/// The world a bundle creates `what` in: its own, or the ambient one.
/// `None` (a [`fault`]) for a dead world or none at all.
fn resolve(world: Option<WorldId>, what: &str) -> Option<Rc<WorldArena>> {
    let arena = match world {
        None => native::try_ambient(),
        Some(world) => native::arena_of(world),
    };
    if arena.is_none() {
        fault(match world {
            None => format!("{what}() called outside World::enter — components must run inside a reactive context"),
            Some(world) => format!("runtime-world: {what} created in a dead world {world}"),
        });
    }
    arena
}

/// The handle a refused creation answers: never live, so nothing reaches
/// it. (A wasm host never even returns it: the fault traps first.)
const REFUSED: Handle = (WorldId::MAX, 0, 0);

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
        if native::arena_of(world).is_none() {
            return fault(format!("kernel bridge: a bundle entered world {world}, which does not exist"));
        }
        native::enter_push(world);
        push_frame(Frame::Enter);
    }
    fn enter_pop() {
        if end_frame(Frame::Enter, "enter").is_some() {
            native::enter_pop()
        }
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
        // Checked before the proxy exists: a refused proxy's drop would
        // call back into the bundle mid-request.
        let Some(arena) = resolve(world, "signal") else { return (REFUSED, false) };
        let proxy: Box<dyn AnySignal> = Box::new(ValueProxy {
            value,
            commit: G::commit,
            drop_value: G::drop_value,
            promote: G::promote,
            promote_finish: G::promote_finish,
            defused: false,
        });
        // The host slot's `created_at` is this line: the bundle keeps the
        // author's site itself (`Engine::signal_created_at`), since a
        // source location cannot cross a wasm boundary.
        let (slot, gen, collected) = native::create_signal(&arena, proxy, crate::caller_site());
        ((arena.id, slot, gen), collected)
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
        let Some(arena) = resolve(world, "effect") else { return REFUSED };
        let proxy = EffectProxy::<G> { effect, _g: PhantomData };
        // `&proxy` makes the closure capture the WHOLE proxy (a bare
        // `proxy.effect` would capture only the `u32` under disjoint
        // capture, dropping the proxy — and releasing the bundle's body —
        // right here). The body dies with the closure, i.e. with the slot.
        let body: Box<dyn FnMut()> = Box::new(move || {
            let p = &proxy;
            G::run_effect(p.effect);
        });
        let (slot, gen) = native::create_effect(&arena, class, body);
        (arena.id, slot, gen)
    }
    fn effect_is_alive((world, slot, gen): Handle) -> bool {
        Native::effect_is_alive(world, slot, gen)
    }
    fn on_cleanup(cleanup: Id) {
        if !Native::in_effect() {
            return fault(native::ON_CLEANUP_OUTSIDE_EFFECT.to_string());
        }
        let proxy = CleanupProxy::<G> { cleanup, ran: Cell::new(false), _g: PhantomData };
        native::on_cleanup(Box::new(move || proxy.run()))
    }

    fn untrack_push() {
        native::untrack_push();
        push_frame(Frame::Untrack);
    }
    fn untrack_pop() {
        if end_frame(Frame::Untrack, "untrack").is_some() {
            native::untrack_pop()
        }
    }
    fn unscoped_begin() {
        let saved = native::unscoped_begin();
        push_frame(Frame::Unscoped(saved));
    }
    fn unscoped_end() {
        // A torn-down thread has nothing left to restore.
        if let Some(Frame::Unscoped(saved)) = end_frame(Frame::Unscoped(Default::default()), "unscoped") {
            native::unscoped_end(saved);
        }
    }
    fn unanchored_begin() {
        let saved = native::unanchored_begin();
        push_frame(Frame::Unanchored(saved));
    }
    fn unanchored_end() {
        if let Some(Frame::Unanchored(saved)) = end_frame(Frame::Unanchored(Default::default()), "unanchored") {
            native::unanchored_end(saved);
        }
    }

    fn collect_begin() {
        native::collect_begin();
        push_frame(Frame::Collect);
    }
    fn collect_end() -> u32 {
        if end_frame(Frame::Collect, "collect_owned").is_none() {
            return 0;
        }
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
        if end_frame(Frame::Collect, "collect_owned").is_none() {
            return;
        }
        native::drop_items(native::collect_end().unwrap_or_default());
    }
    fn scope_merge(into: u32, other: u32) {
        let orphaned = with_host(|h| {
            let mut moved = h.scopes.remove(&other)?;
            match h.scopes.get_mut(&into) {
                Some(scope) => {
                    scope.append(&mut moved);
                    None
                }
                // A scope the bundle no longer holds (claimed, dropped, or
                // never its own): parking the items under it would leave
                // them pending for good. They are freed instead.
                None => Some(moved),
            }
        });
        if let Some(items) = orphaned {
            native::drop_items(items);
            fault(format!("kernel bridge: a bundle merged into scope {into}, which it does not hold"));
        }
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
        if resolve(None, "provide").is_none() {
            return;
        }
        native::ctx_provide(CtxKey::Foreign(key), ctx_proxy::<G>(ctx))
    }
    fn ctx_inject(key: Id) -> Option<Id> {
        let arena = resolve(None, "inject")?;
        native::context_top(&arena, CtxKey::Foreign(key), ctx_id::<G>).flatten()
    }

    fn value_fetch(h: Handle, staged: bool, out: &mut Vec<u8>) -> bool {
        out.clear();
        // The `Rc` comes out of the table before the codec runs.
        let Some(export) = with_host(|s| s.export(&h)) else { return false };
        export.fetch(staged, out)
    }
    fn value_stage(h: Handle, bytes: &[u8], mode: StageMode) {
        let Some(export) = with_host(|s| s.export(&h)) else {
            return fault(format!(
                "kernel bridge: a bundle wrote host signal (world {}, slot {}), which was not exported to it",
                h.0, h.1
            ));
        };
        if let Err(msg) = export.stage(bytes, mode) {
            fault(msg);
        }
    }
    fn ctx_fetch(name: &str, out: &mut Vec<u8>) -> bool {
        out.clear();
        let Some(fetch) = with_host(|s| s.contexts.get(name).and_then(|r| r.last()).map(|r| r.value.clone())) else {
            return false;
        };
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
    with_host(|s| s.exports.entry(h).or_default().push(Registration { token: OWNED, value: export }));
}

/// Withdraw a promoted slot's own export; any guard-held re-exports of it
/// stay until their guards drop.
pub(crate) fn unregister_export(h: Handle) {
    let removed = try_host(|s| s.remove_export(&h, OWNED)).flatten();
    drop(removed);
}
