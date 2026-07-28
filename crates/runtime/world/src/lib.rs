//! runtime-world — the reactive kernel for the idea-lite core migration (P0).
//!
//! Core model (idea-lite's three axioms, on arena storage):
//! - **Signals stage writes** (`set`/`update`) and commit them in batches at
//!   [`World::flush`]; reads (`get`/`with`) always see the committed snapshot.
//! - **Effects auto-track** the signals they read and re-run when those
//!   effectively change. Cleanups run before each re-run and at teardown.
//! - **Dropping is the entire teardown story**: slots created inside
//!   [`collect_owned`] are freed when the returned [`Owned`] drops; everything
//!   else is freed when its [`World`] drops.
//!
//! Storage model — per-world arenas, Copy handles:
//! - Each [`World`] owns an arena of generational signal/effect slots. Handles
//!   ([`Signal`], [`ReadSignal`], [`WriteSignal`], [`Effect`], [`Memo`]) are
//!   `Copy` triples `(world id, slot, generation)` — the authored tree passes
//!   them around with zero clone ceremony.
//! - ONE `thread_local!` total holds the registry `world id → arena` plus all
//!   transient stacks (ambient enter stack, running-effect stack, ownership
//!   collectors). A single TLS key matters on Android, where bionic caps
//!   pthread TLS keys at 128 (see the runtime-core stylesheet-registry
//!   incident).
//! - Handles route to their OWN world through the registry at use time, not
//!   the ambient world: writing an A-world signal from inside a B-world
//!   effect stages into A's queue; A and B flush independently.
//! - Slots are **generational**: freeing a slot bumps its generation, so a
//!   stale `Copy` handle (its `Owned` scope dropped) can never alias the
//!   slot's next occupant — it panics with a diagnostic instead. Generations
//!   are `u32` and wrap; ABA after 2^32 reuses of one slot is accepted.
//!
//! Flush model — derivation-class, glitch-free (the P0 centerpiece): see
//! [`World::flush`] for the algorithm and its glitch-freedom argument.

use std::any::{Any, TypeId};
use std::cell::{Cell, RefCell};
use std::marker::PhantomData;
use std::rc::Rc;

use rustc_hash::FxHashMap;

#[cfg(test)]
mod tests;

/// Panic with an actionable diagnostic in dev builds and a terse stable code
/// in release builds (same convention as runtime-core: long prose ships in
/// wasm rodata, so it is compiled out of release; the slug keeps the failure
/// greppable and `#[should_panic(expected = "idealyst[<slug>]")]` stable in
/// both modes).
macro_rules! diag_panic {
    ($code:literal, $fmt:literal $(, $arg:expr)* $(,)?) => {{
        #[cfg(debug_assertions)]
        { panic!(concat!("idealyst[", $code, "]: ", $fmt) $(, $arg)*) }
        #[cfg(not(debug_assertions))]
        {
            $(let _ = &$arg;)* // args feed the dev-only prose; consume them here
            panic!(concat!("idealyst[", $code, "] (debug build has details)"))
        }
    }};
}

// ============================================================================
// The one thread-local — registry of live worlds + every transient stack.
// Consulted at handle use time (routing) and at creation time (ambient).
// Nothing here owns reactive state except the registry's arena Rcs; the
// stacks are empty at rest.
// ============================================================================

/// Non-zero id of a live (or dead) world. Baked into every handle.
type WorldId = u32;

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
}

