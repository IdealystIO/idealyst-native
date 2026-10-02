//! Phase 3: a bundle's reactive kernel running on the HOST's graph, over
//! wasm. `spike-kernelguest` is plain runtime-world code; built as a bundle,
//! its engine is the bridged one, so everything below is observed from the
//! host side: the host's world, the host's flush, the host's scopes.

use runtime_world::World;
use stream_host::kernel::KernelBundle;
use stream_spike::KERNEL_GUEST_WASM;

fn load() -> KernelBundle {
    let engine = stream_host::remote::engine();
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

// ---------------------------------------------------------------------------
// Phase 3b: host-owned props and context crossing into the bundle — the
// reactive half of RemoteCounter, on the host's graph.
// ---------------------------------------------------------------------------

use runtime_world::{provide, ReadSignal, Signal};
use stream_abi::Wire;
use stream_host::kernel::{export_context, export_read_signal, export_signal, ExportGuard};

/// The host's context type. A shared crate would define it once for both
/// sides; here the host declares its encoding (the handle of the user
/// signal) under the name the bundle registered.
#[derive(Clone)]
struct HostUser(ReadSignal<String>, (u32, u32, u32));

struct Counter {
    bundle: KernelBundle,
    world: World,
    external: Signal<i64>,
    value: Signal<i64>,
    user: Signal<String>,
    _guards: Vec<ExportGuard>,
}

impl Counter {
    fn mount() -> Counter {
        let bundle = load();
        let world = World::new();
        let external = world.signal(5i64);
        let value = world.signal(100i64);
        let user = world.signal("ada".to_string());
        let (eh, g1) = export_read_signal(external.read_only());
        let (vh, g2) = export_signal(value);
        let (uh, g3) = export_read_signal(user.read_only());
        let g4 = export_context::<HostUser>("CurrentUser", |u, out| {
            for v in [u.1 .0, u.1 .1, u.1 .2] {
                v.encode(out);
            }
        });
        world.enter(|| {
            provide(HostUser(user.read_only(), uh));
            bundle.call::<(u32, u32, u32, u32, u32, u32), ()>("kg_counter_mount", (eh.0, eh.1, eh.2, vh.0, vh.1, vh.2));
        });
        Counter { bundle, world, external, value, user, _guards: vec![g1, g2, g3, g4] }
    }

    fn line(&self) -> String {
        let packed = self.bundle.call::<(), i64>("kg_counter_line", ());
        let bytes = self.bundle.read_memory((packed >> 32) as u32, packed as u32);
        String::from_utf8(bytes).unwrap()
    }
}

#[test]
fn bundle_renders_from_host_props_and_host_context() {
    let c = Counter::mount();
    assert_eq!(c.line(), "external: 5 clicks: 0 value: 100 user: ada");
}

/// A host prop changes → host flush → the bundle's effect re-ran with it.
#[test]
fn host_prop_change_rerenders_the_bundle() {
    let c = Counter::mount();
    c.external.set(6);
    c.world.flush();
    assert_eq!(c.line(), "external: 6 clicks: 0 value: 100 user: ada");
}

/// Host context holding a signal stays reactive across the boundary.
#[test]
fn host_context_change_rerenders_the_bundle() {
    let c = Counter::mount();
    c.user.set("grace".to_string());
    c.world.flush();
    assert_eq!(c.line(), "external: 5 clicks: 0 value: 100 user: grace");
}

/// The bundle writes a two-way host prop: the host sees it, composed, after
/// its own flush — and the bundle's own render follows.
#[test]
fn bundle_writes_a_two_way_host_prop() {
    let c = Counter::mount();
    c.bundle.call::<(), ()>("kg_counter_bump_value", ());
    c.bundle.call::<(), ()>("kg_counter_bump_value", ());
    assert_eq!(c.value.get(), 100, "staged on the host");
    c.world.flush();
    assert_eq!(c.value.get(), 102);
    assert_eq!(c.line(), "external: 5 clicks: 0 value: 102 user: ada");
}

/// Bundle-local state and host state in one effect, one flush.
#[test]
fn bundle_local_state_and_host_props_settle_together() {
    let c = Counter::mount();
    c.bundle.call::<(), ()>("kg_counter_click", ());
    c.external.set(9);
    c.world.flush();
    assert_eq!(c.line(), "external: 9 clicks: 1 value: 100 user: ada");
}
