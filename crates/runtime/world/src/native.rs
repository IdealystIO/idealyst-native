//! The native engine: the in-process arena every app runs. See
//! [`crate::engine`] for the seam this implements and why it exists.
//!
//! Everything here was the body of `lib.rs` before the engine seam; the
//! code is unchanged except that values arrive and leave type-erased
//! (`Box<dyn AnySignal>`) and creation returns raw `(slot, gen)` pairs that
//! the typed layer wraps into handles.

use std::any::{Any, TypeId};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rustc_hash::FxHashMap;

use crate::engine::{Access, AnySignal, EffectClass, Engine, WorldId};
use crate::SiteLoc;

/// The native engine. Zero-sized: its state is the thread-local below.
pub(crate) struct Native;

/// A context key. The typed layer keys context by `TypeId`; the bridge
/// (`crate::bridge`) keys a remote bundle's context by an id the bundle
/// assigns, because the bundle's `TypeId`s mean nothing in this binary.
/// The two never collide: they are different variants.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum CtxKey {
    Type(TypeId),
    #[cfg_attr(not(any(test, all(feature = "bridge", any(not(target_arch = "wasm32"), idealyst_stream_guest)))), allow(dead_code))]
    Foreign(u64),
}

// ============================================================================
// The one thread-local — registry of live worlds + every transient stack.
// Consulted at handle use time (routing) and at creation time (ambient).
// Nothing here owns reactive state except the registry's arena Rcs; the
// stacks are empty at rest.
// ============================================================================

/// `(effect slot, effect generation)` — how a signal's subscriber list and
/// the running-effect stack identify an effect without owning it.
type EffectKey = (u32, u32);

struct Tls {
    /// Live worlds. A handle whose id is absent here belongs to a dead world.
    worlds: FxHashMap<WorldId, Rc<WorldArena>>,
    /// Next world id to hand out. Starts at 1 — 0 is never a valid world.
    next_world: WorldId,
    /// Ambient *creation* context: the world the free `signal()`/`effect()`/
    /// `provide()` fns create into. A stack, so worlds nest.
    enter_stack: Vec<WorldId>,
    /// The effects currently executing, innermost last (they nest when an
    /// effect body creates another effect, or flushes another world). A
    /// tracked read subscribes the TOP entry — and only if that effect lives
    /// in the signal's world; see `maybe_subscribe` for why.
    effect_stack: Vec<(WorldId, u32, u32)>,
    /// Per-running-effect dependency collection frames, parallel to
    /// `effect_stack`. A tracked read appends `(signal_slot, signal_gen)`
    /// to the TOP frame; the run's end RECONCILES the frame against the
    /// effect's previous dep set (`reconcile_deps`). This is the
    /// stable-deps fast path: an effect whose reads are identical run to
    /// run (the overwhelmingly common shape — style bindings, text
    /// bindings) touches NO subscriber list on re-run. The old scheme
    /// (unsubscribe-all upfront via `retain`, re-subscribe via
    /// `contains`) was O(subscribers) per operation — quadratic for N
    /// same-signal subscribers per commit, measured as the dominant cost
    /// of the js-framework-bench shared-style fan-out (1k effects × 1k
    /// entry scans).
    pending_deps: Vec<Vec<(u32, u32)>>,
    /// In-flight ownership collectors (see [`collect_owned`]), innermost
    /// last. Signal/effect creations register into the top collector; with
    /// none active they are world-root-owned.
    collectors: Vec<Vec<OwnedItem>>,
    /// Depth of nested [`untrack`] scopes. Tracking is suspended globally
    /// (across all worlds) while non-zero — see `untrack` for the rationale.
    untrack_depth: u32,
    /// Depth of in-progress flushes across all worlds on this thread.
    /// Non-zero ⇒ [`is_flushing`] is true.
    flush_depth: u32,
    /// One-entry cache in front of `worlds`: the last world resolved and its
    /// arena. Almost every access on a thread is to the same world, so the
    /// common case is a compare instead of a hash + probe.
    ///
    /// Why it exists: before the engine seam the optimizer happened to
    /// inline the `worlds` lookup into every signal operation; after it, at
    /// opt-level "z" without LTO (the web release profile) it stopped, and
    /// the lookup's out-of-line calls measured +14% on untracked reads.
    /// Inlining hints moved the cost between profiles rather than removing
    /// it. The cache makes the hot path independent of that decision.
    ///
    /// INVARIANT: never names a dead world. `world_drop` clears it in the
    /// same step that unregisters the world, and world ids are never
    /// reused, so a cache hit is exactly as live as a `worlds` hit. `Cell` /
    /// `RefCell` so a lookup can refresh it under the shared borrow that
    /// lookups take.
    cached_id: Cell<WorldId>,
    cached_arena: RefCell<Option<Rc<WorldArena>>>,
}

impl Tls {
    /// Resolve `world` through the cache, falling back to the registry and
    /// caching the result. Clones the `Rc` out; never runs user code.
    #[inline]
    fn lookup(&self, world: WorldId) -> Option<Rc<WorldArena>> {
        if self.cached_id.get() == world {
            if let Some(arena) = &*self.cached_arena.borrow() {
                return Some(Rc::clone(arena));
            }
        }
        let arena = self.worlds.get(&world).cloned()?;
        self.cached_id.set(world);
        // The replaced entry is a clone of a registered world's Rc (it is
        // cleared on unregister), so dropping it here frees nothing.
        *self.cached_arena.borrow_mut() = Some(Rc::clone(&arena));
        Some(arena)
    }
}

thread_local! {
    static TLS: RefCell<Tls> = RefCell::new(Tls {
        worlds: FxHashMap::default(),
        next_world: 1,
        enter_stack: Vec::new(),
        effect_stack: Vec::new(),
        pending_deps: Vec::new(),
        collectors: Vec::new(),
        untrack_depth: 0,
        flush_depth: 0,
        cached_id: Cell::new(0),
        cached_arena: RefCell::new(None),
    });
}

/// Short-lived TLS access. Never call user code inside `f` — the RefCell
/// borrow is held for its duration.
fn with_tls<R>(f: impl FnOnce(&mut Tls) -> R) -> R {
    TLS.with(|t| f(&mut t.borrow_mut()))
}

/// Resolve a handle's world to its arena. `None` means the world is dead
/// (dropped, or the handle crossed to another thread, or the thread is
/// tearing down its TLS) — callers decide whether that is a panic (reads)
/// or a silent no-op (writes, frees).
pub(crate) fn arena_of(world: WorldId) -> Option<Rc<WorldArena>> {
    TLS.try_with(|t| t.borrow().lookup(world)).ok().flatten()
}

/// Run `f` against the ambient world's arena, or panic with the canonical
/// "outside enter" message. Only *creation*-side APIs (free `signal()`,
/// `effect()`, `provide()`, …) consult the ambient world; handle *use*
/// routes through the handle's own world id.
pub(crate) fn with_ambient<R>(f: impl FnOnce(&Rc<WorldArena>) -> R) -> R {
    let arena = TLS.with(|t| {
        let t = t.borrow();
        t.enter_stack.last().and_then(|&id| t.lookup(id))
    });
    match arena {
        Some(arena) => f(&arena),
        None => panic!(
            "signal()/effect() called outside World::enter — components must run inside a reactive context"
        ),
    }
}