thread_local! {
    static TLS: RefCell<Tls> = RefCell::new(Tls {
        worlds: FxHashMap::default(),
        next_world: 1,
        enter_stack: Vec::new(),
        effect_stack: Vec::new(),
        collectors: Vec::new(),
        untrack_depth: 0,
        flush_depth: 0,
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
fn arena_of(world: WorldId) -> Option<Rc<WorldArena>> {
    TLS.try_with(|t| t.borrow().worlds.get(&world).cloned())
        .ok()
        .flatten()
}

/// Run `f` against the ambient world's arena, or panic with the canonical
/// "outside enter" message. Only *creation*-side APIs (free `signal()`,
/// `effect()`, `provide()`, …) consult the ambient world; handle *use*
/// routes through the handle's own world id.
fn with_ambient<R>(f: impl FnOnce(&Rc<WorldArena>) -> R) -> R {
    let arena = TLS.with(|t| {
        let t = t.borrow();
        t.enter_stack.last().and_then(|id| t.worlds.get(id).cloned())
    });
    match arena {
        Some(arena) => f(&arena),
        None => panic!(
            "signal()/effect() called outside World::enter — components must run inside a reactive context"
        ),
    }
}

// ============================================================================
// World — a discrete reactive runtime. A cloneable value; the LAST clone's
// drop tears the world down (cleanups run, registry entry removed, arena
// freed). Many worlds coexist on a thread and flush independently.
// ============================================================================

/// A reactive world. Create with [`World::new`], make it ambient with
/// [`World::enter`], commit staged writes with [`World::flush`]. Cheap to
/// clone (handle semantics); dropping the last clone tears the world down.
pub struct World {
    core: Rc<WorldCore>,
}

impl Clone for World {
    fn clone(&self) -> Self {
        World { core: Rc::clone(&self.core) }
    }
}

struct WorldCore {
    id: WorldId,
    arena: Rc<WorldArena>,
}

impl Drop for WorldCore {
    fn drop(&mut self) {
        // Run every live effect's cleanups while the world is STILL
        // registered, so cleanup code can read/write sibling signals (writes
        // stage into a queue that will simply never flush). Only then remove
        // the registry entry — after that, handle reads see "dead world".
        let datas: Vec<Rc<EffectData>> = self
            .arena
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
        let _ = TLS.try_with(|t| {
            t.borrow_mut().worlds.remove(&self.id);
        });
    }
}

impl Default for World {
    fn default() -> Self {
        World::new()
    }
}

impl World {
    /// Create a new, empty world and register it in this thread's registry.
    pub fn new() -> World {
        let (id, arena) = with_tls(|t| {
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
                flushing: Cell::new(false),
            });
            t.worlds.insert(id, Rc::clone(&arena));
            (id, arena)
        });
        World { core: Rc::new(WorldCore { id, arena }) }
    }

    /// This world's registry id (diagnostic use).
    pub fn id(&self) -> u32 {
        self.core.id
    }

    /// Run `f` with this world as the ambient creation context, so plain
    /// functions can call the free [`signal()`]/[`effect()`] without ever
    /// seeing a world. This is what the framework wraps component calls in.
    /// A stack — worlds nest, and the previous ambient world is restored on
    /// exit (panic-safe).
    pub fn enter<R>(&self, f: impl FnOnce() -> R) -> R {
        with_tls(|t| t.enter_stack.push(self.core.id));
        struct Guard;
        impl Drop for Guard {
            fn drop(&mut self) {
                let _ = TLS.try_with(|t| {
                    t.borrow_mut().enter_stack.pop();
                });
            }
        }
        let _guard = Guard;
        f()
    }

    /// Create a signal belonging to this world (regardless of the ambient
    /// world). See the free [`signal()`] for the ambient form.
    pub fn signal<T: PartialEq + 'static>(&self, value: T) -> Signal<T> {
        create_signal(&self.core.arena, value)
    }

    /// Create an effect in this world and run it once immediately to collect
    /// dependencies. See the free [`effect()`] for the ambient form.
    pub fn effect<C: IntoCleanup>(&self, f: impl FnMut() -> C + 'static) -> Effect {
        create_effect(&self.core.arena, EffectClass::Reaction, wrap_effect_body(f))
    }

    /// Create a memo in this world. See the free [`memo()`].
    pub fn memo<T, F>(&self, f: F) -> Memo<T>
    where
        T: PartialEq + Clone + 'static,
        F: Fn() -> T + 'static,
    {
        self.enter(|| memo(f))
    }

    /// Store a value in this world's context, keyed by its type. Use newtype
    /// wrappers to distinguish values of the same underlying type.
    pub fn provide<T: Clone + 'static>(&self, value: T) {
        self.core
            .arena
            .context
            .borrow_mut()
            .insert(TypeId::of::<T>(), Box::new(value));
    }

    /// Fetch a value from this world's context by type. Untracked — this is
    /// delivery, not reactivity; put a `Signal` inside the value for that.
    pub fn inject<T: Clone + 'static>(&self) -> Option<T> {
        self.core
            .arena
            .context
            .borrow()
            .get(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast_ref::<T>())
            .cloned()
    }

    /// Commit all staged writes and run affected effects, glitch-free.
    ///
    /// Effects carry a class: **Derivation** (memo recomputes) or
    /// **Reaction** (everything else). Per outer round:
    ///
    /// 1. drain + commit staged signals, collecting dirty effects (dedup);
    /// 2. run dirty Derivations — their writes stage; commit and run newly
    ///    dirty Derivations until none is dirty (Reactions collected along
    ///    the way are HELD);
    /// 3. then run each held Reaction exactly once;
    /// 4. Reactions may stage → next outer round.
    ///
    /// **Glitch-freedom argument**: a Reaction only runs after step 2 has
    /// settled, i.e. after every Derivation reachable from this round's
    /// commits has recomputed and committed. Whatever mix of raw signals and
    /// memos a Reaction reads, all of them reflect the same settled
    /// generation of the graph — the diamond `S → memo(S)` read together
    /// with `S` can never show (fresh S, stale memo), and the Reaction runs
    /// once per flush, not once per round. (idea-lite's naive round loop ran
    /// both classes together, letting a reaction observe the stale pair and
    /// run twice; the current sync core is depth-first-consistent, so
    /// authors implicitly rely on consistency — this preserves it.)
    ///
    /// A round limit (100) turns cyclic updates (reaction A stages what
    /// reaction B reads and vice versa) into a panic instead of a hang.
    ///
    /// Calling `flush` on world A from inside world B's effect is legal
    /// (worlds are independent); calling it re-entrantly on the SAME world
    /// from inside its own effect panics — staged writes made during a flush
    /// are committed by that same flush's next round.
    pub fn flush(&self) {
        flush_arena(&self.core.arena);
    }

    /// Is THIS world mid-flush? See the free [`is_flushing`] for the
    /// any-world form (the `is_reactive_busy` replacement).
    pub fn is_flushing(&self) -> bool {
        self.core.arena.flushing.get()
    }
}

/// True while any world on this thread is inside [`World::flush`] — i.e.
/// while effects are being run by the reactive system rather than by event
/// handlers. Replaces runtime-core's `is_reactive_busy`.
pub fn is_flushing() -> bool {
    TLS.try_with(|t| t.borrow().flush_depth > 0).unwrap_or(false)
}

// ============================================================================
// Arena — the per-world slot storage. Signals and effects live in parallel
// generational Vecs; the staged queue holds slot indices awaiting commit.
// All RefCells are borrowed only for short, user-code-free windows; any
// operation that runs user code (effect bodies, update closures, value
// drops) first moves what it needs OUT of the arena (see with_signal_data).
// ============================================================================

