//! Carrying signal VALUES across a dev-time hot re-run.
//!
//! A subsecond patch rebinds function addresses; it does not re-render
//! anything. To put a patched component body on screen the dev host
//! re-runs the whole mounted tree, and a fresh run builds a fresh world
//! with fresh signals — so a counter goes back to zero, a text input
//! empties, a navigator returns to its first screen. Correct, and
//! useless: the edit you wanted to see is two clicks away again.
//!
//! This module is how the values get from the old run to the new one.
//!
//! # Values, not handles
//!
//! The obvious design is to hand the new run the OLD signal handles.
//! It does not survive contact with the kernel: handles are generational
//! indices into a world's arena, the old world is being torn down, and
//! sharing a slot between two `Owned` scopes means either a double free
//! or a leak. Worse, the old run's effects are still subscribed to
//! those slots, so a write from the new code would re-run code from the
//! patch's previous generation.
//!
//! So the value MOVES instead. [`harvest`] takes each recorded signal's
//! boxed payload straight out of the dying arena, and [`seed`] hands it
//! to the matching `signal()` call in the next run as its initial
//! value. Nothing is shared, nothing is cloned (so no `Clone` bound
//! appears on `signal`), and the new world is an ordinary world.
//!
//! # How a signal is matched across the two runs
//!
//! By WHERE it is created, not by what it is called — the kernel never
//! sees a variable name. The key is:
//!
//! - the **component path**: each `#[component]` body opens a frame
//!   ([`push_frame`], driven by the build probe the macro emits), and a
//!   frame's identity folds its parent's identity with the component's
//!   name and which occurrence of that name it is under that parent;
//! - the **ordinal**: the Nth `signal()` call inside that frame;
//! - the **type**: `TypeId` of the value.
//!
//! Position-based matching is what React Fast Refresh does, for the
//! same reason: it is the only identity available. Its failure mode is
//! bounded here by the type check plus frame poisoning — the first
//! ordinal whose type disagrees with what was recorded stops reuse for
//! the rest of that frame, so a body that gained or reordered state
//! gets FRESH state rather than another signal's value.
//!
//! # What is deliberately not preserved
//!
//! Only signals created during the mount's build window ([`arm`] …
//! [`disarm`]). A signal created later — inside an event handler, or
//! inside a reactive region that mounts after the initial walk — is not
//! recorded and comes back fresh.
//!
//! That is a real limit (a keyed list's row-local state is built by a
//! driver effect, after the walk) and it is the conservative one. Those
//! creations happen in whatever frame is open at the time, which for a
//! driver effect is the ROOT frame, so two different regions building
//! the same component would compete for the same ordinal. Handing one
//! region another's state is a worse failure than resetting it, and it
//! would be invisible.
//!
//! # Cost when the feature is off
//!
//! Nothing here compiles. The whole module is behind `hot-reload`, and
//! the two call sites in `create_signal` are `#[cfg]`-gated.

use std::any::{Any, TypeId};
use std::cell::RefCell;

use rustc_hash::FxHashMap;

use crate::WorldId;

/// One frame of the component path — an open `#[component]` build.
struct Frame {
    /// Fold of the parent's hash with this component's name and its
    /// occurrence index under that parent.
    hash: u64,
    /// How many `signal()` calls this frame has seen.
    ordinal: u32,
    /// Occurrences per child component name, so two `Section()`s under
    /// one parent are different frames.
    children: FxHashMap<&'static str, u32>,
    /// Set once a reuse was refused in this frame (a type mismatch, or
    /// the recording ran out of ordinals). Everything after it in the
    /// same frame gets fresh state: once the layout has diverged, later
    /// ordinals no longer refer to the same author-visible state.
    poisoned: bool,
}

/// What one recorded creation needs for harvesting.
struct Recorded {
    key: SlotKey,
    world: WorldId,
    slot: u32,
    gen: u32,
    /// A collector (a component's scope, in a mount) took ownership, so
    /// the slot is freed when the tree is. `false` is world-root: it
    /// outlives the tree and only dies with the world.
    owned: bool,
}

/// Identity of one `signal()` call inside one build.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct SlotKey {
    path: u64,
    ordinal: u32,
}

/// A harvested value. The TYPE it was harvested as lives in
/// [`Tls::expected`] instead, because the divergence check has to
/// happen even for a key whose value was already consumed.
struct Stored {
    /// `Box<SignalData<T>>` erased. See [`crate::steal_signal_data`].
    data: Box<dyn Any>,
}

/// Values carried from one run to the next. Opaque on purpose: the
/// dev host moves it from `harvest` to `seed` and never looks inside.
pub struct HotState {
    slots: FxHashMap<SlotKey, Stored>,
}