pub(crate) struct WorldArena {
    pub(crate) id: WorldId,
    pub(crate) signals: RefCell<Vec<SignalSlot>>,
    free_signals: RefCell<Vec<u32>>,
    effects: RefCell<Vec<EffectSlot>>,
    free_effects: RefCell<Vec<u32>>,
    /// Signal slots with a staged `next` (or a forced notify), awaiting
    /// commit. Deduped via `SignalSlot::queued`. Empty at rest.
    staged: RefCell<Vec<u32>>,
    /// World context: for each type, the STACK of live provisions, newest
    /// last. `inject` reads the top. A stack rather than a single slot
    /// because provisions are owned (see [`CtxEntry`]): retracting a
    /// shadowed entry must leave its shadower in place, and retracting a
    /// shadower must re-expose what it shadowed.
    context: RefCell<FxHashMap<CtxKey, Vec<CtxEntry>>>,
    /// Hands out [`CtxEntry::id`]. Monotonic per world; never reused, so an
    /// id identifies one provision for the world's whole life.
    next_ctx_id: Cell<u64>,
    flushing: Cell<bool>,
}

/// One `provide`d value.
///
/// `id` is what an owning scope retracts on teardown. It has to be an id
/// and not "the top of the stack": between a provision and its retraction,
/// other scopes may have provided the same type (shadowing) or the same
/// scope may have re-provided it, and a drop must remove exactly the entry
/// it created — never a stranger's.
struct CtxEntry {
    id: u64,
    value: Box<dyn Any>,
}

pub(crate) struct SignalSlot {
    /// Bumped on free. A handle is valid iff its gen matches AND `data` is
    /// occupied (an empty slot with a matching gen is mid-operation — its
    /// storage is temporarily moved out; see `with_signal_data`).
    gen: u32,
    /// Already in `staged`? Collapses N sets into one commit.
    queued: bool,
    /// A `set_always`/`touch` happened this staging window: commit must
    /// notify subscribers even if the value nets to "unchanged".
    forced: bool,
    /// Effects subscribed as of their latest run. Entries carry the effect's
    /// generation so a freed-and-reused effect slot can never be woken by a
    /// stale subscription (and `free_effect` eagerly unlinks, keeping lists
    /// tight).
    subscribers: Vec<EffectKey>,
    /// The typed value storage, `None` when the slot is free or mid-op.
    data: Option<Box<dyn AnySignal>>,
    /// Where this signal was created — the staged-read warning names it so
    /// the author can find the state, not just the read. A `SiteLoc`, so
    /// this field is a ZST (zero bytes, no initializer) in release.
    pub(crate) created_at: SiteLoc,
}


struct EffectSlot {
    gen: u32,
    /// `Rc` so the runner can clone the effect out and execute its body with
    /// zero arena borrows held (bodies read/write signals re-entrantly).
    data: Option<Rc<EffectData>>,
}



struct EffectData {
    /// Self-identity, for subscriber entries and liveness checks.
    slot: u32,
    gen: u32,
    class: EffectClass,
    /// The body. `None` once the effect is freed: `free_effect` takes it so
    /// the closure — and everything it captures — dies WITH THE SLOT, not
    /// whenever the last `Rc<EffectData>` lets go. A flush holds its own
    /// `Rc` for every queued effect until that effect's turn, so without
    /// the take a freed structural driver's closure kept its subtree's
    /// `Owned` alive for the rest of the flush; the subtree's effects still
    /// looked live to `effect_is_live`, ran, and read signals their parent
    /// scope had already freed (Wave-43 stale-handle crash —
    /// `regression_freed_effect_queued_in_same_flush_never_runs`).
    f: RefCell<Option<Box<dyn FnMut()>>>,
    /// Dedup flag: already collected into the current flush round?
    queued: Cell<bool>,
    /// Reverse edges: every `(signal_slot, signal_gen)` (of this effect's
    /// own world — subscriptions are always intra-world) subscribed as of
    /// the latest run. The INVARIANT is exact correspondence: this list is
    /// precisely the set of subscriber entries carrying this effect's key
    /// (modulo slots since freed, whose gen mismatch makes them inert).
    /// `reconcile_deps` diffs the next run's collected reads against it —
    /// identical sequences (stable deps) skip all subscriber-list work.
    /// The gen rides along so a reconcile after the body freed-and-reused
    /// a dep's slot can never subscribe to (or unsubscribe from) the
    /// slot's NEW occupant.
    deps: RefCell<Vec<(u32, u32)>>,
    /// Cleanups registered via `on_cleanup()` (or returned from the body)
    /// during the latest run. Run before the next re-run, or at teardown —
    /// whichever comes first.
    cleanups: RefCell<Vec<Box<dyn FnOnce()>>>,
}

fn run_cleanups(data: &EffectData) {
    // take() the Vec first so no borrow is held while user code runs.
    let cleanups = std::mem::take(&mut *data.cleanups.borrow_mut());
    for cleanup in cleanups {
        cleanup();
    }
}


// ============================================================================
// Slot allocation and freeing.
// ============================================================================

pub(crate) fn create_signal(arena: &Rc<WorldArena>, data: Box<dyn AnySignal>, site: SiteLoc) -> (u32, u32, bool) {
    let (slot, gen) = {
        let mut signals = arena.signals.borrow_mut();
        if let Some(slot) = arena.free_signals.borrow_mut().pop() {
            let s = &mut signals[slot as usize];
            debug_assert!(s.data.is_none(), "free-listed slot still occupied");
            s.data = Some(data);
            s.created_at = site;
            (slot, s.gen)
        } else {
            let slot = signals.len() as u32;
            signals.push(SignalSlot {
                gen: 0,
                queued: false,
                forced: false,
                subscribers: Vec::new(),
                data: Some(data),
                created_at: site,
            });
            (slot, 0)
        }
    };
    let collected = register_owned(OwnedItem::Signal { world: arena.id, slot, gen });
    (slot, gen, collected)
}

/// The value type of a live signal slot, without touching the value.
/// `None` when the slot is gone or the generation moved on.
#[cfg(feature = "hot-reload")]
pub(crate) fn signal_type_id(world: WorldId, slot: u32, gen: u32) -> Option<std::any::TypeId> {
    let arena = arena_of(world)?;
    let signals = arena.signals.borrow();
    let s = signals.get(slot as usize)?;
    if s.gen != gen {
        return None;
    }
    Some(s.data.as_ref()?.value_type_id())
}

