//! The kernel's semantics suite: the 33-test idea-lite suite ported to
//! worlds + Copy handles, plus the P0 additions — derivation-class flush
//! (diamond glitch-freedom), multi-world routing, generational staleness,
//! ownership collectors, and the guarded-set decomposition
//! (set / set_always / touch / set_untracked).
//!
//! Port notes (idea-lite `src/tests.rs` → here):
//! - Effect handles are `Copy` and non-owning; every "drop the guard" test
//!   holds the effect in an `Owned` from `collect_owned` and drops that.
//! - idea-lite's `writes_to_a_dead_world_are_noops` also asserted `get()`
//!   still answered after the scheduler died (its signals were self-owning
//!   Rcs). Arena signals die with their world, so the port asserts writes
//!   stay silent no-ops while reads panic (`reads_from_a_dead_world_panic`).
//! - Tests 28–32 exercised idea-lite's scene layer (`Element`/`realize`/
//!   keyed lists). Their LIFECYCLE semantics are ported here against the
//!   kernel's ownership primitives — see the "scene-lifecycle analogues"
//!   section, which hand-rolls the P1 Dyn/Keyed driver shapes on
//!   `collect_owned`; the full scene-layer ports live in
//!   `runtime-scene/src/tests.rs` (P1), including `key_conversions` (33).

use super::*;
use std::cell::Cell;
use std::collections::HashMap;

fn counter() -> Rc<Cell<usize>> {
    Rc::new(Cell::new(0))
}

fn bump(c: &Rc<Cell<usize>>) {
    c.set(c.get() + 1);
}

/// An effect (root-owned by `world`) that reads `sig` and counts its runs.
fn watch<T: PartialEq + Clone + 'static>(world: &World, sig: Signal<T>) -> Rc<Cell<usize>> {
    let runs = counter();
    let r = Rc::clone(&runs);
    world.enter(|| {
        effect(move || {
            let _ = sig.get();
            bump(&r);
        })
    });
    runs
}

// ============================================================================
// Signals: staged writes, batch commits
// ============================================================================

#[test]
fn set_stages_until_flush() {
    let world = World::new();
    let s = world.enter(|| signal(1));
    s.set(2);
    assert_eq!(s.get(), 1, "writes stay staged until the batch boundary");
    world.flush();
    assert_eq!(s.get(), 2);
}

#[test]
fn writes_collapse_into_one_commit() {
    let world = World::new();
    let s = world.enter(|| signal(0));
    let runs = watch(&world, s);
    assert_eq!(runs.get(), 1, "effects run once at creation");
    s.set(1);
    s.set(2);
    world.flush();
    assert_eq!(s.get(), 2, "last write wins");
    assert_eq!(runs.get(), 2, "N sets in one batch notify once");
}

#[test]
fn redundant_writes_are_net_noops() {
    let world = World::new();
    let s = world.enter(|| signal(5));
    let runs = watch(&world, s);
    s.set(9);
    s.set(5); // back to the committed value
    world.flush();
    assert_eq!(runs.get(), 1, "a set that lands on the committed value notifies nobody");
}

#[test]
fn update_composes_on_the_staged_value() {
    let world = World::new();
    let s = world.enter(|| signal(0));
    s.update(|n| n + 1);
    s.update(|n| n + 1);
    world.flush();
    assert_eq!(s.get(), 2, "two updates in one batch must not lose an increment");

    s.set(10);
    s.update(|n| n + 1); // composes on the staged 10, not the committed 2
    world.flush();
    assert_eq!(s.get(), 11);
}

#[test]
fn capability_halves_share_the_signal() {
    let world = World::new();
    let (read, write) = world.enter(|| signal(0).split());
    write.update(|n| n + 1);
    write.update(|n| n + 1);
    world.flush();
    assert_eq!(read.get(), 2);
    write.set(7);
    world.flush();
    assert_eq!((read.get(), read.peek()), (7, 7));
}

#[test]
fn writes_to_a_dead_world_are_noops() {
    let world = World::new();
    let s = world.enter(|| signal(1));
    drop(world);
    // Every write shape must be a silent no-op — nothing can ever commit
    // without a world (in-flight async writes racing a teardown are
    // expected). The read-side deviation from idea-lite is covered by
    // `reads_from_a_dead_world_panic`.
    s.set(2);
    s.set_always(3);
    s.set_untracked(4);
    s.touch();
    s.update(|n| n + 1);
}

#[test]
#[should_panic(expected = "idealyst[dead-world-read]")]
fn reads_from_a_dead_world_panic() {
    let world = World::new();
    let s = world.enter(|| signal(1));
    drop(world);
    let _ = s.get();
}

// ============================================================================
// Effects: exact tracking, dedup, ownership
// ============================================================================

#[test]
fn dependencies_recollect_each_run() {
    let world = World::new();
    let runs = counter();
    let (cond, a, b) = world.enter(|| {
        let cond = signal(true);
        let a = signal(0);
        let b = signal(0);
        let runs = Rc::clone(&runs);
        effect(move || {
            if cond.get() {
                a.get();
            } else {
                b.get();
            }
            bump(&runs);
        });
        (cond, a, b)
    });
    assert_eq!(runs.get(), 1);

    b.set(5);
    world.flush();
    assert_eq!(runs.get(), 1, "the untaken branch's signal is not a dependency");

    a.set(5);
    world.flush();
    assert_eq!(runs.get(), 2);

    cond.set(false);
    world.flush();
    assert_eq!(runs.get(), 3);

    a.set(9);
    world.flush();
    assert_eq!(runs.get(), 3, "a was dropped from the deps when the branch flipped");

    b.set(9);
    world.flush();
    assert_eq!(runs.get(), 4);
}

#[test]
fn an_effect_runs_once_per_flush_even_with_two_changed_deps() {
    let world = World::new();
    let runs = counter();
    let (x, y) = world.enter(|| {
        let x = signal(0);
        let y = signal(0);
        let runs = Rc::clone(&runs);
        effect(move || {
            x.get();
            y.get();
            bump(&runs);
        });
        (x, y)
    });
    x.set(1);
    y.set(1);
    world.flush();
    assert_eq!(runs.get(), 2, "one run sees the whole round's snapshot");
}