struct WorldArena {
    id: WorldId,
    signals: RefCell<Vec<SignalSlot>>,
    free_signals: RefCell<Vec<u32>>,
    effects: RefCell<Vec<EffectSlot>>,
    free_effects: RefCell<Vec<u32>>,
    /// Signal slots with a staged `next` (or a forced notify), awaiting
    /// commit. Deduped via `SignalSlot::queued`. Empty at rest.
    staged: RefCell<Vec<u32>>,
    /// World-level context: shared values keyed by type.
    context: RefCell<FxHashMap<TypeId, Box<dyn Any>>>,
    flushing: Cell<bool>,
}

struct SignalSlot {
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
}

/// Type-erased signal storage, so one arena Vec holds signals of any `T`.
trait AnySignal: Any {
    /// Flip `next` into `value` if it's an effective change (or `forced`);
    /// return whether subscribers must be notified.
    fn commit(&mut self, forced: bool) -> bool;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

struct SignalData<T> {
    /// Committed value — the only thing reads ever see.
    value: T,
    /// Staged value, last-write-wins within a batch.
    next: Option<T>,
}

impl<T: PartialEq + 'static> AnySignal for SignalData<T> {
    fn commit(&mut self, forced: bool) -> bool {
        match self.next.take() {
            Some(next) => {
                // The equality cut: a staged value that nets to the committed
                // one notifies nobody (A→B→A within one window is a no-op —
                // window-initial comparison for free, matching the guarded
                // `set` semantics runtime-core just landed). `forced` (a
                // set_always/touch in the window) taints the window and
                // notifies regardless.
                if forced || next != self.value {
                    self.value = next;
                    true
                } else {
                    false
                }
            }
            // No staged value: notify iff a `touch` forced it.
            None => forced,
        }
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

struct EffectSlot {
    gen: u32,
    /// `Rc` so the runner can clone the effect out and execute its body with
    /// zero arena borrows held (bodies read/write signals re-entrantly).
    data: Option<Rc<EffectData>>,
}

/// Scheduling class — the key to glitch-free flushing (see [`World::flush`]).
#[derive(Clone, Copy, PartialEq, Eq)]
enum EffectClass {
    /// A memo recompute: settles BEFORE reactions each round.
    Derivation,
    /// Bindings, user effects, structural drivers: run once per round, after
    /// every derivation has settled.
    Reaction,
}

struct EffectData {
    /// Self-identity, for subscriber entries and liveness checks.
    slot: u32,
    gen: u32,
    class: EffectClass,
    f: RefCell<Box<dyn FnMut()>>,
    /// Dedup flag: already collected into the current flush round?
    queued: Cell<bool>,
    /// Reverse edges: every signal slot (of this effect's own world —
    /// subscriptions are always intra-world) subscribed during the latest
    /// run, so the next run can unsubscribe before re-collecting exactly
    /// what it reads — not the union of all historical runs.
    deps: RefCell<Vec<u32>>,
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

fn create_signal<T: PartialEq + 'static>(arena: &Rc<WorldArena>, value: T) -> Signal<T> {
    let data: Box<dyn AnySignal> = Box::new(SignalData { value, next: None });
    let (slot, gen) = {
        let mut signals = arena.signals.borrow_mut();
        if let Some(slot) = arena.free_signals.borrow_mut().pop() {
            let s = &mut signals[slot as usize];
            debug_assert!(s.data.is_none(), "free-listed slot still occupied");
            s.data = Some(data);
            (slot, s.gen)
        } else {
            let slot = signals.len() as u32;
            signals.push(SignalSlot {
                gen: 0,
                queued: false,
                forced: false,
                subscribers: Vec::new(),
                data: Some(data),
            });
            (slot, 0)
        }
    };
    register_owned(OwnedItem::Signal { world: arena.id, slot, gen });
    Signal { world: arena.id, slot, gen, _marker: PhantomData }
}

fn create_effect(arena: &Rc<WorldArena>, class: EffectClass, f: Box<dyn FnMut()>) -> Effect {
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
            f: RefCell::new(f),
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
    Effect { world: arena.id, slot, gen, _marker: PhantomData }
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
/// cleanups with no arena borrows held.
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
        let deps: Vec<u32> = data.deps.borrow_mut().drain(..).collect();
        let mut signals = arena.signals.borrow_mut();
        for dep in deps {
            if let Some(s) = signals.get_mut(dep as usize) {
                s.subscribers.retain(|&(es, eg)| !(es == slot && eg == gen));
            }
        }
    }
    run_cleanups(&data);
}

// ============================================================================
// Handles — Copy triples routing to their own world. `PhantomData<*const T>`
// keeps them !Send/!Sync on purpose: worlds are thread-local, so a handle on
// another thread could only ever observe "dead world"; better to reject the
// program shape at compile time.
// ============================================================================

/// The full read+write handle to a signal. `Copy`. Prefer handing components
/// a capability half ([`ReadSignal`] / [`WriteSignal`]) — the signature then
/// documents the data flow.
pub struct Signal<T> {
    world: WorldId,
    slot: u32,
    gen: u32,
    _marker: PhantomData<*const T>,
}

/// Read capability only: `get`/`peek`/`with`. What display-only components
/// take. `Copy`.
pub struct ReadSignal<T> {
    world: WorldId,
    slot: u32,
    gen: u32,
    _marker: PhantomData<*const T>,
}

/// Write capability only: `set`/`update`/`touch`/…. What controls and
/// emitters take. `Copy`.
pub struct WriteSignal<T> {
    world: WorldId,
    slot: u32,
    gen: u32,
    _marker: PhantomData<*const T>,
}

/// A non-owning `Copy` handle to an effect. The effect's LIFETIME is owned
/// by the [`Owned`] scope that collected it (or the world root) — dropping
/// this handle changes nothing; dropping the `Owned` retires the effect.
pub struct Effect {
    world: WorldId,
    slot: u32,
    gen: u32,
    _marker: PhantomData<*const ()>,
}

macro_rules! impl_handle_meta {
    ($name:ident $(<$t:ident>)?) => {
        impl$(<$t>)? Clone for $name$(<$t>)? {
            fn clone(&self) -> Self { *self }
        }
        impl$(<$t>)? Copy for $name$(<$t>)? {}
        impl$(<$t>)? PartialEq for $name$(<$t>)? {
            fn eq(&self, other: &Self) -> bool {
                self.world == other.world && self.slot == other.slot && self.gen == other.gen
            }
        }
        impl$(<$t>)? Eq for $name$(<$t>)? {}
        impl$(<$t>)? std::fmt::Debug for $name$(<$t>)? {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, concat!(stringify!($name), "(w{}, s{}, g{})"), self.world, self.slot, self.gen)
            }
        }
    };
}