/// Take a signal's payload box out of its arena, erased, and retire the
/// slot.
///
/// The dev-time hot-reload harvest ([`crate::hot_state::harvest`]) is the only
/// caller. It MOVES the value rather than cloning it, which is what
/// keeps `Clone` off `signal`'s bounds — the cost is that the slot is
/// finished afterwards, so the generation is bumped exactly as
/// `free_signal` would. Any handle still pointing here is stale from
/// this moment, and stale is the kernel's loud state, not its silent
/// one.
///
/// The index goes back on the free list, exactly as `free_signal` would
/// put it: the generation is already bumped, so a handle still pointing
/// here is stale and fails loudly rather than reading the next occupant.
/// Recycling matters since the web rebuild KEEPS its world (see
/// [`crate::hot_state::harvest_owned`]); a slot retired without it would leak
/// once per signal per patch. The owning scope's later `free_signal`
/// for the old generation finds the mismatch and does nothing.
#[cfg(feature = "hot-reload")]
pub(crate) fn steal_signal_data(
    world: WorldId,
    slot: u32,
    gen: u32,
) -> Option<(std::any::TypeId, Box<dyn Any>)> {
    let arena = arena_of(world)?;
    let data = {
        let mut signals = arena.signals.borrow_mut();
        let s = signals.get_mut(slot as usize)?;
        if s.gen != gen {
            return None;
        }
        let data = s.data.take()?;
        s.gen = s.gen.wrapping_add(1);
        s.queued = false;
        s.forced = false;
        s.subscribers.clear();
        arena.free_signals.borrow_mut().push(slot);
        data
    };
    let type_id = data.value_type_id();
    Some((type_id, data.into_any()))
}

pub(crate) fn create_effect(arena: &Rc<WorldArena>, class: EffectClass, f: Box<dyn FnMut()>) -> (u32, u32) {
    let (slot, gen, data) = {
        let mut effects = arena.effects.borrow_mut();
        let (slot, gen) = if let Some(slot) = arena.free_effects.borrow_mut().pop() {
            (slot, effects[slot as usize].gen)
        } else {
            let slot = effects.len() as u32;
            effects.push(EffectSlot { gen: 0, data: None });
            (slot, 0)
        };
        let data = Rc::new(EffectData {
            slot,
            gen,
            class,
            f: RefCell::new(Some(f)),
            queued: Cell::new(false),
            deps: RefCell::new(Vec::new()),
            cleanups: RefCell::new(Vec::new()),
        });
        effects[slot as usize].data = Some(Rc::clone(&data));
        (slot, gen, data)
    };
    // Register BEFORE the first run: if the body panics, the enclosing
    // collector's guard still frees this slot.
    register_owned(OwnedItem::Effect { world: arena.id, slot, gen });
    run_effect(arena, &data);
    (slot, gen)
}

/// Free one signal slot: bump its generation, clear staging metadata and
/// subscriptions, recycle the index. The value box is dropped OUTSIDE the
/// arena borrow (its `Drop` may touch other signals).
fn free_signal(arena: &WorldArena, slot: u32, gen: u32) {
    let data = {
        let mut signals = arena.signals.borrow_mut();
        let Some(s) = signals.get_mut(slot as usize) else { return };
        if s.gen != gen {
            return; // already freed (double-drop safety through gen check)
        }
        let Some(data) = s.data.take() else { return };
        s.gen = s.gen.wrapping_add(1);
        s.queued = false;
        s.forced = false;
        // Clear subscriptions: any subscriber still holding a reverse edge to
        // this slot will harmlessly retain-nothing when it later re-runs
        // (its (slot, gen) entry is gone, and the slot's next occupant starts
        // with a fresh list).
        s.subscribers.clear();
        arena.free_signals.borrow_mut().push(slot);
        data
    };
    drop(data);
}

/// Free one effect slot: unlink its subscriptions eagerly (keeps subscriber
/// lists tight), bump the generation, recycle the index, then run its
/// cleanups and drop its body with no arena borrows held.
fn free_effect(arena: &WorldArena, slot: u32, gen: u32) {
    let data = {
        let mut effects = arena.effects.borrow_mut();
        let Some(s) = effects.get_mut(slot as usize) else { return };
        if s.gen != gen {
            return;
        }
        let Some(data) = s.data.take() else { return };
        s.gen = s.gen.wrapping_add(1);
        arena.free_effects.borrow_mut().push(slot);
        data
    };
    {
        let deps: Vec<(u32, u32)> = data.deps.borrow_mut().drain(..).collect();
        let mut signals = arena.signals.borrow_mut();
        for (dep, dep_gen) in deps {
            if let Some(s) = signals.get_mut(dep as usize) {
                // Gen check: a since-freed-and-reused slot's NEW occupant
                // never held this subscription (free_signal cleared it).
                if s.gen == dep_gen {
                    s.subscribers.retain(|&(es, eg)| !(es == slot && eg == gen));
                }
            }
        }
    }
    run_cleanups(&data);
    // Drop the body NOW (see `EffectData::f`). Its captures' `Drop`s run
    // here — typically a nested scope's `Owned`, which frees that subtree's
    // effects before any of them can get a turn in the running flush. If
    // the body is executing (the effect is freeing itself from inside its
    // own run, e.g. a driver that drops the scope owning it), its `RefCell`
    // is borrowed; `run_effect` drops it as soon as that run returns.
    let body = data.f.try_borrow_mut().ok().and_then(|mut f| f.take());
    drop(body);
}


/// True when `(world, slot, gen)` still names a live signal. Never panics
/// and never subscribes — the non-committal form of the check
/// `with_signal_data` performs before it aborts.
fn signal_is_alive(world: WorldId, slot: u32, gen: u32) -> bool {
    // A slot whose `data` is temporarily `None` is mid-operation, not dead
    // (see `with_signal_data`), so the generation alone is the liveness
    // test — matching exactly what `stale_signal_panic` keys on.
    arena_of(world).is_some_and(|arena| {
        arena.signals.borrow().get(slot as usize).is_some_and(|s| s.gen == gen)
    })
}

/// How many effects are subscribed to `(world, slot, gen)` as of their
/// latest run; `0` for a dead handle. Never panics, never subscribes.
fn signal_subscriber_count(world: WorldId, slot: u32, gen: u32) -> usize {
    arena_of(world).map_or(0, |arena| {
        arena
            .signals
            .borrow()
            .get(slot as usize)
            .filter(|s| s.gen == gen)
            .map_or(0, |s| s.subscribers.len())
    })
}

/// True when `(world, slot, gen)` still names a live effect.
fn effect_is_alive(world: WorldId, slot: u32, gen: u32) -> bool {
    arena_of(world).is_some_and(|arena| {
        arena.effects.borrow().get(slot as usize).is_some_and(|e| e.gen == gen)
    })
}


/// A signal touched from inside its own get/with/set/update window — the
/// bridge's copy of the diagnostic `with_signal_data` expands inline (same
/// slug, same text; kept identical so `should_panic` tests hold on both
/// engines).
#[cfg_attr(not(any(test, all(feature = "bridge", any(not(target_arch = "wasm32"), idealyst_stream_guest)))), allow(dead_code))]
pub(crate) fn reentrant_signal_panic(world: WorldId, slot: u32) -> ! {
    diag_panic!(
        "reentrant-signal-read",
        "signal (world {}, slot {}) accessed re-entrantly while mid-operation: its \
         storage is moved out of the arena for the duration of its own \
         get/with/set/update, so an access reaching it from inside that window (e.g. \
         reading a signal from inside its own update() closure) finds the slot empty. \
         Read the value out first, then operate.",
        world,
        slot
    )
}

pub(crate) fn stale_signal_panic(world: WorldId, slot: u32) -> ! {
    diag_panic!(
        "stale-signal-handle",
        "signal used through a stale handle (world {}, slot {}): the Owned scope that \
         collected this signal/effect was dropped and its slot freed — a handle or closure \
         outlived its component scope. Hoist the state to a longer-lived scope (created \
         outside collect_owned it is world-root-owned), or keep the Owned alive as long as \
         the handle.",
        world,
        slot
    )
}