#[test]
fn dropping_the_owned_scope_retires_the_effect() {
    // Port of `dropping_the_handle_retires_the_effect`: handles are Copy and
    // non-owning here, so lifecycle rides on the Owned collector instead.
    let world = World::new();
    let runs = counter();
    let s = world.enter(|| signal(0));
    let ((), owned) = world.enter(|| {
        collect_owned(|| {
            let runs = Rc::clone(&runs);
            effect(move || {
                s.get();
                bump(&runs);
            });
        })
    });
    drop(owned);
    s.set(1);
    world.flush();
    assert_eq!(runs.get(), 1, "a dropped effect stops being notified");
}

#[test]
fn peek_never_subscribes() {
    let world = World::new();
    let runs = counter();
    let s = world.enter(|| {
        let s = signal(0);
        let runs = Rc::clone(&runs);
        effect(move || {
            s.peek();
            bump(&runs);
        });
        s
    });
    s.set(1);
    world.flush();
    assert_eq!(runs.get(), 1);
}

#[test]
fn untrack_suspends_and_restores_tracking() {
    let world = World::new();
    let runs = counter();
    let (a, b) = world.enter(|| {
        let a = signal(0);
        let b = signal(0);
        let runs = Rc::clone(&runs);
        effect(move || {
            untrack(|| a.get());
            b.get(); // tracking must be restored after untrack
            bump(&runs);
        });
        (a, b)
    });
    a.set(1);
    world.flush();
    assert_eq!(runs.get(), 1, "untracked read is not a dependency");
    b.set(1);
    world.flush();
    assert_eq!(runs.get(), 2, "tracking resumed after the untrack scope");
}

#[test]
fn effect_writes_settle_within_one_flush() {
    let world = World::new();
    let seen = Rc::new(Cell::new(0));
    let (a, b) = world.enter(|| {
        let a = signal(1);
        let b = signal(0);
        effect(move || b.set(a.get() * 2));
        let seen = Rc::clone(&seen);
        effect(move || seen.set(b.get()));
        (a, b)
    });
    assert_eq!(seen.get(), 0, "e1's write is staged, not visible at creation");
    world.flush();
    assert_eq!((b.get(), seen.get()), (2, 2));
    a.set(5);
    world.flush();
    assert_eq!((b.get(), seen.get()), (10, 10), "chained rounds settle in one flush call");
}

#[test]
#[should_panic(expected = "did not settle")]
fn cyclic_updates_panic_instead_of_hanging() {
    let world = World::new();
    world.enter(|| {
        let a = signal(0);
        let b = signal(0);
        effect(move || a.set(b.get() + 1));
        effect(move || b.set(a.get() + 1));
    });
    world.flush();
}

#[test]
#[should_panic(expected = "outside World::enter")]
fn ambient_api_requires_a_world() {
    let _ = signal(0);
}

// ============================================================================
// Cleanups: both styles, ordering, drop
// ============================================================================

#[test]
fn on_cleanup_runs_before_rerun_and_on_drop() {
    let world = World::new();
    let cleaned = counter();
    let s = world.enter(|| signal(0));
    let ((), owned) = world.enter(|| {
        collect_owned(|| {
            let cleaned = Rc::clone(&cleaned);
            effect(move || {
                s.get();
                let cleaned = Rc::clone(&cleaned);
                on_cleanup(move || bump(&cleaned));
            });
        })
    });
    assert_eq!(cleaned.get(), 0);
    s.set(1);
    world.flush(); // re-run: previous run's cleanup fires first
    assert_eq!(cleaned.get(), 1);
    drop(owned);
    assert_eq!(cleaned.get(), 2);
}

#[test]
fn returned_closures_are_cleanups() {
    let world = World::new();
    let cleaned = counter();
    let s = world.enter(|| signal(0));
    let ((), owned) = world.enter(|| {
        collect_owned(|| {
            let cleaned = Rc::clone(&cleaned);
            effect(move || {
                s.get();
                let cleaned = Rc::clone(&cleaned);
                move || bump(&cleaned)
            });
        })
    });
    s.set(1);
    world.flush();
    assert_eq!(cleaned.get(), 1);
    drop(owned);
    assert_eq!(cleaned.get(), 2);
}

#[test]
fn optional_cleanups_only_register_when_some() {
    let world = World::new();
    let cleaned = counter();
    let s = world.enter(|| signal(0));
    let ((), owned) = world.enter(|| {
        collect_owned(|| {
            let cleaned = Rc::clone(&cleaned);
            effect(move || {
                let v = s.get();
                let cleaned = Rc::clone(&cleaned);
                if v == 0 { Some(move || bump(&cleaned)) } else { None }
            });
        })
    });
    s.set(1);
    world.flush(); // first run's Some(cleanup) fires; second run registers None
    assert_eq!(cleaned.get(), 1);
    drop(owned);
    assert_eq!(cleaned.get(), 1, "no cleanup was registered by the last run");
}

#[test]
fn cleanups_run_in_registration_order() {
    let world = World::new();
    let order = Rc::new(RefCell::new(Vec::new()));
    let ((), owned) = world.enter(|| {
        collect_owned(|| {
            let order = Rc::clone(&order);
            effect(move || {
                let (a, b) = (Rc::clone(&order), Rc::clone(&order));
                on_cleanup(move || a.borrow_mut().push("first"));
                on_cleanup(move || b.borrow_mut().push("second"));
            });
        })
    });
    drop(owned);
    assert_eq!(*order.borrow(), ["first", "second"]);
}

// ============================================================================
// Worlds: isolation
// ============================================================================