impl_handle_meta!(Signal<T>);
impl_handle_meta!(ReadSignal<T>);
impl_handle_meta!(WriteSignal<T>);
impl_handle_meta!(Effect);

// ============================================================================
// Signal operations. All routing goes handle → registry → the signal's OWN
// arena. The storage box is temporarily moved OUT of the arena for any
// operation whose closure could re-enter the reactive system (update
// composers, borrow-reads, value drops) — so no arena RefCell borrow is ever
// held across user code. The cost: touching the SAME signal from inside its
// own operation finds the slot empty and gets the reentrancy diagnostic.
// ============================================================================

fn stale_signal_panic(world: WorldId, slot: u32) -> ! {
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
fn with_signal_data<T: PartialEq + 'static, R>(
    arena: &WorldArena,
    world: WorldId,
    slot: u32,
    gen: u32,
    f: impl FnOnce(&mut SignalData<T>) -> R,
) -> R {
    let mut boxed: Box<dyn AnySignal> = {
        let mut signals = arena.signals.borrow_mut();
        let Some(s) = signals.get_mut(slot as usize) else {
            stale_signal_panic(world, slot)
        };
        if s.gen != gen {
            stale_signal_panic(world, slot);
        }
        match s.data.take() {
            Some(b) => b,
            None => diag_panic!(
                "reentrant-signal-read",
                "signal (world {}, slot {}) accessed re-entrantly while mid-operation: its \
                 storage is moved out of the arena for the duration of its own \
                 get/with/set/update, so an access reaching it from inside that window (e.g. \
                 reading a signal from inside its own update() closure) finds the slot empty. \
                 Read the value out first, then operate.",
                world,
                slot
            ),
        }
    };
    let result = f(boxed
        .as_any_mut()
        .downcast_mut::<SignalData<T>>()
        .expect("runtime-world: signal type mismatch despite matching generation — kernel bug"));
    let mut signals = arena.signals.borrow_mut();
    if let Some(s) = signals.get_mut(slot as usize) {
        if s.gen == gen && s.data.is_none() {
            s.data = Some(boxed);
        }
        // else: freed (or even reallocated) during f — drop the box.
    }
    result
}

/// Tracked/untracked committed-value read. Reads PANIC on a dead world:
/// there is no committed value left to return, and silently fabricating one
/// would hide a real lifetime bug.
fn read_signal<T: PartialEq + 'static, R>(
    world: WorldId,
    slot: u32,
    gen: u32,
    track: bool,
    f: impl FnOnce(&T) -> R,
) -> R {
    let Some(arena) = arena_of(world) else {
        diag_panic!(
            "dead-world-read",
            "signal read after its World was dropped (world {}, slot {}): the world's \
             registry entry is gone — the World value was dropped (its signals died with \
             it), or this handle crossed to another thread. Writes to a dead world are \
             silent no-ops, but a read has no committed value to return; restructure so the \
             reader cannot outlive the world.",
            world,
            slot
        )
    };
    if track {
        maybe_subscribe(&arena, world, slot, gen);
    }
    with_signal_data::<T, R>(&arena, world, slot, gen, |d| f(&d.value))
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
fn maybe_subscribe(arena: &WorldArena, world: WorldId, slot: u32, gen: u32) {
    let top = TLS.with(|t| {
        let t = t.borrow();
        if t.untrack_depth > 0 {
            None
        } else {
            t.effect_stack.last().copied()
        }
    });
    let Some((eworld, eslot, egen)) = top else { return };
    if eworld != world {
        return; // cross-world read: never subscribes
    }
    let added = {
        let mut signals = arena.signals.borrow_mut();
        let Some(s) = signals.get_mut(slot as usize) else { return };
        if s.gen != gen || s.data.is_none() {
            return; // stale/mid-op: the read itself will produce the diagnostic
        }
        if s.subscribers.contains(&(eslot, egen)) {
            false
        } else {
            s.subscribers.push((eslot, egen));
            true
        }
    };
    if added {
        // Remember the reverse edge so the effect's next run (or free) can
        // unsubscribe before re-collecting.
        let data = {
            let effects = arena.effects.borrow();
            effects
                .get(eslot as usize)
                .and_then(|e| if e.gen == egen { e.data.clone() } else { None })
        };
        if let Some(data) = data {
            data.deps.borrow_mut().push(slot);
        }
    }
}

/// Stage a value (last-write-wins) and enqueue the signal for commit at the
/// owning world's next flush. `force` marks the window tainted (always
/// notify). Writes to a DEAD world are silent no-ops — the world (an app
/// teardown, a finished SSR request) is gone and nothing can ever commit;
/// in-flight async writes racing a teardown are expected and harmless.
/// Writes through a STALE handle (live world, freed slot) still panic: that
/// is a use-after-unmount logic error worth surfacing.
fn stage_set<T: PartialEq + 'static>(world: WorldId, slot: u32, gen: u32, value: T, force: bool) {
    let Some(arena) = arena_of(world) else { return };
    with_signal_data::<T, ()>(&arena, world, slot, gen, |d| {
        d.next = Some(value);
    });
    enqueue(&arena, slot, gen, force);
}