impl HotState {
    /// How many signal values are being carried. For the dev host's log
    /// and for tests.
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
}

impl std::fmt::Debug for HotState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HotState({} values)", self.slots.len())
    }
}

#[derive(Default)]
struct Tls {
    /// Open component frames, outermost first. Index 0 is the implicit
    /// ROOT frame, pushed by [`arm`] — the app's own entry fn is not a
    /// `#[component]`, so its signals need a frame of their own.
    frames: Vec<Frame>,
    /// `Some` while a build window is open: every creation is appended.
    recording: Option<Vec<Recorded>>,
    /// `Some` during the run that is consuming a previous harvest.
    seeded: Option<FxHashMap<SlotKey, Stored>>,
    /// What the previous run recorded at each key, so a divergence can
    /// be DETECTED (rather than silently reusing whatever is there).
    expected: FxHashMap<SlotKey, TypeId>,
    /// Depth of `runtime_world::unscoped` regions currently open.
    world_lifetime: u32,
}

thread_local! {
    static TLS: RefCell<Tls> = RefCell::new(Tls::default());
}

/// Open a build window: push the root frame and start recording
/// creations. Call immediately before mounting.
///
/// Idempotent in the sense that it resets — a window left open by a
/// panicking mount is discarded rather than accumulated.
pub fn arm() {
    TLS.with(|t| {
        let mut t = t.borrow_mut();
        t.frames.clear();
        t.frames.push(root_frame());
        t.recording = Some(Vec::new());
    });
}

/// Close the build window. Creations after this point are not recorded
/// and will not survive the next patch — see the module docs.
pub fn disarm() {
    TLS.with(|t| {
        let mut t = t.borrow_mut();
        t.frames.clear();
        t.seeded = None;
    });
}

fn root_frame() -> Frame {
    Frame {
        hash: 0xcbf2_9ce4_8422_2325,
        ordinal: 0,
        children: FxHashMap::default(),
        poisoned: false,
    }
}

/// Open a component's frame. Returns `false` when no build window is
/// open, in which case the caller must not pop.
///
/// Driven by the build probe `#[component]` emits, which brackets the
/// whole body including its `component_scope`.
pub fn push_frame(name: &'static str) -> bool {
    TLS.with(|t| {
        let mut t = t.borrow_mut();
        let Some(parent) = t.frames.last_mut() else {
            return false;
        };
        let nth = {
            let c = parent.children.entry(name).or_insert(0);
            let n = *c;
            *c += 1;
            n
        };
        let hash = fold(fold(parent.hash, name.as_bytes()), &nth.to_le_bytes());
        t.frames.push(Frame {
            hash,
            ordinal: 0,
            children: FxHashMap::default(),
            poisoned: false,
        });
        true
    })
}

/// Close the frame [`push_frame`] opened.
pub fn pop_frame() {
    TLS.with(|t| {
        let mut t = t.borrow_mut();
        // Never pop the root frame — `disarm` owns that.
        if t.frames.len() > 1 {
            t.frames.pop();
        }
    });
}

/// FNV-1a. Not cryptographic and does not need to be: a collision costs
/// one component's preserved state, and both halves of the comparison
/// are computed by this same function in one process.
fn fold(mut h: u64, bytes: &[u8]) -> u64 {
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Called from `create_signal` BEFORE the slot is allocated: if the
/// previous run recorded a signal of the same type at this position,
/// hand its value back so the new slot starts where the old one
/// finished.
///
/// Returns `None` — meaning "use the author's initial value" — when no
/// window is open, when nothing was seeded, when this frame is already
/// poisoned, or when the recorded type disagrees. The last case also
/// poisons the frame: once a body's state layout has diverged, later
/// ordinals in it no longer denote the same author-visible state, and
/// reusing them would put one variable's value into another.
pub(crate) fn take_seed<T: 'static>() -> Option<T> {
    TLS.with(|t| {
        let mut t = t.borrow_mut();
        let Tls { frames, seeded, expected, .. } = &mut *t;
        let seeded = seeded.as_mut()?;
        let frame = frames.last_mut()?;
        if frame.poisoned {
            return None;
        }
        let key = SlotKey { path: frame.hash, ordinal: frame.ordinal };
        match expected.get(&key) {
            Some(recorded) if *recorded == TypeId::of::<T>() => {}
            // A different type here, or the previous run had no signal
            // at this position at all: the layout moved.
            _ => {
                frame.poisoned = true;
                return None;
            }
        }
        let stored = seeded.remove(&key)?;
        let data = stored.data.downcast::<crate::SignalData<T>>().ok()?;
        Some(data.value)
    })
}