#[test]
fn worlds_are_discrete() {
    let w1 = World::new();
    let w2 = World::new();
    let s = w1.enter(|| signal(0));
    let runs = watch(&w1, s);
    s.set(1);
    w2.flush();
    assert_eq!((s.get(), runs.get()), (0, 1), "another world's flush commits nothing here");
    w1.flush();
    assert_eq!((s.get(), runs.get()), (1, 2));
}

#[derive(Clone, PartialEq, Debug)]
struct Theme(&'static str);

#[test]
fn context_is_typed_and_per_world() {
    let w1 = World::new();
    let w2 = World::new();
    w1.enter(|| provide(Theme("dark")));
    w1.enter(|| provide(42u32));
    assert_eq!(w1.enter(inject::<Theme>), Some(Theme("dark")));
    assert_eq!(w1.enter(inject::<u32>), Some(42));
    assert_eq!(w2.enter(inject::<Theme>), None);
    // The direct World methods hit the same map as the ambient fns.
    assert_eq!(w1.inject::<Theme>(), Some(Theme("dark")));
    w2.provide(Theme("light"));
    assert_eq!(w2.enter(inject::<Theme>), Some(Theme("light")));
}

// ============================================================================
// Memos
// ============================================================================

#[test]
fn memos_compute_once_and_share() {
    let world = World::new();
    let computations = counter();
    let runs_a = counter();
    let runs_b = counter();
    let (s, m) = world.enter(|| {
        let s = signal(1);
        let computations = Rc::clone(&computations);
        let m = memo(move || {
            bump(&computations);
            s.get() * 10
        });
        let runs = Rc::clone(&runs_a);
        effect(move || {
            m.get();
            bump(&runs);
        });
        let runs = Rc::clone(&runs_b);
        effect(move || {
            m.get();
            bump(&runs);
        });
        (s, m)
    });
    let baseline = computations.get();
    assert_eq!(m.get(), 10);
    s.set(2);
    world.flush();
    assert_eq!(m.get(), 20);
    assert_eq!(computations.get(), baseline + 1, "one recompute serves both consumers");
    assert_eq!((runs_a.get(), runs_b.get()), (2, 2));
}

#[test]
fn memo_equality_cut_stops_propagation() {
    let world = World::new();
    let runs = counter();
    let src = world.enter(|| {
        let src = signal(vec![1]);
        let m = memo(move || src.with(|v| v.len()));
        let runs = Rc::clone(&runs);
        effect(move || {
            m.get();
            bump(&runs);
        });
        src
    });
    assert_eq!(runs.get(), 1);
    src.set(vec![2]); // different list, same length
    world.flush();
    assert_eq!(runs.get(), 1, "unchanged derived value must not wake consumers");
    src.set(vec![2, 3]);
    world.flush();
    assert_eq!(runs.get(), 2);
}

// ============================================================================
// Value<T> props
// ============================================================================

#[test]
fn const_values_bind_once_with_no_effect() {
    let world = World::new();
    world.enter(|| {
        let applied = Rc::new(Cell::new(0));
        let binding = 5.into_value().bind({
            let applied = Rc::clone(&applied);
            move |n| applied.set(*n)
        });
        assert!(binding.is_none(), "Const needs no reactive machinery");
        assert_eq!(applied.get(), 5);
    });
}

#[test]
fn dyn_values_bind_reactively_until_dropped() {
    let world = World::new();
    let applied = Rc::new(Cell::new(0));
    let s = world.enter(|| signal(1));
    let (binding, owned) = world.enter(|| {
        collect_owned(|| {
            s.into_value().bind({
                let applied = Rc::clone(&applied);
                move |n| applied.set(*n)
            })
        })
    });
    assert!(binding.is_some(), "a signal prop is Dyn");
    assert_eq!(applied.get(), 1);
    s.set(2);
    world.flush();
    assert_eq!(applied.get(), 2);
    drop(owned); // the binding effect lives in the collected scope
    s.set(3);
    world.flush();
    assert_eq!(applied.get(), 2, "dropping the binding's scope stops updates");
}

#[test]
fn closures_are_derived_dyn_values() {
    let world = World::new();
    let applied = Rc::new(Cell::new(0));
    let s = world.enter(|| {
        let s = signal(2);
        let binding = (move || s.get() * 10).into_value().bind({
            let applied = Rc::clone(&applied);
            move |n| applied.set(*n)
        });
        assert!(binding.is_some(), "a closure prop is Dyn");
        s
    });
    assert_eq!(applied.get(), 20);
    s.set(3);
    world.flush();
    assert_eq!(applied.get(), 30);
}

// ============================================================================
// Component scope
// ============================================================================

#[test]
fn component_scope_collects_effects_into_owned() {
    let world = World::new();
    world.enter(|| {
        let (_, owned) = component_scope(|| "x");
        assert!(owned.is_empty(), "no creations → empty scope, dropping it is a no-op");

        let (_, owned) = component_scope(|| {
            effect(|| {}); // fire-and-forget: the scope must co-own it
            "x"
        });
        assert_eq!(owned.len(), 1, "the body's effect is co-owned by the scope");
    });
}

#[test]
fn component_bodies_run_untracked() {
    let world = World::new();
    let runs = counter();
    let s = world.enter(|| {
        let s = signal(0);
        let runs = Rc::clone(&runs);
        effect(move || {
            bump(&runs);
            // A component mounted inside this effect: its body-level read
            // must NOT subscribe this effect.
            let (_, _scope) = component_scope(|| {
                s.get();
                "x"
            });
        });
        s
    });
    s.set(1);
    world.flush();
    assert_eq!(runs.get(), 1, "a body-level get() is a snapshot, not a subscription");
}

// ============================================================================
// Scene-lifecycle analogues — ports of idea-lite's realize/Dyn/Keyed tests
// (28–32) against the kernel's ownership primitives. These hand-roll the
// exact scope shapes P1's structural drivers will use, so the lifecycle
// semantics (swap = drop old scope, unmount = drop, keyed identity keeps
// row scopes alive) are pinned at the kernel level.
// ============================================================================

/// A scope owning one effect whose cleanup bumps `drops` — makes teardown
/// observable, the way component effects are in real apps. (Port of
/// idea-lite's `label_with_drop_probe`.)
fn scope_with_drop_probe(label: &str, drops: &Rc<Cell<usize>>) -> (String, Owned) {
    let drops = Rc::clone(drops);
    collect_owned(move || {
        let drops = Rc::clone(&drops);
        effect(move || {
            let drops = Rc::clone(&drops);
            move || bump(&drops)
        });
        label.to_string()
    })
}

#[test]
fn nested_scopes_compose_results() {
    // Port of `realize_walks_structure`: structure comes back through the
    // collector transparently, and ownership PARTITIONS — the inner scope's
    // creations belong to the inner Owned only.
    let world = World::new();
    let ((tree, inner_owned, s_outer, s_inner), outer_owned) = world.enter(|| {
        collect_owned(|| {
            let s_outer = signal(1);
            let ((label, s_inner), inner_owned) = collect_owned(|| (String::from("b"), signal(2)));
            (format!("root/a/{label}"), inner_owned, s_outer, s_inner)
        })
    });
    assert_eq!(tree, "root/a/b");
    assert_eq!(outer_owned.len(), 1, "outer owns only its own signal");
    assert_eq!(inner_owned.len(), 1, "inner owns only its own signal");
    drop(outer_owned);
    assert_eq!(s_inner.get(), 2, "inner scope survives the outer Owned's drop");
    let _ = s_outer; // its slot is now stale — covered by the staleness tests
    drop(inner_owned);
}

#[test]
fn dyn_slot_swap_tears_down_previous_scope() {
    // Port of `dyn_holes_swap_and_tear_down`: the P1 Dyn driver's shape — an
    // effect that rebuilds a scope on each run and stores it in a slot,
    // where the assignment drops the previous Realized/Owned.
    let world = World::new();
    let drops = counter();
    let slot: Rc<RefCell<Option<(String, Owned)>>> = Rc::new(RefCell::new(None));
    let s = world.enter(|| signal(true));
    world.enter(|| {
        let slot = Rc::clone(&slot);
        let drops = Rc::clone(&drops);
        effect(move || {
            let built = if s.get() {
                scope_with_drop_probe("on", &drops)
            } else {
                collect_owned(|| String::from("off"))
            };
            *slot.borrow_mut() = Some(built); // drops the previous scope
        });
    });
    assert_eq!(slot.borrow().as_ref().unwrap().0, "on");
    assert_eq!(drops.get(), 0);

    s.set(false);
    world.flush();
    assert_eq!(slot.borrow().as_ref().unwrap().0, "off");
    assert_eq!(drops.get(), 1, "the replaced scope's effects were dropped");
}

#[test]
fn dropping_the_root_scope_retires_owned_effects() {
    // Port of `dropping_the_realized_tree_retires_its_effects`: unmounting
    // is dropping.
    let world = World::new();
    let runs = counter();
    let s = world.enter(|| signal(0));
    let (_label, owned) = world.enter(|| {
        collect_owned(|| {
            let runs = Rc::clone(&runs);
            effect(move || {
                s.get();
                bump(&runs);
            });
            "x"
        })
    });
    s.set(1);
    world.flush();
    assert_eq!(runs.get(), 2);
    drop(owned);
    s.set(2);
    world.flush();
    assert_eq!(runs.get(), 2, "unmounting is dropping");
}

/// The live state of a keyed list: one scope per key, plus presentation
/// order. The P1 Keyed driver's core, hand-rolled.
struct KeyedScopes {
    order: Vec<i64>,
    entries: HashMap<i64, (String, Owned)>,
}

/// One reconcile pass, ported from idea-lite's `Element::Keyed` effect body:
/// kept keys carry their scope over untouched (`render` NOT called), new
/// keys render fresh, duplicates panic. Returns the vanished scopes so the
/// caller can drop them AFTER releasing any borrows (their cleanups run at
/// that drop — idea-lite's "release the borrow before the vanished subtrees
/// drop" invariant).
fn reconcile_keyed(
    list: &mut KeyedScopes,
    items: &[i64],
    render: &mut dyn FnMut(i64) -> (String, Owned),
) -> HashMap<i64, (String, Owned)> {
    let mut previous = std::mem::take(&mut list.entries);
    list.order.clear();
    for &key in items {
        let scope = previous.remove(&key).unwrap_or_else(|| render(key));
        if list.entries.insert(key, scope).is_some() {
            panic!("keyed list: duplicate key {key:?} — every item needs a unique key");
        }
        list.order.push(key);
    }
    previous
}

#[test]
fn keyed_scopes_reconcile_by_identity() {
    // Port of `keyed_lists_reconcile_by_identity`.
    let world = World::new();
    let renders = counter();
    let drops = counter();
    let list = world.enter(|| signal(vec![1i64, 2]));
    let scopes = Rc::new(RefCell::new(KeyedScopes { order: Vec::new(), entries: HashMap::new() }));
    let ((), root) = world.enter(|| {
        collect_owned(|| {
            let scopes = Rc::clone(&scopes);
            let renders = Rc::clone(&renders);
            let drops = Rc::clone(&drops);
            effect(move || {
                let items = list.get();
                let renders = Rc::clone(&renders);
                let drops = Rc::clone(&drops);
                let mut render = move |n: i64| {
                    bump(&renders);
                    scope_with_drop_probe(&format!("r{n}"), &drops)
                };
                let vanished = {
                    let mut s = scopes.borrow_mut();
                    reconcile_keyed(&mut s, &items, &mut render)
                };
                drop(vanished); // unmount AFTER the borrow is released
            });
        })
    });
    let rendered = |scopes: &Rc<RefCell<KeyedScopes>>| -> Vec<String> {
        let s = scopes.borrow();
        s.order.iter().map(|k| s.entries[k].0.clone()).collect()
    };
    assert_eq!(rendered(&scopes), ["r1", "r2"]);
    assert_eq!((renders.get(), drops.get()), (2, 0));

    list.set(vec![2, 1, 3]); // reorder the kept rows, insert one
    world.flush();
    assert_eq!(rendered(&scopes), ["r2", "r1", "r3"]);
    assert_eq!(
        (renders.get(), drops.get()),
        (3, 0),
        "kept keys keep their scopes: one render, zero drops"
    );

    list.set(vec![3]);
    world.flush();
    assert_eq!(rendered(&scopes), ["r3"]);
    assert_eq!((renders.get(), drops.get()), (3, 2));

    drop(root); // retire the list effect...
    scopes.borrow_mut().entries.clear(); // ...and unrealize the remaining row
    assert_eq!(drops.get(), 3, "unrealizing the list unmounts the remaining rows");
}

#[test]
#[should_panic(expected = "duplicate key")]
fn duplicate_keys_panic() {
    // Port of idea-lite's duplicate-key panic, against the same reconcile
    // shape P1 will use. (No world needed: collect_owned works anywhere.)
    let mut list = KeyedScopes { order: Vec::new(), entries: HashMap::new() };
    let _ = reconcile_keyed(&mut list, &[1, 1], &mut |n| collect_owned(|| format!("r{n}")));
}

// NOT PORTED HERE — `key_conversions` (33/33): `Key` is a scene-layer type
// (Element::Keyed's identity); it landed with P1's runtime-scene, where the
// test now lives (`runtime-scene/src/tests.rs::key_conversions`).

// ============================================================================
// NEW (P0): derivation-class flush — glitch freedom
// ============================================================================

#[test]
fn diamond_effect_runs_once_with_consistent_pair() {
    // The P0 centerpiece: S → memo(S), one Reaction reading BOTH. The
    // reaction must run exactly once per flush and never observe
    // (fresh S, stale memo). idea-lite's naive round loop ran it twice, the
    // first time with the stale pair.
    let world = World::new();
    let runs = counter();
    let pairs: Rc<RefCell<Vec<(i32, i32)>>> = Rc::new(RefCell::new(Vec::new()));
    let s = world.enter(|| {
        let s = signal(1);
        let m = memo(move || s.get() * 10);
        let runs = Rc::clone(&runs);
        let pairs = Rc::clone(&pairs);
        effect(move || {
            let pair = (s.get(), m.get());
            pairs.borrow_mut().push(pair);
            bump(&runs);
        });
        s
    });
    assert_eq!(runs.get(), 1);
    s.set(2);
    world.flush();
    assert_eq!(runs.get(), 2, "diamond: exactly ONE reaction run per flush");
    s.set(3);
    world.flush();
    assert_eq!(runs.get(), 3);
    for &(sv, mv) in pairs.borrow().iter() {
        assert_eq!(mv, sv * 10, "a reaction observed (fresh source, stale memo): ({sv}, {mv})");
    }
}

#[test]
fn memo_chain_settles_in_one_flush_with_one_reaction_run() {
    // Two-level derivation chain: memo of memo. Derivations settle in
    // dependency order within the inner loop; the reaction still runs once,
    // seeing a consistent (s, m1, m2) triple.
    let world = World::new();
    let runs = counter();
    let triples: Rc<RefCell<Vec<(i32, i32, i32)>>> = Rc::new(RefCell::new(Vec::new()));
    let s = world.enter(|| {
        let s = signal(1);
        let m1 = memo(move || s.get() * 10);
        let m2 = memo(move || m1.get() + 1);
        let runs = Rc::clone(&runs);
        let triples = Rc::clone(&triples);
        effect(move || {
            let t = (s.get(), m1.get(), m2.get());
            triples.borrow_mut().push(t);
            bump(&runs);
        });
        s
    });
    assert_eq!(runs.get(), 1);
    s.set(2);
    world.flush();
    assert_eq!(runs.get(), 2, "the whole chain settles with ONE reaction run");
    for &(sv, m1v, m2v) in triples.borrow().iter() {
        assert_eq!(m1v, sv * 10, "m1 stale relative to s");
        assert_eq!(m2v, m1v + 1, "m2 stale relative to m1");
    }
}

// ============================================================================
// NEW (P0): multi-world routing
// ============================================================================

#[test]
fn cross_world_write_stages_into_the_signals_world() {
    let a = World::new();
    let b = World::new();
    let sa = a.enter(|| signal(0));
    let a_runs = watch(&a, sa);
    let tb = b.enter(|| signal(1));
    // The B-world effect writes the A-world signal: the write must route to
    // A's queue no matter that B is ambient and running.
    b.enter(|| effect(move || sa.set(tb.get())));
    // The creation run already staged sa=1 — into A, invisibly.
    assert_eq!(sa.get(), 0);
    assert_eq!(a_runs.get(), 1);

    b.flush();
    assert_eq!(sa.get(), 0, "B's flush must not commit A's staged writes");
    assert_eq!(a_runs.get(), 1, "B's flush must not run A's effects");

    a.flush();
    assert_eq!(sa.get(), 1, "A's own flush delivers the cross-world write");
    assert_eq!(a_runs.get(), 2);

    tb.set(5);
    b.flush(); // re-runs the B-effect, staging sa=5 into A
    assert_eq!(sa.get(), 1);
    a.flush();
    assert_eq!(sa.get(), 5);
}

#[test]
fn cross_world_read_creates_no_subscription() {
    let a = World::new();
    let b = World::new();
    let sa = a.enter(|| signal(0));
    let b_runs = counter();
    b.enter(|| {
        let b_runs = Rc::clone(&b_runs);
        effect(move || {
            let _ = sa.get();
            bump(&b_runs);
        });
    });
    assert_eq!(b_runs.get(), 1);
    sa.set(7);
    a.flush();
    b.flush();
    assert_eq!(b_runs.get(), 1, "a B-effect reading an A-signal subscribes nothing in A");
}

#[test]
fn parallel_worlds_flush_and_drop_independently() {
    let worlds: Vec<World> = (0..3).map(|_| World::new()).collect();
    let sigs: Vec<Signal<i32>> = worlds.iter().map(|w| w.enter(|| signal(0))).collect();
    let runs: Vec<Rc<Cell<usize>>> =
        worlds.iter().zip(&sigs).map(|(w, &s)| watch(w, s)).collect();

    for s in &sigs {
        s.set(1);
    }
    worlds[1].flush();
    assert_eq!(
        (runs[0].get(), runs[1].get(), runs[2].get()),
        (1, 2, 1),
        "only the flushed world's effects ran"
    );
    assert_eq!((sigs[0].get(), sigs[1].get(), sigs[2].get()), (0, 1, 0));

    worlds[0].flush();
    worlds[2].flush();
    assert_eq!((runs[0].get(), runs[1].get(), runs[2].get()), (2, 2, 2));

    // Dropping one world leaves the others fully functional.
    let mut it = worlds.into_iter();
    let w0 = it.next().unwrap();
    let w1 = it.next().unwrap();
    let w2 = it.next().unwrap();
    drop(w0);
    sigs[1].set(9);
    w1.flush();
    assert_eq!((sigs[1].get(), runs[1].get()), (9, 3));
    sigs[2].set(9);
    w2.flush();
    assert_eq!((sigs[2].get(), runs[2].get()), (9, 3));
    sigs[0].set(9); // dead-world write: silent no-op
}

#[test]
fn dropping_one_world_leaves_others_working() {
    let w1 = World::new();
    let w2 = World::new();
    let s1 = w1.enter(|| signal(0));
    let s2 = w2.enter(|| signal(0));
    let runs2 = watch(&w2, s2);
    drop(w1);
    let _ = s1; // dead-world handle: writes no-op (covered elsewhere)
    s2.set(9);
    w2.flush();
    assert_eq!((s2.get(), runs2.get()), (9, 2), "surviving worlds are unaffected");
}

// ============================================================================
// NEW (P0): generational staleness + ownership teardown
// ============================================================================

#[test]
#[should_panic(expected = "idealyst[stale-signal-handle]")]
fn stale_signal_read_panics() {
    let world = World::new();
    let (s, owned) = world.enter(|| collect_owned(|| signal(1i32)));
    drop(owned); // frees the slot, bumping its generation
    // A fresh signal reuses the freed slot (different T — the generation
    // check is what makes this ABA-safe).
    let _fresh = world.enter(|| signal(7u64));
    let _ = s.get();
}

#[test]
#[should_panic(expected = "idealyst[stale-signal-handle]")]
fn stale_signal_write_panics() {
    // Stale writes in a LIVE world panic too (unlike dead-world writes,
    // which no-op): the world is running, so a write through a freed slot is
    // a use-after-unmount logic error worth surfacing loudly.
    let world = World::new();
    let (s, owned) = world.enter(|| collect_owned(|| signal(1i32)));
    drop(owned);
    s.set(2);
}

#[test]
fn dropping_owned_runs_cleanups_and_frees_slots() {
    let world = World::new();
    let cleaned = counter();
    let (s, owned) = world.enter(|| {
        collect_owned(|| {
            let s = signal(1);
            let cleaned = Rc::clone(&cleaned);
            effect(move || {
                let cleaned = Rc::clone(&cleaned);
                move || bump(&cleaned)
            });
            s
        })
    });
    assert_eq!(owned.len(), 2, "one signal + one effect collected");
    assert_eq!(cleaned.get(), 0);
    assert_eq!(s.get(), 1, "live while the Owned lives");
    drop(owned);
    assert_eq!(cleaned.get(), 1, "effect cleanups run at scope teardown");
    // The freed slots are recycled for new creations under a new generation.
    let s2 = world.enter(|| signal(99u64));
    assert_eq!(s2.get(), 99);
}

#[test]
fn nested_collectors_own_their_scopes_independently() {
    let world = World::new();
    let c_outer = counter();
    let c_inner = counter();
    let (inner_owned, outer_owned) = world.enter(|| {
        collect_owned(|| {
            let c = Rc::clone(&c_outer);
            effect(move || {
                let c = Rc::clone(&c);
                move || bump(&c)
            });
            let ((), inner_owned) = collect_owned(|| {
                let c = Rc::clone(&c_inner);
                effect(move || {
                    let c = Rc::clone(&c);
                    move || bump(&c)
                });
            });
            inner_owned
        })
    });
    assert_eq!(outer_owned.len(), 1, "the inner scope's effect is NOT in the outer collector");
    drop(outer_owned);
    assert_eq!((c_outer.get(), c_inner.get()), (1, 0), "outer teardown leaves the inner scope live");
    drop(inner_owned);
    assert_eq!((c_outer.get(), c_inner.get()), (1, 1));
}

// ============================================================================
// NEW (P0): guarded-set decomposition — set / set_always / touch /
// set_untracked, staged-commit flavor (mirrors runtime-core's
// reactive/tests.rs notify-semantics section, with flush as the window).
// ============================================================================

#[test]
fn set_skips_when_value_unchanged() {
    let world = World::new();
    let s = world.enter(|| signal(7i32));
    let runs = watch(&world, s);
    assert_eq!(runs.get(), 1, "initial effect run");
    s.set(7); // same value
    world.flush();
    assert_eq!(runs.get(), 1, "no re-run on no-op set");
    s.set(8); // real change
    world.flush();
    assert_eq!(runs.get(), 2, "re-run on real change");
}

#[test]
fn set_always_notifies_when_value_unchanged() {
    let world = World::new();
    let s = world.enter(|| signal(7i32));
    let runs = watch(&world, s);
    s.set_always(7); // same value, but `set_always` always notifies
    world.flush();
    assert_eq!(runs.get(), 2);
}

#[test]
fn touch_notifies_without_writing() {
    let world = World::new();
    let s = world.enter(|| signal(7i32));
    let runs = watch(&world, s);
    s.touch();
    world.flush();
    assert_eq!(runs.get(), 2, "touch wakes subscribers");
    assert_eq!(s.peek(), 7, "touch writes nothing");
}

#[test]
fn set_untracked_writes_without_notifying() {
    let world = World::new();
    let s = world.enter(|| signal(7i32));
    let runs = watch(&world, s);
    s.set_untracked(9);
    assert_eq!(s.peek(), 9, "the silent write hits the COMMITTED value immediately (no flush)");
    world.flush();
    assert_eq!(runs.get(), 1, "silent write must not fan out");
    // A later touch delivers the silently-written value.
    s.touch();
    world.flush();
    assert_eq!(runs.get(), 2);
}

#[test]
fn set_untracked_then_equal_set_skips_fanout() {
    // The guarded set compares against the CURRENT committed value,
    // including one put there by a silent write: set_untracked(9) then
    // set(9) is a no-op fan-out-wise.
    let world = World::new();
    let s = world.enter(|| signal(7i32));
    let runs = watch(&world, s);
    s.set_untracked(9);
    s.set(9);
    world.flush();
    assert_eq!(runs.get(), 1, "equal to silently-written value → no fan-out");
}

#[test]
fn set_net_zero_flush_window_skips_fanout() {
    // A → B → A within one flush window nets to no change: last-write-wins
    // staging gives window-initial comparison for free.
    let world = World::new();
    let s = world.enter(|| signal(1i32));
    let runs = watch(&world, s);
    s.set(2);
    s.set(1); // back to the committed value
    world.flush();
    assert_eq!(runs.get(), 1, "net-zero window must not fan out");
}

#[test]
fn set_net_change_flush_fires_once() {
    let world = World::new();
    let s = world.enter(|| signal(1i32));
    let runs = watch(&world, s);
    s.set(2);
    s.set(3); // net change 1 → 3
    world.flush();
    assert_eq!(runs.get(), 2, "net change fans out exactly once");
    assert_eq!(s.get(), 3);
}

#[test]
fn set_always_taints_flush_window_forcing_notify() {
    let world = World::new();
    let s = world.enter(|| signal(1i32));
    let runs = watch(&world, s);
    s.set(1); // no-op on its own...
    s.set_always(1); // ...but a force-write taints the window
    world.flush();
    assert_eq!(runs.get(), 2, "force-write notifies despite net-zero");
}

#[test]
fn touch_taints_flush_window_forcing_notify() {
    let world = World::new();
    let s = world.enter(|| signal(1i32));
    let runs = watch(&world, s);
    s.set(1); // net-zero on its own...
    s.touch(); // ...but touch taints the window
    world.flush();
    assert_eq!(runs.get(), 2, "touch notifies despite net-zero window");
    assert_eq!(s.get(), 1);
}

#[test]
fn set_nan_always_notifies() {
    // NaN != NaN, so a NaN-valued commit is never "unchanged".
    let world = World::new();
    let s = world.enter(|| signal(0.0f64));
    let runs = watch(&world, s);
    s.set(f64::NAN);
    world.flush();
    assert_eq!(runs.get(), 2, "0.0 → NaN is a change");
    s.set(f64::NAN);
    world.flush();
    assert_eq!(runs.get(), 3, "NaN → NaN still notifies (NaN != NaN)");
}

// ============================================================================
// NEW (P0): with() borrow-reads, untrack globality, flush introspection
// ============================================================================

/// Deliberately NOT Clone: `with`/`set` compiling against it proves the
/// borrow-read path never clones.
#[derive(PartialEq)]
struct Opaque(i32);

#[test]
fn with_reads_without_cloning_and_tracks() {
    let world = World::new();
    let s = world.enter(|| signal(Opaque(1)));
    let seen = Rc::new(Cell::new(0));
    let untracked_runs = counter();
    world.enter(|| {
        let seen = Rc::clone(&seen);
        effect(move || seen.set(s.with(|v| v.0)));
        let untracked_runs = Rc::clone(&untracked_runs);
        effect(move || {
            let _ = s.with_untracked(|v| v.0);
            bump(&untracked_runs);
        });
    });
    assert_eq!(seen.get(), 1);
    s.set(Opaque(2));
    world.flush();
    assert_eq!(seen.get(), 2, "with() is a tracked read");
    assert_eq!(untracked_runs.get(), 1, "with_untracked() subscribes nothing");
    s.update(|v| Opaque(v.0 + 1)); // update() needs no Clone either
    world.flush();
    assert_eq!(seen.get(), 3);
}

#[test]
fn untrack_suspends_tracking_globally_across_worlds() {
    // The documented untrack decision: suspension is a property of the code
    // region (a global depth counter), not of the ambient world. Entering
    // ANOTHER world changes only the creation ambient — a tracked read of
    // the running effect's own-world signal still subscribes (first effect),
    // and untrack must suppress exactly that (second effect).
    let a = World::new();
    let b = World::new();
    let sa = a.enter(|| signal(0));
    let tracked_runs = counter();
    let untracked_runs = counter();
    a.enter(|| {
        let b2 = b.clone();
        let tracked_runs = Rc::clone(&tracked_runs);
        effect(move || {
            b2.enter(|| {
                let _ = sa.get(); // A-signal read from an A-effect, via B's ambient
            });
            bump(&tracked_runs);
        });
        let b2 = b.clone();
        let untracked_runs = Rc::clone(&untracked_runs);
        effect(move || {
            b2.enter(|| untrack(|| { let _ = sa.get(); }));
            bump(&untracked_runs);
        });
    });
    sa.set(1);
    a.flush();
    assert_eq!(tracked_runs.get(), 2, "enter() must not affect tracking — the read subscribed");
    assert_eq!(untracked_runs.get(), 1, "untrack suspends tracking regardless of the ambient world");
}

#[test]
fn is_flushing_reports_active_flush() {
    let world = World::new();
    let s = world.enter(|| signal(0));
    let during = Rc::new(Cell::new(false));
    world.enter(|| {
        let during = Rc::clone(&during);
        effect(move || {
            s.get();
            during.set(is_flushing());
        });
    });
    assert!(!during.get(), "the creation run happens outside any flush");
    assert!(!is_flushing());
    assert!(!world.is_flushing());
    s.set(1);
    world.flush();
    assert!(during.get(), "effects re-run by flush see is_flushing() == true");
    assert!(!is_flushing(), "cleared once the flush returns");
    assert!(!world.is_flushing());
}

#[test]
#[should_panic(expected = "idealyst[reentrant-flush]")]
fn reentrant_flush_panics() {
    let world = World::new();
    let s = world.enter(|| signal(0));
    let w2 = world.clone();
    world.enter(|| {
        effect(move || {
            if s.get() == 1 {
                w2.flush(); // same-world flush from inside its own effect
            }
        });
    });
    s.set(1);
    world.flush();
}

#[test]
fn handles_are_copy() {
    fn assert_copy<T: Copy>() {}
    assert_copy::<Signal<String>>();
    assert_copy::<ReadSignal<Vec<u8>>>();
    assert_copy::<WriteSignal<String>>();
    assert_copy::<Effect>();
    assert_copy::<Memo<String>>();
}

// ============================================================================
// NEW (P1 support): nested-effect tracking context + Owned::merge
// ============================================================================

#[test]
fn effect_created_inside_untrack_still_tracks() {
    // Regression for the P1 structural drivers: subtree builds run inside
    // `untrack()` (the old walker's `untrack_for_build` contract), and every
    // nested driver/binding effect is CREATED — and first-run — inside that
    // window. That first run is the only dependency-collection pass the
    // effect gets, so its body must be a fresh tracking context. Before
    // `run_effect` reset `untrack_depth`, this effect never re-ran.
    let world = World::new();
    let runs = counter();
    let s = world.enter(|| signal(0));
    world.enter(|| {
        untrack(|| {
            let runs = Rc::clone(&runs);
            effect(move || {
                let _ = s.get();
                bump(&runs);
            });
        });
    });
    assert_eq!(runs.get(), 1);
    s.set(1);
    world.flush();
    assert_eq!(
        runs.get(),
        2,
        "the nested effect's body is its own tracking context — it subscribed despite the enclosing untrack"
    );
}

#[test]
fn untrack_window_restored_after_nested_effect_run() {
    // The inverse guard: once the nested effect's first run returns, the
    // enclosing untrack window is back in force — reads after the nested
    // creation still subscribe nothing.
    let world = World::new();
    let outer_runs = counter();
    let s = world.enter(|| signal(0));
    world.enter(|| {
        let outer_runs = Rc::clone(&outer_runs);
        effect(move || {
            bump(&outer_runs);
            untrack(|| {
                effect(move || {}); // nested creation resets tracking for ITS body only
                let _ = s.get(); // must still be untracked out here
            });
        });
    });
    s.set(1);
    world.flush();
    assert_eq!(
        outer_runs.get(),
        1,
        "the untracked read after the nested effect's run did not subscribe the outer effect"
    );
}

#[test]
fn owned_merge_folds_scopes_into_one_drop() {
    // The scene layer folds Element::Owned component scopes into the
    // enclosing Realized's Owned. Merged slots must (a) survive the donor
    // Owned's death and (b) all be freed — cleanups first — when the
    // merged-into Owned drops.
    let world = World::new();
    let drops = counter();
    let (host_sig, mut host) = world.enter(|| collect_owned(|| signal(1)));
    let (donor_probe, donor) = world.enter(|| {
        let drops = Rc::clone(&drops);
        collect_owned(move || {
            let drops = Rc::clone(&drops);
            effect(move || {
                let drops = Rc::clone(&drops);
                move || bump(&drops)
            });
            signal(2)
        })
    });
    host.merge(donor); // donor's emptied shell drops here — must be a no-op
    assert_eq!(drops.get(), 0, "merge itself tears nothing down");
    assert_eq!(host.len(), 3, "host now owns its signal + the donor's effect and signal");
    assert_eq!(donor_probe.get(), 2, "donor slots stay live after the merge");
    drop(host);
    assert_eq!(drops.get(), 1, "dropping the merged-into Owned runs the donor's cleanups");
    let host_sig = host_sig; // both scopes' slots are gone: stale handles panic
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| host_sig.get()));
    assert!(panicked.is_err(), "the host's own slot was freed too");
}