/// Read-modify-write against the STAGED value (falling back to committed),
/// so multiple updates in one batch compose (two `+1`s = `+2`).
fn stage_update<T: PartialEq + 'static>(
    world: WorldId,
    slot: u32,
    gen: u32,
    f: impl FnOnce(&T) -> T,
) {
    let Some(arena) = arena_of(world) else { return };
    with_signal_data::<T, ()>(&arena, world, slot, gen, |d| {
        let next = f(d.next.as_ref().unwrap_or(&d.value));
        d.next = Some(next);
    });
    enqueue(&arena, slot, gen, false);
}

/// Force a notify with no value write (`touch`). Validates the generation
/// itself — with no storage access to piggyback on, a stale handle would
/// otherwise wake the recycled slot's NEW occupant's subscribers.
fn stage_touch(world: WorldId, slot: u32, gen: u32) {
    let Some(arena) = arena_of(world) else { return };
    {
        let signals = arena.signals.borrow();
        let Some(s) = signals.get(slot as usize) else {
            stale_signal_panic(world, slot)
        };
        if s.gen != gen {
            stale_signal_panic(world, slot);
        }
    }
    enqueue(&arena, slot, gen, true);
}

/// Write the COMMITTED value directly, bypassing staging, waking nobody
/// (`set_untracked` — runtime-core semantics preserved). Dependents keep
/// their stale view until something else notifies. A later guarded `set`
/// compares its staged value against THIS write (so `set_untracked(9)` then
/// `set(9)` nets to no fan-out), and a later `touch` delivers it.
fn write_untracked<T: PartialEq + 'static>(world: WorldId, slot: u32, gen: u32, value: T) {
    let Some(arena) = arena_of(world) else { return };
    with_signal_data::<T, ()>(&arena, world, slot, gen, |d| {
        d.value = value;
    });
}

fn enqueue(arena: &WorldArena, slot: u32, gen: u32, force: bool) {
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

macro_rules! impl_read_ops {
    () => {
        /// Read the committed value. If called during an effect run — an
        /// effect of this signal's OWN world — subscribes that effect.
        pub fn get(&self) -> T
        where
            T: Clone,
        {
            read_signal(self.world, self.slot, self.gen, true, T::clone)
        }

        /// Read the committed value WITHOUT tracking — never subscribes,
        /// even inside an effect. For effects that update state derived from
        /// themselves: `set(peek() + 1)` inside an effect body would
        /// infinitely re-trigger if written with `get()`.
        pub fn peek(&self) -> T
        where
            T: Clone,
        {
            read_signal(self.world, self.slot, self.gen, false, T::clone)
        }

        /// Tracked borrow-read of the committed value: `f` gets `&T`, no
        /// clone. The idiom for `Vec` rows / large values. Note: reading the
        /// SAME signal again from inside `f` is a reentrancy error (the
        /// storage is moved out for `f`'s duration); other signals are fine.
        pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
            read_signal(self.world, self.slot, self.gen, true, f)
        }

        /// Untracked borrow-read — [`with`](Self::with) without the
        /// subscription.
        pub fn with_untracked<R>(&self, f: impl FnOnce(&T) -> R) -> R {
            read_signal(self.world, self.slot, self.gen, false, f)
        }
    };
}

macro_rules! impl_write_ops {
    () => {
        /// Stage a value. Nothing observable changes until the owning world
        /// flushes; reads keep returning the committed value. Commit is
        /// equality-guarded (`PartialEq`): a staged value equal to the
        /// committed one notifies nobody. If the owning world is gone, this
        /// is a silent no-op.
        pub fn set(&self, value: T) {
            stage_set(self.world, self.slot, self.gen, value, false);
        }

        /// Stage a value that will notify subscribers at commit **even if
        /// equal** to the committed value (retrigger semantics). The
        /// escape hatch from the guarded [`set`](Self::set).
        pub fn set_always(&self, value: T) {
            stage_set(self.world, self.slot, self.gen, value, true);
        }

        /// Write the committed value directly **without staging or
        /// notifying**. Dependents keep their stale view until something
        /// else notifies (a later `set` that changes it, or a `touch`).
        /// Exists for the rare bookkeeping value effects read but must never
        /// react to; an antipattern for ordinary state.
        pub fn set_untracked(&self, value: T) {
            write_untracked(self.world, self.slot, self.gen, value);
        }

        /// Wake this signal's subscribers at the next flush **without
        /// writing** — the notification half of
        /// [`set_always`](Self::set_always). Use after mutating shared
        /// interior state the signal's value merely points to.
        pub fn touch(&self) {
            stage_touch(self.world, self.slot, self.gen);
        }

        /// Read-modify-write against the STAGED value (falling back to the
        /// committed one). What event handlers should use: two clicks in one
        /// batch compose (0→1→2), whereas `set(peek() + 1)` twice would read
        /// the committed 0 both times and lose an increment.
        pub fn update(&self, f: impl FnOnce(&T) -> T) {
            stage_update(self.world, self.slot, self.gen, f);
        }
    };
}