/// Move the slot's storage out, run `f` against it borrow-free, put it back.
/// Panics on stale handles; a slot whose storage is already out (gen matches
/// but data is `None`) gets the reentrancy diagnostic. If `f` itself frees
/// the slot (drops an `Owned`, drops the world), the put-back quietly drops
/// the box instead. If `f` panics, the box is lost and later access reports
/// reentrancy — acceptable for a poisoned tree.
#[inline]
fn with_signal_data<R>(
    arena: &WorldArena,
    world: WorldId,
    slot: u32,
    gen: u32,
    f: impl FnOnce(&mut dyn AnySignal) -> R,
) -> R {
    // Decide under the borrow, panic OUTSIDE it. Both diagnostics below are
    // fatal, and on a `panic = "abort"` target (wasm) there is no unwinding
    // to run the `RefCell` guard's `Drop` — so panicking with `signals`
    // still borrowed leaves the borrow flag set forever. Every later signal
    // operation in the module then dies with "already borrowed" from
    // whatever event or promise happens to touch the world next, burying
    // the one diagnostic that actually described the bug under a cascade of
    // ones that don't. Formatting the message needs the arena too, so the
    // panic has to happen after the guard is dropped, not merely on a
    // different line.
    enum Taken {
        Got(Box<dyn AnySignal>),
        Stale,
        Reentrant,
    }
    let taken = {
        let mut signals = arena.signals.borrow_mut();
        match signals.get_mut(slot as usize) {
            Some(s) if s.gen == gen => match s.data.take() {
                Some(b) => Taken::Got(b),
                None => Taken::Reentrant,
            },
            _ => Taken::Stale,
        }
    };
    let mut boxed: Box<dyn AnySignal> = match taken {
        Taken::Got(b) => b,
        Taken::Stale => stale_signal_panic(world, slot),
        // Expanded here rather than calling `reentrant_signal_panic` (which
        // says the same thing, for the bridge): outlining it measured +6-8%
        // on untracked reads at opt-level 3 without LTO (cargo's default
        // release profile) — it changed how this hot body was laid out.
        Taken::Reentrant => diag_panic!(
            "reentrant-signal-read",
            "signal (world {}, slot {}) accessed re-entrantly while mid-operation: its \
             storage is moved out of the arena for the duration of its own \
             get/with/set/update, so an access reaching it from inside that window (e.g. \
             reading a signal from inside its own update() closure) finds the slot empty. \
             Read the value out first, then operate.",
            world,
            slot
        ),
    };
    let result = f(&mut *boxed);
    let mut signals = arena.signals.borrow_mut();
    if let Some(s) = signals.get_mut(slot as usize) {
        if s.gen == gen && s.data.is_none() {
            s.data = Some(boxed);
        }
        // else: freed (or even reallocated) during f — drop the box.
    }
    result
}


/// Replace live slot `(world, slot, gen)`'s storage with `data`, returning
/// what it held — the bridge's PROMOTION of a bundle-owned value into a
/// native one (`remote::receive_signal`). Subscribers, queue state and
/// ownership stay with the slot. `Err(data)` when the slot is not live and
/// occupied. Not a hot path: bridge-only, once per promoted signal.
#[cfg(all(feature = "bridge", any(not(target_arch = "wasm32"), idealyst_stream_guest)))]
#[cfg_attr(idealyst_stream_guest, allow(dead_code))]
pub(crate) fn swap_signal_data(
    world: WorldId,
    slot: u32,
    gen: u32,
    data: Box<dyn AnySignal>,
) -> Result<Box<dyn AnySignal>, Box<dyn AnySignal>> {
    let Some(arena) = arena_of(world) else { return Err(data) };
    let mut signals = arena.signals.borrow_mut();
    match signals.get_mut(slot as usize) {
        Some(s) if s.gen == gen && s.data.is_some() => Ok(s.data.replace(data).expect("occupied")),
        _ => Err(data),
    }
}

/// Subscribe the innermost RUNNING effect to this signal — but only if that
/// effect lives in the signal's world.
///
/// Why "innermost running effect", not "the ambient world's current effect":
/// the running effect is a property of the thread's dynamic extent, not of
/// whichever world happens to be ambient (`enter` changes the CREATION
/// context only). And why intra-world only: a B-world effect reading an
/// A-world signal must create no subscription (the design's Weak<Scheduler>
/// reproduction) — A flushing later must not run B's effect, whose re-run is
/// B's flush's business. Cross-world dataflow is expressed by a B-effect
/// *writing* A-signals (stages into A), never by cross-world subscriptions.
/// Returns whether the read DID subscribe — i.e. whether a later commit of
/// this signal is guaranteed to re-deliver the value to this reader.
/// `read_signal` forks the staged-read diagnostic on it (a subscribed read
/// of a staged value self-corrects at the flush; an unsubscribed one is
/// final).
pub(crate) fn maybe_subscribe(world: WorldId, slot: u32, gen: u32) -> bool {
    // Record the read into the running effect's PENDING frame only — no
    // subscriber list is touched here. `reconcile_deps` (at the run's
    // end) turns the collected frame into subscription adds/removes by
    // diff; a dep set identical to the previous run's does zero work.
    // This also makes the tracked-read hot path allocation- and
    // arena-borrow-free (the old scheme paid an O(subscribers)
    // `contains` per first read of each signal).
    TLS.with(|t| {
        let mut t = t.borrow_mut();
        if t.untrack_depth > 0 {
            return false;
        }
        let Some(&(eworld, _eslot, _egen)) = t.effect_stack.last() else {
            return false;
        };
        if eworld != world {
            return false; // cross-world read: never subscribes
        }
        let frame = t
            .pending_deps
            .last_mut()
            .expect("pending frame exists whenever an effect is on the stack");
        // Same-run dedupe (the old subscribers.contains role) — frames
        // are small (an effect's distinct reads), so the scan is cheap.
        if !frame.contains(&(slot, gen)) {
            frame.push((slot, gen));
        }
        true
    })
}


pub(crate) fn enqueue(arena: &WorldArena, slot: u32, gen: u32, force: bool) {
    let mut signals = arena.signals.borrow_mut();
    let Some(s) = signals.get_mut(slot as usize) else { return };
    if s.gen != gen {
        return; // freed during the write's own closure — nothing to notify
    }
    if force {
        s.forced = true;
    }
    if !s.queued {
        s.queued = true;
        arena.staged.borrow_mut().push(slot);
    }
}


