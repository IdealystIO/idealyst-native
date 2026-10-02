//! Phase 3: a bundle's reactive kernel running on the HOST's graph, over
//! wasm. `spike-kernelguest` is plain runtime-world code; built as a bundle,
//! its engine is the bridged one, so everything below is observed from the
//! host side: the host's world, the host's flush, the host's scopes.

use runtime_world::World;
use stream_host::kernel::KernelBundle;
use stream_spike::KERNEL_GUEST_WASM;

fn load() -> KernelBundle {
    let engine = stream_spike::full::engine(wasmi::CompilationMode::LazyTranslation);
    KernelBundle::load(&engine, KERNEL_GUEST_WASM).expect("kernel bundle loads")
}

/// The bundle's signal lives in the world the HOST entered — there is no
/// second graph.
#[test]
fn bundle_signals_live_in_the_hosts_world() {
    let b = load();
    let world = World::new();
    world.enter(|| b.call::<(), ()>("kg_setup", ()));
    assert_eq!(b.call::<(), i64>("kg_count_world", ()), (world.id() & 0xff) as i64);
}

/// Creating an effect runs its body at once — a call back into the bundle
/// while `kg_setup` is still running, which only works through the import's
/// own `Caller` (the re-entrant path).
#[test]
fn effect_created_mid_call_runs_immediately() {
    let b = load();
    let world = World::new();
    world.enter(|| b.call::<(), ()>("kg_setup", ()));
    assert_eq!(b.call::<(), i64>("kg_effect_runs", ()), 1);
    assert_eq!(b.call::<(), i64>("kg_effect_saw", ()), 0);
}

/// A bundle write is STAGED in the host's graph: invisible until the host
/// flushes, and then the bundle's memo and effect follow — run by the host's
/// flush, from host code, outside any bundle call.
#[test]
fn host_flush_commits_bundle_writes_and_reruns_bundle_effects() {
    let b = load();
    let world = World::new();
    world.enter(|| b.call::<(), ()>("kg_setup", ()));
    b.call::<(), ()>("kg_bump", ());
    b.call::<(), ()>("kg_bump", ());
    assert_eq!(b.call::<(), i64>("kg_count", ()), 0, "staged, not committed");

    world.flush();
    assert_eq!(b.call::<(), i64>("kg_count", ()), 2, "two updates compose on the staged value");
    assert_eq!(b.call::<(), i64>("kg_doubled", ()), 4, "the memo is a host derivation");
    assert_eq!(b.call::<(), i64>("kg_effect_runs", ()), 2, "one re-run per flush");
    assert_eq!(b.call::<(), i64>("kg_effect_saw", ()), 4, "the effect saw the settled memo");
}

/// Ownership is the host's: a scope collected in the bundle holds host
/// slots; dropping it runs the bundle's cleanup and retracts the bundle's
/// context provision.
#[test]
fn bundle_scopes_cleanups_and_context_ride_the_hosts_ownership() {
    let b = load();
    let world = World::new();
    let seen = world.enter(|| b.call::<(), i64>("kg_scope_open", ()));
    assert_eq!(seen, 7, "inject sees the bundle's own provision");
    assert_eq!(b.call::<(), i64>("kg_scope_len", ()), 3, "signal + effect + context entry");
    assert_eq!(b.call::<(), i64>("kg_cleanups", ()), 0);

    let after = world.enter(|| b.call::<(), i64>("kg_scope_drop", ()));
    assert_eq!(b.call::<(), i64>("kg_cleanups", ()), 1, "the effect's cleanup ran on scope drop");
    assert_eq!(after, -1, "the provision was retracted with its scope");
}

/// Two instances of the same bundle share one host graph without their
/// ids (values, effects, context keys) colliding — every id is namespaced
/// by bundle on the host.
#[test]
fn two_bundles_share_one_graph_without_colliding() {
    let a = load();
    let b = load();
    let world = World::new();
    world.enter(|| {
        a.call::<(), ()>("kg_setup", ());
        b.call::<(), ()>("kg_setup", ());
    });
    a.call::<(), ()>("kg_bump", ());
    world.flush();
    assert_eq!((a.call::<(), i64>("kg_count", ()), b.call::<(), i64>("kg_count", ())), (1, 0));
    assert_eq!((a.call::<(), i64>("kg_effect_runs", ()), b.call::<(), i64>("kg_effect_runs", ())), (2, 1));

    let seen_a = world.enter(|| a.call::<(), i64>("kg_scope_open", ()));
    let seen_b = world.enter(|| b.call::<(), i64>("kg_scope_open", ()));
    assert_eq!((seen_a, seen_b), (7, 7));
    world.enter(|| a.call::<(), i64>("kg_scope_drop", ()));
    assert_eq!(
        (a.call::<(), i64>("kg_cleanups", ()), b.call::<(), i64>("kg_cleanups", ())),
        (1, 0),
        "dropping one bundle's scope must not touch the other's"
    );
}

/// Dropping the host world tears the bundle's state down through the
/// proxies (cleanups run, values released) without a trap.
#[test]
fn dropping_the_host_world_tears_the_bundle_state_down() {
    let b = load();
    let world = World::new();
    world.enter(|| {
        b.call::<(), ()>("kg_setup", ());
        b.call::<(), i64>("kg_scope_open", ());
    });
    drop(world);
    assert_eq!(b.call::<(), i64>("kg_cleanups", ()), 1);
}