impl<T: PartialEq + 'static> Signal<T> {
    impl_read_ops!();
    impl_write_ops!();

    /// Split into capability halves. The `Signal` itself stays usable (it's
    /// `Copy`); this is about what you hand to OTHERS.
    pub fn split(&self) -> (ReadSignal<T>, WriteSignal<T>) {
        (self.read_only(), self.write_only())
    }

    pub fn read_only(&self) -> ReadSignal<T> {
        ReadSignal { world: self.world, slot: self.slot, gen: self.gen, _marker: PhantomData }
    }

    pub fn write_only(&self) -> WriteSignal<T> {
        WriteSignal { world: self.world, slot: self.slot, gen: self.gen, _marker: PhantomData }
    }
}

impl<T: PartialEq + 'static> ReadSignal<T> {
    impl_read_ops!();
}

impl<T: PartialEq + 'static> WriteSignal<T> {
    impl_write_ops!();
}

/// Create a signal in the ambient world. What components call.
pub fn signal<T: PartialEq + 'static>(value: T) -> Signal<T> {
    with_ambient(|arena| create_signal(arena, value))
}

// ============================================================================
// Effects — creation, running, cleanups, untrack.
// ============================================================================

fn wrap_effect_body<C: IntoCleanup>(mut f: impl FnMut() -> C + 'static) -> Box<dyn FnMut()> {
    Box::new(move || f().register())
}

/// Create an effect in the ambient world and run it once immediately to
/// collect dependencies. The body may return a cleanup closure
/// (React-style); it runs before the next re-run or at teardown, same as
/// [`on_cleanup`]. The returned handle is `Copy` and non-owning — the
/// effect's lifetime belongs to the enclosing [`collect_owned`] scope (or
/// the world root).
pub fn effect<C: IntoCleanup>(f: impl FnMut() -> C + 'static) -> Effect {
    with_ambient(|arena| create_effect(arena, EffectClass::Reaction, wrap_effect_body(f)))
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
    {
        let deps: Vec<u32> = data.deps.borrow_mut().drain(..).collect();
        let mut signals = arena.signals.borrow_mut();
        for dep in deps {
            if let Some(s) = signals.get_mut(dep as usize) {
                s.subscribers.retain(|&(es, eg)| !(es == data.slot && eg == data.gen));
            }
        }
    }
    let saved_untrack = with_tls(|t| {
        t.enter_stack.push(arena.id);
        t.effect_stack.push((arena.id, data.slot, data.gen));
        std::mem::replace(&mut t.untrack_depth, 0)
    });
    struct Guard {
        saved_untrack: u32,
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            let saved = self.saved_untrack;
            let _ = TLS.try_with(|t| {
                let mut t = t.borrow_mut();
                t.enter_stack.pop();
                t.effect_stack.pop();
                t.untrack_depth = saved;
            });
        }
    }
    let _guard = Guard { saved_untrack };
    // The body borrow is held across the run; a re-entrant run of the SAME
    // effect is impossible by construction (only flush runs effects, and a
    // world's flush cannot re-enter itself — see the reentrant-flush guard).
    (data.f.borrow_mut())();
}

/// Run `f` with dependency tracking suspended: signal reads inside subscribe
/// nothing, even when called from within an effect.
///
/// Suspension applies to the current code region only, not to nested effect
/// BODIES: an effect created inside `untrack` runs its first body with
/// tracking re-enabled (see `run_effect`) — it collects its own deps like
/// any other effect, and the enclosing region is untracked again once that
/// first run returns.
///
/// Suspension is GLOBAL (a depth counter in the single TLS struct), not
/// per-world: "this read is a snapshot" is a property of the code region,
/// not of whichever world is ambient or owns the signal. The alternative —
/// clearing only the ambient world's current effect — breaks under nesting:
/// inside `a`-effect { `b.enter(|| untrack(|| a_signal.get()))` } it would
/// clear B's (empty) slot while the running A-effect still tracks the read.
/// The global counter makes untrack mean the same thing in every
/// cross-world composition (tested: `untrack_suspends_tracking_globally_across_worlds`).
pub fn untrack<R>(f: impl FnOnce() -> R) -> R {
    with_tls(|t| t.untrack_depth += 1);
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = TLS.try_with(|t| {
                t.borrow_mut().untrack_depth -= 1;
            });
        }
    }
    let _guard = Guard;
    f()
}

/// Register a cleanup for the innermost RUNNING effect (whatever world it
/// belongs to). Runs before that effect's next re-run, or when its owning
/// scope drops — whichever comes first. May be called multiple times per
/// run; cleanups run in registration order.
pub fn on_cleanup(f: impl FnOnce() + 'static) {
    let top = TLS.with(|t| t.borrow().effect_stack.last().copied());
    let Some((world, slot, gen)) = top else {
        panic!("on_cleanup called outside an effect");
    };
    let Some(arena) = arena_of(world) else { return };
    let data = {
        let effects = arena.effects.borrow();
        effects
            .get(slot as usize)
            .and_then(|e| if e.gen == gen { e.data.clone() } else { None })
    };
    if let Some(data) = data {
        data.cleanups.borrow_mut().push(Box::new(f));
    }
}

/// What an effect body is allowed to return. Sealed: exactly three forms —
/// nothing, a cleanup closure, or `Option` of one (conditional cleanup). A
/// returned cleanup is registered through the same [`on_cleanup`] mechanism,
/// so ordering and drop semantics are identical for both styles.
#[diagnostic::on_unimplemented(
    message = "an effect body must return `()`, a cleanup closure, or `Option<cleanup closure>`",
    label = "not a valid effect return value"
)]
pub trait IntoCleanup: sealed::Sealed {
    fn register(self);
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for () {}
    impl<F: FnOnce() + 'static> Sealed for F {}
    impl<F: FnOnce() + 'static> Sealed for Option<F> {}
}