#[test]
fn unscoped_creations_survive_the_enclosing_collector() {
    // The world-lifetime-service escape hatch (new-core style engine):
    // a signal/effect created via `unscoped` inside a `collect_owned`
    // scope must NOT be collected — it lives until the world drops.
    let world = World::new();
    let fires = counter();
    let (service_sig, owned) = world.enter(|| {
        collect_owned(|| {
            let fires = Rc::clone(&fires);
            // World-root-owned: survives the Owned drop below.
            let sig = unscoped(|| signal(1));
            unscoped(move || {
                effect(move || {
                    let _ = sig.get();
                    bump(&fires);
                })
            });
            // Collected: dies with the Owned.
            let _scoped = signal(0);
            sig
        })
    });
    assert_eq!(fires.get(), 1, "driver-style effect ran once at creation");
    drop(owned);
    // Both the signal and the effect survived the collector's death.
    world.enter(|| service_sig.set(2));
    world.flush();
    assert_eq!(fires.get(), 2, "unscoped effect still re-fires after the scope died");
    assert_eq!(service_sig.get(), 2, "unscoped signal still lives after the scope died");
}

#[test]
fn unscoped_restores_the_collector_stack() {
    // A collect_owned around an unscoped region still collects what's
    // created AFTER the region — the stack is restored verbatim.
    let world = World::new();
    world.enter(|| {
        let (sig_after, owned) = collect_owned(|| {
            unscoped(|| signal(10));
            signal(20)
        });
        assert_eq!(owned.len(), 1, "only the post-unscoped creation was collected");
        assert_eq!(sig_after.get(), 20);
    });
}