/// Called from `create_signal` AFTER the slot exists: remember where
/// this creation was, so [`harvest`] can find its value later.
///
/// Not generic over the value type: the type is recovered in
/// [`harvest`], from the payload box itself. Recording it here would
/// mean carrying a `TypeId` through the arena for slots that are never
/// harvested — every signal in a dev session, for the benefit of the
/// few that outlive a patch.
pub(crate) fn record(world: WorldId, slot: u32, gen: u32, owned: bool) {
    TLS.with(|t| {
        let mut t = t.borrow_mut();
        let Tls { frames, recording, .. } = &mut *t;
        let Some(recording) = recording.as_mut() else { return };
        let Some(frame) = frames.last_mut() else { return };
        let key = SlotKey { path: frame.hash, ordinal: frame.ordinal };
        frame.ordinal += 1;
        recording.push(Recorded { key, world, slot, gen, owned });
    });
}

/// RAII marker for a `runtime_world::unscoped` region: creations inside
/// it are world-lifetime services, which are cached and may not be
/// re-created by the next run. They take no seed and consume no ordinal,
/// so they cannot shift a component's state onto its neighbour's.
pub(crate) struct WorldLifetimeRegion(());

impl WorldLifetimeRegion {
    pub(crate) fn enter() -> Self {
        TLS.with(|t| t.borrow_mut().world_lifetime += 1);
        WorldLifetimeRegion(())
    }
}

impl Drop for WorldLifetimeRegion {
    fn drop(&mut self) {
        let _ = TLS.try_with(|t| {
            let mut t = t.borrow_mut();
            t.world_lifetime = t.world_lifetime.saturating_sub(1);
        });
    }
}

pub(crate) fn in_world_lifetime_region() -> bool {
    TLS.with(|t| t.borrow().world_lifetime > 0)
}

/// Take every recorded signal's value out of its (dying) arena.
///
/// # Contract
///
/// The caller must drop the world immediately afterwards. Harvesting
/// empties the slot and bumps its generation, so any handle still
/// pointing at it is stale from this moment — which is the kernel's
/// normal posture for a freed slot, and what makes a stale write panic
/// instead of corrupting the next occupant.
pub fn harvest() -> HotState {
    harvest_where(|_| true)
}

/// [`harvest`] for a rebuild that KEEPS its world and replaces only the
/// tree — the web page's hot-patch remount.
///
/// Takes only the signals a collector owned, because only those die with
/// the tree. A world-root signal outlives it, and something may still
/// hold its handle: an app's `thread_local` cache, a framework service.
/// Stealing its value would kill that handle. On CrewForge the page
/// panicked on the first render after a patch with "signal read after its
/// World was dropped" — the remount had replaced the world such a cache
/// pointed into — and keeping the world is what fixed it; this is the
/// half that keeps the world's own signals intact.
///
/// The cost: a signal created at the root level with no component around
/// it is not carried. Such a signal is not under a `#[component]`, and on
/// the web a root that is not a component cannot be hot-patched anyway.
pub fn harvest_owned() -> HotState {
    harvest_where(|r| r.owned)
}

fn harvest_where(keep: impl Fn(&Recorded) -> bool) -> HotState {
    let recorded = TLS.with(|t| t.borrow_mut().recording.take().unwrap_or_default());
    let mut slots = FxHashMap::default();
    let mut expected = FxHashMap::default();
    for r in recorded {
        if !keep(&r) {
            // Not carried, but still part of the layout: record its type
            // so the next run's creation at this position is recognised
            // as the same signal (and simply starts fresh) instead of
            // reading as a divergence that poisons the rest of the frame.
            if let Some(type_id) = crate::signal_type_id(r.world, r.slot, r.gen) {
                expected.insert(r.key, type_id);
            }
            continue;
        }
        let Some((type_id, data)) = crate::steal_signal_data(r.world, r.slot, r.gen) else {
            continue;
        };
        expected.insert(r.key, type_id);
        slots.insert(r.key, Stored { data });
    }
    TLS.with(|t| t.borrow_mut().expected = expected);
    HotState { slots }
}