impl IntoCleanup for () {
    fn register(self) {}
}

impl<F: FnOnce() + 'static> IntoCleanup for F {
    fn register(self) {
        on_cleanup(self)
    }
}

impl<F: FnOnce() + 'static> IntoCleanup for Option<F> {
    fn register(self) {
        if let Some(cleanup) = self {
            on_cleanup(cleanup);
        }
    }
}

/// Store a value in the ambient world's context. What components call.
pub fn provide<T: Clone + 'static>(value: T) {
    with_ambient(|arena| {
        arena.context.borrow_mut().insert(TypeId::of::<T>(), Box::new(value));
    })
}

/// Fetch a value from the ambient world's context. What components call.
/// (Framework naming: `provide`/`inject`.)
pub fn inject<T: Clone + 'static>() -> Option<T> {
    with_ambient(|arena| {
        arena
            .context
            .borrow()
            .get(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast_ref::<T>())
            .cloned()
    })
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

// ============================================================================
// Ownership — the Copy-handle price. Handles can't refcount, so slots need
// owners: collect_owned() gathers every signal/effect created during a
// closure into an Owned; dropping the Owned frees them (effect cleanups
// first). Creations outside any collector are world-root-owned and freed on
// world drop. Phase 1's Realized holds exactly this.
// ============================================================================

#[derive(Clone, Copy)]
enum OwnedItem {
    Signal { world: WorldId, slot: u32, gen: u32 },
    Effect { world: WorldId, slot: u32, gen: u32 },
}

/// The owner of a batch of signal/effect slots (possibly spanning several
/// worlds). Dropping it is the entire teardown story: every collected
/// effect's cleanups run and its subscriptions unlink, then every collected
/// signal slot is freed; stale `Copy` handles left behind panic on use.
/// Items whose world is already dead are skipped (the world drop already
/// tore them down).
pub struct Owned {
    items: Vec<OwnedItem>,
    /// Worlds are thread-local; freeing from another thread could only
    /// silently leak. Reject the shape at compile time.
    _not_send: PhantomData<*const ()>,
}

impl Owned {
    /// Number of collected slots (signals + effects).
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// True when the scope collected nothing (dropping it is a no-op).
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Absorb another scope's slots into this one: `other`'s signals and
    /// effects now live — and die — with `self`. The scene layer (P1) uses
    /// this to fold a component body's `Owned` (an `Element::Owned` wrapper,
    /// collected at build time) into the enclosing `Realized`'s scope, so
    /// one drop still tears down the whole subtree.
    ///
    /// Drop ordering after a merge: `Owned::drop` runs its effects-first
    /// pass over the combined list in item order (self's originals, then
    /// other's), then the signals pass likewise — relative creation order
    /// within each scope is preserved, and every merged cleanup still runs
    /// before any merged signal is freed.
    pub fn merge(&mut self, mut other: Owned) {
        self.items.append(&mut other.items);
        // `other` drops here with an empty item list — a no-op.
    }
}

impl std::fmt::Debug for Owned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Owned({} slots)", self.items.len())
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        let items = std::mem::take(&mut self.items);
        // Effects first, in creation order: every cleanup can still read the
        // scope's sibling signals (freed only in the second pass).
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
}

fn register_owned(item: OwnedItem) {
    with_tls(|t| {
        if let Some(top) = t.collectors.last_mut() {
            top.push(item);
        }
        // No collector: world-root-owned; freed when the world drops.
    })
}

/// Run `f`, collecting every signal and effect it creates (in any world)
/// into the returned [`Owned`]. Collectors stack: an inner `collect_owned`'s
/// creations belong to the INNER scope only. If `f` panics, everything
/// collected so far is freed before the panic propagates.
pub fn collect_owned<R>(f: impl FnOnce() -> R) -> (R, Owned) {
    with_tls(|t| t.collectors.push(Vec::new()));
    struct Guard {
        armed: bool,
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            if self.armed {
                // Panic path: pop the collector and free what it gathered.
                let items = TLS
                    .try_with(|t| t.borrow_mut().collectors.pop())
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                drop(Owned { items, _not_send: PhantomData });
            }
        }
    }
    let mut guard = Guard { armed: true };
    let result = f();
    guard.armed = false;
    let items = with_tls(|t| t.collectors.pop()).expect("collector stack imbalance");
    (result, Owned { items, _not_send: PhantomData })
}

/// Run a component body: untracked (a run-once body's reads are snapshots by
/// definition — they must not subscribe whatever structural effect happens
/// to be mounting the component), with every signal/effect it creates
/// collected into the returned [`Owned`]. This is what `#[component]` will
/// wrap function bodies in (P1/P6); the Owned rides in the `Realized`.
pub fn component_scope<R>(f: impl FnOnce() -> R) -> (R, Owned) {
    collect_owned(|| untrack(f))
}

// ============================================================================
// Memo — a derived signal, composed from existing parts: a cache signal fed
// by a Derivation-class effect. Because the result commits like any signal,
// a memo gets the PartialEq cut for free (recompute lands on an equal value
// → downstream effects don't run), and because the recompute is a
// Derivation, every memo settles before any Reaction runs (glitch-free).
// ============================================================================

