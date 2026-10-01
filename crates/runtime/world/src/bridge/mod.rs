//! The kernel bridge: a second [`Engine`](crate::engine::Engine) that runs
//! the typed layer against ANOTHER engine's graph.
//!
//! A remote component (crates/streaming) runs in a wasm bundle but must share
//! the host app's one reactive graph — the app's signals as props, the app's
//! flush, the app's ownership scopes. So inside the bundle the kernel's
//! engine is [`guest::Bridged`]: it keeps everything that cannot cross a wasm
//! boundary — signal VALUES, effect BODIES, cleanup and context closures — in
//! local tables keyed by id, and forwards every graph operation (slots,
//! subscriptions, staging, flush, scopes, context stacks) to the host's
//! native engine through [`HostOps`]. On the host, [`host::Host`] implements
//! those operations over the native arena, holding a PROXY in every slot the
//! bundle owns: a value proxy whose `commit` asks the bundle "did yours
//! change?", an effect proxy that asks the bundle to run its body, cleanup
//! and context proxies that release the bundle's side when dropped. The host
//! reaches the bundle through [`GuestHooks`].
//!
//! # Plain data only
//!
//! Every argument and result of [`HostOps`] and [`GuestHooks`] is plain data:
//! ids, integers, flags. That is the contract a wasm import/export pair can
//! carry, and it is enforced here, in-process, so the bridge is exercised
//! exactly as it will be used — not through a convenient shortcut that only
//! works when both sides share an address space.
//!
//! # Loopback: the parity proof
//!
//! [`Loopback`] wires [`guest::Bridged`] straight to [`host::Host`] in one
//! process. With the `loopback-engine` feature, the crate's `Active` engine
//! IS the loopback, so the entire kernel test suite runs through the bridge:
//!
//! ```sh
//! cargo test -p runtime-world --features loopback-engine
//! ```
//!
//! Together with the `Engine` trait (a contract change does not compile
//! until every engine implements it), that is what keeps the bridged engine
//! from drifting from the native one.

pub(crate) mod guest;
pub(crate) mod host;

use crate::engine::{EffectClass, WorldId};

/// `(world, slot, gen)` — a slot's identity, as the host's engine assigned it.
/// The bridged engine's handles are the host's handles.
pub(crate) type Handle = (WorldId, u32, u32);

/// The bridged engine wired to a native host in the same process: what
/// `--features loopback-engine` runs the kernel suite against.
pub(crate) type Loopback = guest::Bridged<host::Host<guest::Local>>;

/// Bundle → host. Each operation is the graph half of an `Engine` method;
/// see that method for semantics. Plain data only (see the module docs).
pub(crate) trait HostOps: 'static {
    fn world_new() -> WorldId;
    fn world_drop(world: WorldId);
    fn world_flush(world: WorldId);
    fn world_is_flushing(world: WorldId) -> bool;
    /// World-lifetime context entry `ctx` (a bundle-side value id) under the
    /// bundle's context key `key`.
    fn world_provide(world: WorldId, key: u32, ctx: u32);
    /// The bundle-side value id of the newest provision of `key`.
    fn world_inject(world: WorldId, key: u32) -> Option<u32>;
    fn enter_push(world: WorldId);
    fn enter_pop();

    fn is_flushing() -> bool;
    fn is_entered() -> bool;
    fn in_effect() -> bool;
    fn effect_depth() -> u32;
    fn current_effect() -> Option<Handle>;
    fn in_collector() -> bool;

    /// A slot whose value is bundle-side value `value`. Returns the slot and
    /// whether a collector took it.
    fn signal_create(world: Option<WorldId>, value: u32) -> (Handle, bool);
    /// Liveness before a bundle-side access: `false` for a dead world;
    /// panics (the stale-handle diagnostic) for a freed slot.
    fn signal_check(h: Handle) -> bool;
    /// [`signal_check`](HostOps::signal_check) for a read, subscribing the
    /// running effect first when `track`. `None` for a dead world, else
    /// whether the read subscribed.
    fn signal_read_check(h: Handle, track: bool) -> Option<bool>;
    /// Stage the slot for commit at the next flush.
    fn signal_enqueue(h: Handle, force: bool);
    fn signal_touch(h: Handle);
    fn signal_is_alive(h: Handle) -> bool;
    fn signal_subscriber_count(h: Handle) -> u32;

    /// An effect whose body is bundle-side effect `effect`. Runs it once.
    fn effect_create(world: Option<WorldId>, class: EffectClass, effect: u32) -> Handle;
    fn effect_is_alive(h: Handle) -> bool;
    /// Register bundle-side cleanup `cleanup` on the running effect.
    fn on_cleanup(cleanup: u32);

    fn untrack_push();
    fn untrack_pop();
    fn unscoped_begin();
    fn unscoped_end();
    fn unanchored_begin();
    fn unanchored_end();

    fn collect_begin();
    /// Close the innermost collection; `0` when it collected nothing,
    /// otherwise an id naming the host-side scope.
    fn collect_end() -> u32;
    /// Close the innermost collection and free what it collected (the
    /// panic path of `collect_owned`).
    fn collect_abort();
    fn scope_merge(into: u32, other: u32);
    fn scope_len(scope: u32) -> u32;
    fn scope_drop(scope: u32);

    fn ctx_provide(key: u32, ctx: u32);
    fn ctx_inject(key: u32) -> Option<u32>;
}

/// Host → bundle: the proxies' calls back into the side that owns the value
/// or closure. Plain data only.
pub(crate) trait GuestHooks: 'static {
    /// Commit value `value`'s staged write; whether subscribers must hear.
    fn commit(value: u32, forced: bool) -> bool;
    /// The host freed the slot holding `value`.
    fn drop_value(value: u32);
    fn run_effect(effect: u32);
    fn drop_effect(effect: u32);
    fn run_cleanup(cleanup: u32);
    fn drop_cleanup(cleanup: u32);
    fn drop_context(ctx: u32);
}