/// Install a harvest for the next run. Values are consumed as the new
/// run's `signal()` calls reach their positions; whatever is left over
/// is dropped by the following [`disarm`].
pub fn seed(state: HotState) {
    TLS.with(|t| t.borrow_mut().seeded = Some(state.slots));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{signal, World};

    /// Drive one build window the way the dev host does, then harvest.
    fn run_and_harvest<R>(world: &World, body: impl FnOnce() -> R) -> (R, HotState) {
        arm();
        let r = world.enter(body);
        let state = harvest();
        disarm();
        (r, state)
    }

    #[test]
    fn a_root_signals_value_survives_into_the_next_run() {
        let w1 = World::new();
        let (_, state) = run_and_harvest(&w1, || {
            let n = signal(0i32);
            n.set(7);
            w1.flush();
            assert_eq!(n.get(), 7);
        });
        assert_eq!(state.len(), 1);
        drop(w1);

        let w2 = World::new();
        seed(state);
        arm();
        let seen = w2.enter(|| signal(0i32).get());
        disarm();
        assert_eq!(seen, 7, "the second run's signal must start where the first ended");
    }

    /// Regression: a world-lifetime creation (inside `unscoped`) is a
    /// cached service, so the second run usually does NOT create it
    /// again. Counted as an ordinal, it shifted every later signal in
    /// the frame by one — here handing `b` the cache's value.
    #[test]
    fn regression_an_unscoped_creation_does_not_shift_the_frame() {
        let w1 = World::new();
        let (_, state) = run_and_harvest(&w1, || {
            push_frame("Card");
            let a = signal(1i32);
            a.set(10);
            let cache = crate::unscoped(|| signal(0i32));
            cache.set(99);
            let b = signal(2i32);
            b.set(20);
            pop_frame();
            w1.flush();
        });
        drop(w1);

        let w2 = World::new();
        seed(state);
        arm();
        let (a, b) = w2.enter(|| {
            push_frame("Card");
            let a = signal(1i32).get();
            // The cache already exists this time; nothing is created.
            let b = signal(2i32).get();
            pop_frame();
            (a, b)
        });
        disarm();
        assert_eq!((a, b), (10, 20), "the cache's value leaked into a component's state");
    }

    /// Regression: the web rebuild keeps its world, and anything that
    /// outlives the tree — an app's `thread_local` signal cache, a
    /// framework service — must still be readable after the harvest. On
    /// CrewForge the first render after a patch panicked with "signal
    /// read after its World was dropped"; `harvest_owned` takes only what
    /// the tree's teardown frees.
    #[test]
    fn regression_harvest_owned_leaves_world_root_signals_alive() {
        let w = World::new();
        arm();
        let (global, (a, owned)) = w.enter(|| {
            let global = signal(5i32);
            let (a, owned) = crate::collect_owned(|| {
                push_frame("Counter");
                let a = signal(0i32);
                pop_frame();
                a
            });
            (global, (a, owned))
        });
        a.set(7);
        global.set(6);
        w.flush();
        let state = harvest_owned();
        disarm();
        assert_eq!(state.len(), 1, "only the tree-owned signal is carried");
        drop(owned);

        // The world lives on, and so does its root signal.
        assert_eq!(global.get(), 6);

        seed(state);
        arm();
        let (again, _owned) = w.enter(|| {
            crate::collect_owned(|| {
                push_frame("Counter");
                let v = signal(0i32).get();
                pop_frame();
                v
            })
        });
        disarm();
        assert_eq!(again, 7, "the component's state crosses into the rebuilt tree");
    }

    /// The root-level signal is not carried by `harvest_owned` but it is
    /// still part of the layout: the next run's creation at its position
    /// must start fresh WITHOUT poisoning the frame for the signals after
    /// it.
    #[test]
    fn a_signal_left_behind_does_not_poison_its_frame() {
        let w = World::new();
        arm();
        let (root_sig, (b, owned)) = w.enter(|| {
            let root_sig = signal(1i32);
            let b = crate::collect_owned(|| signal(2i32));
            (root_sig, b)
        });
        root_sig.set(100);
        b.set(200);
        w.flush();
        let state = harvest_owned();
        disarm();
        drop(owned);

        seed(state);
        arm();
        let (r, b) = w.enter(|| {
            let r = signal(1i32).get();
            let (b, _o) = crate::collect_owned(|| signal(2i32).get());
            (r, b)
        });
        disarm();
        assert_eq!(r, 1, "not carried: starts at the author's value");
        assert_eq!(b, 200, "and the owned signal after it still gets its own");
    }

    /// Ordinals are per FRAME, so two components with one signal each
    /// do not collide, and neither does the root's own state.
    #[test]
    fn per_component_frames_keep_sibling_state_apart() {
        let w1 = World::new();
        let (_, state) = run_and_harvest(&w1, || {
            let root = signal(1i32);
            root.set(10);
            push_frame("Row");
            let a = signal(2i32);
            a.set(20);
            pop_frame();
            push_frame("Row");
            let b = signal(3i32);
            b.set(30);
            pop_frame();
            w1.flush();
        });
        assert_eq!(state.len(), 3);
        drop(w1);

        let w2 = World::new();
        seed(state);
        arm();
        let (r, a, b) = w2.enter(|| {
            let r = signal(1i32).get();
            push_frame("Row");
            let a = signal(2i32).get();
            pop_frame();
            push_frame("Row");
            let b = signal(3i32).get();
            pop_frame();
            (r, a, b)
        });
        disarm();
        assert_eq!((r, a, b), (10, 20, 30));
    }

    /// The same component name under two different parents is two
    /// different paths.
    #[test]
    fn the_same_component_under_two_parents_does_not_collide() {
        let w1 = World::new();
        let (_, state) = run_and_harvest(&w1, || {
            push_frame("Left");
            push_frame("Row");
            signal(0i32).set(11);
            pop_frame();
            pop_frame();
            push_frame("Right");
            push_frame("Row");
            signal(0i32).set(22);
            pop_frame();
            pop_frame();
            w1.flush();
        });
        drop(w1);

        let w2 = World::new();
        seed(state);
        arm();
        let (l, r) = w2.enter(|| {
            push_frame("Left");
            push_frame("Row");
            let l = signal(0i32).get();
            pop_frame();
            pop_frame();
            push_frame("Right");
            push_frame("Row");
            let r = signal(0i32).get();
            pop_frame();
            pop_frame();
            (l, r)
        });
        disarm();
        assert_eq!((l, r), (11, 22));
    }

    /// The guard that makes position-matching safe: a body whose state
    /// layout changed gets FRESH state from the divergence onward,
    /// rather than one variable's value landing in another.
    #[test]
    fn a_changed_slot_layout_gets_fresh_state_instead_of_the_wrong_value() {
        let w1 = World::new();
        let (_, state) = run_and_harvest(&w1, || {
            let a = signal(0i32);
            let b = signal(String::new());
            a.set(5);
            b.set("kept".to_string());
            w1.flush();
        });
        drop(w1);

        // The patched body inserts a NEW signal of a different type
        // first, so ordinal 0 is now a String where an i32 was.
        let w2 = World::new();
        seed(state);
        arm();
        let (inserted, then) = w2.enter(|| {
            let inserted = signal("fresh".to_string()).get();
            let then = signal(0i32).get();
            (inserted, then)
        });
        disarm();
        assert_eq!(inserted, "fresh", "a type mismatch must not reuse the recorded value");
        assert_eq!(then, 0, "and the rest of the frame must be fresh too, not shifted");
    }

    /// A signal the new run creates BEYOND what the old one recorded
    /// starts from the author's value.
    #[test]
    fn a_newly_added_signal_starts_from_its_declared_value() {
        let w1 = World::new();
        let (_, state) = run_and_harvest(&w1, || {
            signal(0i32).set(9);
            w1.flush();
        });
        drop(w1);

        let w2 = World::new();
        seed(state);
        arm();
        let (first, added) = w2.enter(|| (signal(0i32).get(), signal(42i32).get()));
        disarm();
        assert_eq!(first, 9);
        assert_eq!(added, 42);
    }

    /// Nothing seeded: an ordinary run, values untouched.
    #[test]
    fn without_a_seed_every_signal_uses_its_declared_value() {
        let w = World::new();
        arm();
        let v = w.enter(|| signal(3i32).get());
        disarm();
        assert_eq!(v, 3);
    }

    /// Outside a build window the machinery is inert — no frame, no
    /// record, and `signal()` behaves exactly as it always does.
    #[test]
    fn creations_outside_a_build_window_are_not_recorded() {
        let w = World::new();
        let v = w.enter(|| signal(4i32).get());
        assert_eq!(v, 4);
        assert!(harvest().is_empty());
    }

    /// A non-`Copy`, non-`Clone`-bounded value moves across intact —
    /// the whole reason values are STOLEN rather than cloned.
    #[test]
    fn a_heap_value_moves_across_without_a_clone_bound() {
        #[derive(PartialEq, Debug)]
        struct NotClone(String);

        let w1 = World::new();
        let (_, state) = run_and_harvest(&w1, || {
            let s = signal(NotClone(String::new()));
            s.set(NotClone("typed by the user".into()));
            w1.flush();
        });
        drop(w1);

        let w2 = World::new();
        seed(state);
        arm();
        let kept = w2.enter(|| signal(NotClone(String::new())).with(|v| v.0.clone()));
        disarm();
        assert_eq!(kept, "typed by the user");
    }
}