/// A cached derivation. `Copy`, like every handle: the cache signal and the
/// recomputation effect are OWNED by the collector active at `memo()` time
/// (or the world root), so the handle co-owns nothing — unlike idea-lite's
/// Rc-based `Memo`, whose handle kept the effect alive. Dropping the
/// enclosing `Owned` retires the memo; stale handles panic like any other.
pub struct Memo<T> {
    value: ReadSignal<T>,
    _effect: Effect,
}

impl<T> Clone for Memo<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Memo<T> {}

impl<T> std::fmt::Debug for Memo<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Memo({:?})", self.value)
    }
}

/// Create a derived signal in the ambient world.
///
/// For a single consumer a plain closure (`move || src.get() * 2`) is
/// lighter and needs no machinery — every `IntoValue` prop already accepts
/// one. `memo()` earns its signal when the derivation is shared (one
/// computation instead of N), expensive, or should cut propagation: source
/// changed but the derived value didn't → consumers don't re-run.
pub fn memo<T, F>(f: F) -> Memo<T>
where
    T: PartialEq + Clone + 'static,
    F: Fn() -> T + 'static,
{
    // Initial value computed untracked — the memo's own effect is the one
    // and only subscriber to f's dependencies, wherever memo() was called.
    let initial = untrack(&f);
    let cache = signal(initial);
    let write = cache.write_only();
    let _effect = with_ambient(|arena| {
        create_effect(
            arena,
            EffectClass::Derivation,
            Box::new(move || write.set(f())),
        )
    });
    Memo { value: cache.read_only(), _effect }
}

impl<T: PartialEq + 'static> Memo<T> {
    /// Tracked read of the cached value — same semantics as [`Signal::get`].
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.value.get()
    }

    /// Untracked read — same semantics as [`Signal::peek`].
    pub fn peek(&self) -> T
    where
        T: Clone,
    {
        self.value.peek()
    }

    /// Tracked borrow-read — same semantics as [`Signal::with`].
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.value.with(f)
    }
}

// ============================================================================
// Value<T> — a property that may or may not be dynamic. This is the props
// model: component parameters are `impl IntoValue<T>`, the CALLER picks
// reactivity per argument (literal → Const, signal/closure → Dyn), and
// whoever consumes the prop binds it — zero effects for Const, exactly one
// for Dyn.
// ============================================================================

pub enum Value<T> {
    /// Statically known to never change. Bound once; no reactive machinery.
    Const(T),
    /// Re-evaluated reactively; reads inside are tracked by the binding effect.
    Dyn(Box<dyn Fn() -> T>),
}

impl<T: 'static> Value<T> {
    /// Drive a setter with this value: once for Const (returns `None`), or
    /// via a tracking effect for Dyn (re-applied whenever its dependencies
    /// effectively change). The binding effect is created in the ambient
    /// ownership scope — collect it (via [`collect_owned`]) into whatever
    /// owns the bound thing; dropping that `Owned` stops the updates.
    #[must_use = "for Dyn values the updates stop when the collected binding Effect's Owned scope is dropped — collect it where the bound node lives"]
    pub fn bind(self, mut apply: impl FnMut(&T) + 'static) -> Option<Effect> {
        match self {
            Value::Const(v) => {
                apply(&v);
                None
            }
            Value::Dyn(f) => Some(effect(move || apply(&f()))),
        }
    }

    /// Read the current value. For consumers that want a one-off look rather
    /// than a live binding. (A Dyn read inside a running effect still tracks
    /// through the closure's own `get()` calls.)
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        match self {
            Value::Const(v) => v.clone(),
            Value::Dyn(f) => f(),
        }
    }
}

/// Conversions into `Value<T>`. Components take `impl IntoValue<T>` and
/// callers pass literals, signals, or closures with one call shape.
pub trait IntoValue<T> {
    fn into_value(self) -> Value<T>;
}

/// Pass-through, so a prop can be forwarded to a child component untouched.
impl<T> IntoValue<T> for Value<T> {
    fn into_value(self) -> Value<T> {
        self
    }
}

/// A signal IS a dynamic value: the binding effect subscribes through get().
impl<T: PartialEq + Clone + 'static> IntoValue<T> for Signal<T> {
    fn into_value(self) -> Value<T> {
        Value::Dyn(Box::new(move || self.get()))
    }
}

/// The read half works as a prop source exactly like a full signal.
impl<T: PartialEq + Clone + 'static> IntoValue<T> for ReadSignal<T> {
    fn into_value(self) -> Value<T> {
        Value::Dyn(Box::new(move || self.get()))
    }
}

/// A memo is a dynamic value like any signal. (The handle is Copy; the
/// memo's lifetime is its owning scope's, not the closure's.)
impl<T: PartialEq + Clone + 'static> IntoValue<T> for Memo<T> {
    fn into_value(self) -> Value<T> {
        Value::Dyn(Box::new(move || self.get()))
    }
}

/// Any zero-arg closure is a dynamic (usually derived) value.
impl<T, F> IntoValue<T> for F
where
    F: Fn() -> T + 'static,
{
    fn into_value(self) -> Value<T> {
        Value::Dyn(Box::new(self))
    }
}

/// Plain values of common types are Const. (A blanket `for T` impl would
/// collide with the closure impl, so these are enumerated.)
macro_rules! impl_const_into_value {
    ($($t:ty),+ $(,)?) => {$(
        impl IntoValue<$t> for $t {
            fn into_value(self) -> Value<$t> {
                Value::Const(self)
            }
        }
    )+};
}

impl_const_into_value! {
    String, bool, char,
    i8, i16, i32, i64, i128, isize,
    u8, u16, u32, u64, u128, usize,
    f32, f64,
}

impl IntoValue<String> for &'static str {
    fn into_value(self) -> Value<String> {
        Value::Const(self.into())
    }
}