/// Run one effect: tear down the previous run (cleanups fire, subscriptions
/// drop via the reverse edges so this run re-collects exactly what it
/// reads), then execute the body with this effect on the running-effect
/// stack and its world entered (so the ambient API works inside bodies even
/// when re-run from flush, outside any user `enter`).
///
/// The body runs with `untrack_depth` reset to 0 (saved + restored): an
/// effect body is its own tracking context. `untrack` suspends tracking for
/// the *enclosing* code region, but an effect CREATED inside an untracked
/// region still runs its first body there — and that first run is the only
/// dependency-collection pass it gets. Without the reset, the P1 structural
/// drivers (which realize subtrees inside `untrack`, per the old walker's
/// `untrack_for_build` contract) would create nested driver/binding effects
/// that never subscribe anything and never re-fire. This is Solid/Leptos
/// semantics; regression: `effect_created_inside_untrack_still_tracks`.
fn run_effect(arena: &Rc<WorldArena>, data: &Rc<EffectData>) {
    run_cleanups(data);
    // NB: the previous subscriptions are deliberately NOT torn down here.
    // Reads collect into a fresh pending frame; `reconcile_deps` after the
    // body diffs it against the old set — identical deps (the stable-deps
    // steady state) touch no subscriber list at all. See `Tls::pending_deps`.
    let saved_untrack = with_tls(|t| {
        t.enter_stack.push(arena.id);
        t.effect_stack.push((arena.id, data.slot, data.gen));
        t.pending_deps.push(Vec::new());
        std::mem::replace(&mut t.untrack_depth, 0)
    });
    struct Guard {
        saved_untrack: u32,
        /// Set once the body returned normally; a panic unwind pops the
        /// frames and DISCARDS the pending deps — the old subscriptions
        /// stay installed and stay consistent with `EffectData::deps`.
        completed: bool,
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            if self.completed {
                return; // popped explicitly on the normal path
            }
            let saved = self.saved_untrack;
            let _ = TLS.try_with(|t| {
                let mut t = t.borrow_mut();
                t.enter_stack.pop();
                t.effect_stack.pop();
                t.pending_deps.pop();
                t.untrack_depth = saved;
            });
        }
    }
    let mut guard = Guard { saved_untrack, completed: false };
    // The body borrow is held across the run; a re-entrant run of the SAME
    // effect is impossible by construction (only flush runs effects, and a
    // world's flush cannot re-enter itself — see the reentrant-flush guard).
    //
    // A freed effect never gets here (`effect_is_live` gates every flush
    // run, and creation runs the body before anything can free it), so a
    // `None` body is unreachable in practice; skipping is the safe answer.
    {
        let mut body = data.f.borrow_mut();
        if let Some(f) = body.as_mut() {
            f();
        }
    }
    guard.completed = true;
    let new_deps = with_tls(|t| {
        t.enter_stack.pop();
        t.effect_stack.pop();
        t.untrack_depth = guard.saved_untrack;
        t.pending_deps.pop().expect("pending frame pushed above")
    });
    if !effect_is_live(arena, data) {
        // The body freed its own effect mid-run: `free_effect` could not
        // take the (then-borrowed) body, so release it here, and do not
        // subscribe a dead effect to what this last run read.
        let body = data.f.borrow_mut().take();
        drop(body);
        return;
    }
    reconcile_deps(arena, data, new_deps);
}

/// Swap an effect's dependency set to `new_deps`, adjusting subscriber
/// lists by DIFF. The fast path — `new_deps` identical to the previous
/// run's (stable closures read the same signals in the same order) —
/// does nothing. Otherwise: entries only in the old set unsubscribe
/// (`retain`), entries only in the new set subscribe (plain `push` — the
/// `deps` invariant guarantees the effect isn't already in those lists,
/// so no O(subscribers) `contains` scan is needed).
fn reconcile_deps(arena: &WorldArena, data: &EffectData, new_deps: Vec<(u32, u32)>) {
    let mut old = data.deps.borrow_mut();
    if *old == new_deps {
        return;
    }
    let mut signals = arena.signals.borrow_mut();
    for &(slot, dep_gen) in old.iter() {
        if !new_deps.contains(&(slot, dep_gen)) {
            if let Some(s) = signals.get_mut(slot as usize) {
                if s.gen == dep_gen {
                    s.subscribers
                        .retain(|&(es, eg)| !(es == data.slot && eg == data.gen));
                }
            }
        }
    }
    for &(slot, dep_gen) in new_deps.iter() {
        if !old.contains(&(slot, dep_gen)) {
            if let Some(s) = signals.get_mut(slot as usize) {
                // Gen + occupancy check: the body may have freed (and a
                // successor reused) a slot AFTER reading it — never
                // subscribe to the new occupant.
                if s.gen == dep_gen && s.data.is_some() {
                    s.subscribers.push((data.slot, data.gen));
                }
            }
        }
    }
    *old = new_deps;
}


/// Push `value` into the ambient world's context and return its entry id,
/// which is what an owning scope later retracts.
pub(crate) fn push_context(key: CtxKey, value: Box<dyn Any>) -> (WorldId, CtxKey, u64) {
    with_ambient(|arena| {
        let id = arena.next_ctx_id.get();
        arena.next_ctx_id.set(id.wrapping_add(1));
        arena
            .context
            .borrow_mut()
            .entry(key)
            .or_default()
            .push(CtxEntry { id, value });
        (arena.id, key, id)
    })
}

/// Remove the context entry `id` (a no-op if already gone — double
/// retraction is safe). The boxed value is dropped OUTSIDE the arena
/// borrow: it may hold signal handles whose `Drop` re-enters the world,
/// the same discipline `free_signal` follows.
fn retract_context(arena: &WorldArena, key: CtxKey, id: u64) {
    let removed = {
        let mut ctx = arena.context.borrow_mut();
        let Some(stack) = ctx.get_mut(&key) else { return };
        let Some(pos) = stack.iter().position(|e| e.id == id) else { return };
        let entry = stack.remove(pos);
        if stack.is_empty() {
            ctx.remove(&key);
        }
        entry.value
    };
    drop(removed);
}


// ============================================================================
// Flush — commit staged writes, run Derivations to settlement, then
// Reactions once each. See World::flush for the full algorithm docs.
// ============================================================================

/// Safety valve for cyclic updates (effect A sets a signal effect B reads,
/// B sets a signal A reads, values never settle).
const FLUSH_ROUND_LIMIT: usize = 100;

fn flush_arena(arena: &Rc<WorldArena>) {
    if arena.flushing.get() {
        diag_panic!(
            "reentrant-flush",
            "World::flush called re-entrantly from inside one of this world's own effects. \
             Writes made during a flush are staged and committed by the SAME flush's next \
             round — never call flush from effect bodies. (Flushing a DIFFERENT world from \
             an effect is fine; worlds are independent.)"
        );
    }
    arena.flushing.set(true);
    let _ = TLS.try_with(|t| t.borrow_mut().flush_depth += 1);
    struct Guard(Rc<WorldArena>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.flushing.set(false);
            let _ = TLS.try_with(|t| t.borrow_mut().flush_depth -= 1);
        }
    }
    let _guard = Guard(Rc::clone(arena));

    let mut rounds = 0usize;
    // Reactions dirtied during derivation settling are HELD here (their
    // `queued` flag stays set, deduplicating across commits) and released
    // only once no Derivation is dirty.
    let mut reactions: Vec<Rc<EffectData>> = Vec::new();
    loop {
        // Inner loop: settle every Derivation. A derivation's writes stage;
        // the next iteration commits them, possibly dirtying further
        // derivations (memo-of-memo chains settle here, in dependency
        // order by construction: a derivation only becomes dirty after its
        // input committed).
        loop {
            rounds += 1;
            if rounds > FLUSH_ROUND_LIMIT {
                panic!(
                    "flush did not settle after {FLUSH_ROUND_LIMIT} rounds — cyclic signal updates?"
                );
            }
            let dirty = commit_staged(arena);
            let mut derivations = Vec::new();
            for e in dirty {
                match e.class {
                    EffectClass::Derivation => derivations.push(e),
                    EffectClass::Reaction => reactions.push(e),
                }
            }
            if derivations.is_empty() {
                break;
            }
            for d in derivations {
                d.queued.set(false);
                if effect_is_live(arena, &d) {
                    run_effect(arena, &d);
                }
            }
        }
        if reactions.is_empty() {
            return; // settled: nothing staged, nothing to run
        }
        // Every derivation has settled: run each held Reaction exactly once
        // against the fully consistent snapshot. Their writes stage for the
        // next outer round.
        for r in std::mem::take(&mut reactions) {
            r.queued.set(false);
            if effect_is_live(arena, &r) {
                run_effect(arena, &r);
            }
        }
    }
}

/// Drain the staged queue, commit each signal, and collect the deduplicated
/// dirty effects. No user code runs here except `T: PartialEq` and old-value
/// `Drop` — both executed with the storage box moved out of the arena.
fn commit_staged(arena: &Rc<WorldArena>) -> Vec<Rc<EffectData>> {
    let staged: Vec<u32> = std::mem::take(&mut *arena.staged.borrow_mut());
    let mut dirty: Vec<Rc<EffectData>> = Vec::new();
    for slot in staged {
        // Phase 1: pull staging metadata + the storage box out.
        let (mut data, gen, forced) = {
            let mut signals = arena.signals.borrow_mut();
            let Some(s) = signals.get_mut(slot as usize) else { continue };
            s.queued = false;
            let forced = std::mem::take(&mut s.forced);
            match s.data.take() {
                Some(d) => (d, s.gen, forced),
                None => continue, // freed while staged: nothing to commit
            }
        };
        // Phase 2: commit borrow-free (runs T::eq; drops the old value).
        let changed = data.commit(forced);
        // Phase 3: put the box back and snapshot subscribers if notifying.
        // The gen guard covers the pathological case of the slot being freed
        // by a T::eq / Drop side effect during phase 2.
        let subs: Vec<EffectKey> = {
            let mut signals = arena.signals.borrow_mut();
            match signals.get_mut(slot as usize) {
                Some(s) if s.gen == gen && s.data.is_none() => {
                    s.data = Some(data);
                    if changed { s.subscribers.clone() } else { Vec::new() }
                }
                _ => Vec::new(),
            }
        };
        for (eslot, egen) in subs {
            let ed = {
                let effects = arena.effects.borrow();
                effects
                    .get(eslot as usize)
                    .and_then(|e| if e.gen == egen { e.data.clone() } else { None })
            };
            if let Some(ed) = ed {
                if !ed.queued.get() {
                    ed.queued.set(true);
                    dirty.push(ed);
                }
            }
        }
    }
    dirty
}

/// Is this effect's slot still occupied by this exact effect? Guards against
/// running an effect that was freed (its Owned dropped) after being
/// collected as dirty but before its turn.
fn effect_is_live(arena: &WorldArena, data: &EffectData) -> bool {
    arena
        .effects
        .borrow()
        .get(data.slot as usize)
        .is_some_and(|s| s.gen == data.gen && s.data.is_some())
}


#[derive(Clone, Copy)]
pub(crate) enum OwnedItem {
    Signal { world: WorldId, slot: u32, gen: u32 },
    Effect { world: WorldId, slot: u32, gen: u32 },
    /// A `provide`d context entry (see [`provide`]). Retracted on drop so
    /// a provision cannot outlive the scope-owned handles it carries.
    Context { world: WorldId, key: CtxKey, id: u64 },
}


/// Returns whether a collector took the item. `false` means world-root:
/// freed only when the world drops.
pub(crate) fn register_owned(item: OwnedItem) -> bool {
    with_tls(|t| {
        if let Some(top) = t.collectors.last_mut() {
            top.push(item);
            true
        } else {
            // No collector: world-root-owned; freed when the world drops.
            false
        }
    })
}

pub(crate) fn collect_begin() {
    with_tls(|t| t.collectors.push(Vec::new()));
}

/// Pop the innermost collector. `None` only when the TLS is gone.
pub(crate) fn collect_end() -> Option<Vec<OwnedItem>> {
    TLS.try_with(|t| t.borrow_mut().collectors.pop()).ok().flatten()
}

/// Run `f`, collecting every signal and effect it creates (in any world)
/// into the returned items. Collectors stack: an inner collection's
/// creations belong to the INNER scope only. If `f` panics, everything
/// collected so far is freed before the panic propagates.
fn collect<R>(f: impl FnOnce() -> R) -> (R, Vec<OwnedItem>) {
    collect_begin();
    struct Guard {
        armed: bool,
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            if self.armed {
                // Panic path: pop the collector and free what it gathered.
                drop_items(collect_end().unwrap_or_default());
            }
        }
    }
    let mut guard = Guard { armed: true };
    let result = f();
    guard.armed = false;
    let items = collect_end().expect("collector stack imbalance");
    (result, items)
}

/// Tear down a scope's items: retract context, free effects (cleanups
/// first), then free signals. Items whose world is already dead are skipped
/// (the world drop already tore them down).
pub(crate) fn drop_items(items: Vec<OwnedItem>) {
    // Context entries first, BEFORE any slot is freed: an effect
    // cleanup (next pass) that injects must not be handed a provision
    // whose signals this same drop is about to free. Retracted first,
    // it gets `None` — or the outer scope's still-live provision — and
    // can act on that. Retracted last, it would read freed slots.
    for item in &items {
        if let OwnedItem::Context { world, key, id } = *item {
            if let Some(arena) = arena_of(world) {
                retract_context(&arena, key, id);
            }
        }
    }
    // Effects next, in creation order: every cleanup can still read the
    // scope's sibling signals (freed only in the final pass).
    for item in &items {
        if let OwnedItem::Effect { world, slot, gen } = *item {
            if let Some(arena) = arena_of(world) {
                free_effect(&arena, slot, gen);
            }
        }
    }
    for item in &items {
        if let OwnedItem::Signal { world, slot, gen } = *item {
            if let Some(arena) = arena_of(world) {
                free_signal(&arena, slot, gen);
            }
        }
    }
}


// ============================================================================
// The parts of the public API whose bodies were engine work, and the
// `Engine` impl over all of the above.
// ============================================================================

fn world_new() -> WorldId {
    with_tls(|t| {
        let id = t.next_world;
        t.next_world += 1;
        let arena = Rc::new(WorldArena {
            id,
            signals: RefCell::new(Vec::new()),
            free_signals: RefCell::new(Vec::new()),
            effects: RefCell::new(Vec::new()),
            free_effects: RefCell::new(Vec::new()),
            staged: RefCell::new(Vec::new()),
            context: RefCell::new(FxHashMap::default()),
            next_ctx_id: Cell::new(0),
            flushing: Cell::new(false),
        });
        t.worlds.insert(id, arena);
        id
    })
}

fn world_drop(world: WorldId) {
    let Some(arena) = arena_of(world) else { return };
    // Run every live effect's cleanups while the world is STILL
    // registered, so cleanup code can read/write sibling signals (writes
    // stage into a queue that will simply never flush). Only then remove
    // the registry entry — after that, handle reads see "dead world".
    let datas: Vec<Rc<EffectData>> = arena
        .effects
        .borrow()
        .iter()
        .filter_map(|slot| slot.data.clone())
        .collect();
    for data in datas {
        run_cleanups(&data);
    }
    // try_with: this drop may run during thread TLS teardown, where
    // touching a destroyed TLS key would abort (see the runtime-core
    // thread-death guards this mirrors).
    // The registry entry and the cache entry are taken out under the
    // borrow and dropped after it: they are clones of `arena` (still held
    // here), so nothing frees yet, but the discipline is the kernel's —
    // no Drop runs while the TLS borrow is held.
    let removed = TLS
        .try_with(|t| {
            let t = &mut *t.borrow_mut();
            let entry = t.worlds.remove(&world);
            let cached = if t.cached_id.get() == world {
                t.cached_id.set(0);
                t.cached_arena.borrow_mut().take()
            } else {
                None
            };
            (entry, cached)
        })
        .ok();
    drop(removed);
}

// Each region guard is a begin/end pair with a closure form built on it.
// The closure forms are what the typed layer uses; the pairs exist for the
// bridge, whose closures live on the far side of a boundary that only plain
// values cross. Ends tolerate a torn-down TLS (`try_with`) because they run
// from guards that may fire during thread teardown.

pub(crate) fn enter_push(world: WorldId) {
    with_tls(|t| t.enter_stack.push(world));
}

pub(crate) fn enter_pop() {
    let _ = TLS.try_with(|t| {
        t.borrow_mut().enter_stack.pop();
    });
}

fn world_enter<R>(world: WorldId, f: impl FnOnce() -> R) -> R {
    enter_push(world);
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            enter_pop();
        }
    }
    let _guard = Guard;
    f()
}

pub(crate) fn untrack_push() {
    with_tls(|t| t.untrack_depth += 1);
}

#[cfg(test)]
pub(crate) fn untrack_depth() -> u32 {
    with_tls(|t| t.untrack_depth)
}

pub(crate) fn untrack_pop() {
    let _ = TLS.try_with(|t| {
        t.borrow_mut().untrack_depth -= 1;
    });
}

fn untrack<R>(f: impl FnOnce() -> R) -> R {
    untrack_push();
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            untrack_pop();
        }
    }
    let _guard = Guard;
    f()
}

/// The collector stack an `unscoped` region suspended.
pub(crate) type SavedCollectors = Vec<Vec<OwnedItem>>;

pub(crate) fn unscoped_begin() -> SavedCollectors {
    with_tls(|t| std::mem::take(&mut t.collectors))
}

pub(crate) fn unscoped_end(saved: SavedCollectors) {
    let _ = TLS.try_with(|t| {
        let mut t = t.borrow_mut();
        // Anything the suspended region pushed (it starts empty, so
        // only a nested `collect_owned` that itself panicked could
        // leave residue) is world-root-owned by construction; the
        // outer stack is restored verbatim.
        debug_assert!(
            t.collectors.is_empty(),
            "unscoped: collector stack not empty on exit — a nested \
             collect_owned failed to pop"
        );
        t.collectors = saved;
    });
}

fn unscoped<R>(f: impl FnOnce() -> R) -> R {
    // Tell the hot-reload state carrier this region creates
    // world-lifetime services, not component state. See the typed
    // layer's `signal_in`.
    #[cfg(feature = "hot-reload")]
    let _world_lifetime = crate::hot_state::WorldLifetimeRegion::enter();
    struct Guard {
        saved: SavedCollectors,
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            unscoped_end(std::mem::take(&mut self.saved));
        }
    }
    let _guard = Guard { saved: unscoped_begin() };
    f()
}

/// The running-effect stack and its parallel pending-dependency frames —
/// what `unanchored` suspends and restores as one unit.
pub(crate) type EffectFrames = (Vec<(WorldId, u32, u32)>, Vec<Vec<(u32, u32)>>);

pub(crate) fn unanchored_begin() -> EffectFrames {
    with_tls(|t| (std::mem::take(&mut t.effect_stack), std::mem::take(&mut t.pending_deps)))
}

pub(crate) fn unanchored_end(saved: EffectFrames) {
    let (effects, pending) = saved;
    let _ = TLS.try_with(|t| {
        let mut t = t.borrow_mut();
        // Anything the suspended region pushed is popped by the
        // same `run_effect` that pushed it; the outer stacks are
        // restored verbatim.
        debug_assert!(
            t.effect_stack.is_empty() && t.pending_deps.is_empty(),
            "unanchored: effect stack not empty on exit — a nested \
             effect run failed to pop"
        );
        t.effect_stack = effects;
        t.pending_deps = pending;
    });
}

fn unanchored<R>(f: impl FnOnce() -> R) -> R {
    struct Guard {
        saved: EffectFrames,
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            unanchored_end(std::mem::take(&mut self.saved));
        }
    }
    let _guard = Guard { saved: unanchored_begin() };
    f()
}

pub(crate) fn on_cleanup(f: Box<dyn FnOnce()>) {
    let top = TLS.with(|t| t.borrow().effect_stack.last().copied());
    let Some((world, slot, gen)) = top else {
        // Keep the leading sentence stable: docs and downstream comments
        // quote it. The rest names the fix for the everyday way to get
        // here — a `#[component]` body or mount handler, which never runs
        // inside an effect.
        panic!(
            "on_cleanup called outside an effect. It registers on the innermost \
             RUNNING effect, and component bodies, mount handlers and the \
             initial realize are not effect bodies. For teardown when a component \
             or scope unmounts, use `on_scope_drop(f)` instead; inside an effect, \
             `on_cleanup` (or returning the cleanup closure) is correct."
        );
    };
    let Some(arena) = arena_of(world) else { return };
    let data = {
        let effects = arena.effects.borrow();
        effects
            .get(slot as usize)
            .and_then(|e| if e.gen == gen { e.data.clone() } else { None })
    };
    if let Some(data) = data {
        data.cleanups.borrow_mut().push(f);
    }
}

/// `World::provide`'s body, for any key.
pub(crate) fn world_provide(world: WorldId, key: CtxKey, value: Box<dyn Any>) {
    let Some(arena) = arena_of(world) else { return };
    let id = arena.next_ctx_id.get();
    arena.next_ctx_id.set(id.wrapping_add(1));
    arena.context.borrow_mut().entry(key).or_default().push(CtxEntry { id, value });
}

/// `provide`'s body, for any key: push onto the ambient world's stack,
/// owned by the innermost collector.
pub(crate) fn ctx_provide(key: CtxKey, value: Box<dyn Any>) {
    let (world, key, id) = push_context(key, value);
    register_owned(OwnedItem::Context { world, key, id });
}

pub(crate) fn context_top<R>(arena: &WorldArena, key: CtxKey, f: impl FnOnce(&dyn Any) -> R) -> Option<R> {
    let ctx = arena.context.borrow();
    let entry = ctx.get(&key).and_then(|stack| stack.last())?;
    Some(f(&*entry.value))
}

impl Engine for Native {
    type Scope = Vec<OwnedItem>;

    #[inline]
    fn world_new() -> WorldId {
        world_new()
    }
    #[inline]
    fn world_drop(world: WorldId) {
        world_drop(world)
    }
    #[inline]
    fn world_enter<R>(world: WorldId, f: impl FnOnce() -> R) -> R {
        world_enter(world, f)
    }
    #[inline]
    fn world_flush(world: WorldId) {
        if let Some(arena) = arena_of(world) {
            flush_arena(&arena);
        }
    }
    #[inline]
    fn world_is_flushing(world: WorldId) -> bool {
        arena_of(world).is_some_and(|a| a.flushing.get())
    }
    #[inline]
    fn world_provide(world: WorldId, key: TypeId, value: Box<dyn Any>) {
        world_provide(world, CtxKey::Type(key), value)
    }
    #[inline]
    fn world_inject<R>(world: WorldId, key: TypeId, f: impl FnOnce(&dyn Any) -> R) -> Option<R> {
        let arena = arena_of(world)?;
        context_top(&arena, CtxKey::Type(key), f)
    }

    #[inline]
    fn is_flushing() -> bool {
        TLS.try_with(|t| t.borrow().flush_depth > 0).unwrap_or(false)
    }
    #[inline]
    fn is_entered() -> bool {
        TLS.try_with(|t| {
            let t = t.borrow();
            t.enter_stack.last().map(|id| t.worlds.contains_key(id)).unwrap_or(false)
        })
        .unwrap_or(false)
    }
    #[inline]
    fn in_effect() -> bool {
        TLS.try_with(|t| !t.borrow().effect_stack.is_empty()).unwrap_or(false)
    }
    #[inline]
    fn effect_depth() -> usize {
        TLS.try_with(|t| t.borrow().effect_stack.len()).unwrap_or(0)
    }
    #[inline]
    fn current_effect() -> Option<(WorldId, u32, u32)> {
        TLS.try_with(|t| t.borrow().effect_stack.last().copied()).ok()?
    }
    #[inline]
    fn in_collector() -> bool {
        TLS.try_with(|t| !t.borrow().collectors.is_empty()).unwrap_or(false)
    }

    #[inline]
    fn signal_create(world: Option<WorldId>, data: Box<dyn AnySignal>, site: SiteLoc) -> (WorldId, u32, u32, bool) {
        let create = |arena: &Rc<WorldArena>| {
            let (slot, gen, collected) = create_signal(arena, data, site);
            (arena.id, slot, gen, collected)
        };
        match world {
            None => with_ambient(create),
            Some(world) => match arena_of(world) {
                Some(arena) => create(&arena),
                None => panic!("runtime-world: signal created in a dead world {world}"),
            },
        }
    }
    #[inline]
    fn signal_access<R>(
        world: WorldId,
        slot: u32,
        gen: u32,
        f: impl FnOnce(&mut dyn AnySignal) -> R,
    ) -> Access<R> {
        match arena_of(world) {
            Some(arena) => Access::Done(with_signal_data(&arena, world, slot, gen, f)),
            None => Access::DeadWorld,
        }
    }
    #[inline]
    fn signal_read<R>(
        world: WorldId,
        slot: u32,
        gen: u32,
        track: bool,
        f: impl FnOnce(&mut dyn AnySignal, bool) -> R,
    ) -> Access<R> {
        // The pre-seam order: resolve the arena, subscribe, access.
        let Some(arena) = arena_of(world) else { return Access::DeadWorld };
        let subscribed = track && maybe_subscribe(world, slot, gen);
        Access::Done(with_signal_data(&arena, world, slot, gen, |d| f(d, subscribed)))
    }
    #[inline]
    fn signal_write<R>(
        world: WorldId,
        slot: u32,
        gen: u32,
        force: bool,
        f: impl FnOnce(&mut dyn AnySignal) -> R,
    ) -> Access<R> {
        // The pre-seam shape: resolve once, write, enqueue.
        let Some(arena) = arena_of(world) else { return Access::DeadWorld };
        let r = with_signal_data(&arena, world, slot, gen, f);
        enqueue(&arena, slot, gen, force);
        Access::Done(r)
    }
    #[inline]
    fn signal_touch(world: WorldId, slot: u32, gen: u32) {
        let Some(arena) = arena_of(world) else { return };
        let live = arena.signals.borrow().get(slot as usize).is_some_and(|s| s.gen == gen);
        if !live {
            stale_signal_panic(world, slot);
        }
        enqueue(&arena, slot, gen, true);
    }
    #[inline]
    fn signal_is_alive(world: WorldId, slot: u32, gen: u32) -> bool {
        signal_is_alive(world, slot, gen)
    }
    #[inline]
    fn signal_subscriber_count(world: WorldId, slot: u32, gen: u32) -> usize {
        signal_subscriber_count(world, slot, gen)
    }
    #[cfg(debug_assertions)]
    #[inline]
    fn signal_created_at(world: WorldId, slot: u32) -> Option<SiteLoc> {
        let arena = arena_of(world)?;
        let signals = arena.signals.borrow();
        signals.get(slot as usize).map(|s| s.created_at)
    }

    #[inline]
    fn effect_create(world: Option<WorldId>, class: EffectClass, body: Box<dyn FnMut()>) -> (WorldId, u32, u32) {
        let create = |arena: &Rc<WorldArena>| {
            let (slot, gen) = create_effect(arena, class, body);
            (arena.id, slot, gen)
        };
        match world {
            None => with_ambient(create),
            Some(world) => match arena_of(world) {
                Some(arena) => create(&arena),
                None => panic!("runtime-world: effect created in a dead world {world}"),
            },
        }
    }
    #[inline]
    fn effect_is_alive(world: WorldId, slot: u32, gen: u32) -> bool {
        effect_is_alive(world, slot, gen)
    }
    #[inline]
    fn on_cleanup(f: Box<dyn FnOnce()>) {
        on_cleanup(f)
    }

    #[inline]
    fn untrack<R>(f: impl FnOnce() -> R) -> R {
        untrack(f)
    }
    #[inline]
    fn unscoped<R>(f: impl FnOnce() -> R) -> R {
        unscoped(f)
    }
    #[inline]
    fn unanchored<R>(f: impl FnOnce() -> R) -> R {
        unanchored(f)
    }

    #[inline]
    fn collect<R>(f: impl FnOnce() -> R) -> (R, Self::Scope) {
        collect(f)
    }
    #[inline]
    fn scope_merge(into: &mut Self::Scope, mut other: Self::Scope) {
        into.append(&mut other);
    }
    #[inline]
    fn scope_len(scope: &Self::Scope) -> usize {
        scope.len()
    }
    #[inline]
    fn scope_drop(scope: &mut Self::Scope) {
        drop_items(std::mem::take(scope));
    }

    #[inline]
    fn ctx_provide(key: TypeId, value: Box<dyn Any>) {
        ctx_provide(CtxKey::Type(key), value)
    }
    #[inline]
    fn ctx_inject<R>(key: TypeId, f: impl FnOnce(&dyn Any) -> R) -> Option<R> {
        with_ambient(|arena| context_top(arena, CtxKey::Type(key), f))
    }
}
