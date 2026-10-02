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
// In a bundle build the host side is compiled (the parity check names it)
// but not used — the host is the app, on the far side of the wasm boundary.
#[cfg_attr(all(idealyst_stream_guest, not(all(feature = "bridge", any(not(target_arch = "wasm32"), idealyst_stream_guest)))), allow(dead_code))]
pub(crate) mod host;
/// The bundle side over wasm: what a remote bundle's kernel runs on.
#[cfg(idealyst_stream_guest)]
pub(crate) mod wasm;

use crate::engine::{EffectClass, WorldId};

/// `(world, slot, gen)` — a slot's identity, as the host's engine assigned it.
/// The bridged engine's handles are the host's handles.
pub type Handle = (WorldId, u32, u32);

/// An id for something that lives on the bundle side (a value, an effect
/// body, a cleanup, a context value) or a bundle's context key. 64 bits so a
/// host serving several bundles over one graph can NAMESPACE them — bundle
/// in the high half, the bundle's own id in the low half — and route each
/// proxy's callback to the right bundle (see the wasm transport in
/// crates/streaming). In-process (loopback) the bundle's id is used as is.
pub type Id = u64;

/// The bridged engine wired to a native host in the same process: what
/// `--features loopback-engine` runs the kernel suite against.
pub(crate) type Loopback = guest::Bridged<host::Host<guest::Local>>;

/// Bundle → host. Each operation is the graph half of an `Engine` method;
/// see that method for semantics. Plain data only (see the module docs).
pub trait HostOps: 'static {
    fn world_new() -> WorldId;
    fn world_drop(world: WorldId);
    fn world_flush(world: WorldId);
    fn world_is_flushing(world: WorldId) -> bool;
    /// World-lifetime context entry `ctx` (a bundle-side value id) under the
    /// bundle's context key `key`.
    fn world_provide(world: WorldId, key: Id, ctx: Id);
    /// The bundle-side value id of the newest provision of `key`.
    fn world_inject(world: WorldId, key: Id) -> Option<Id>;
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
    fn signal_create(world: Option<WorldId>, value: Id) -> (Handle, bool);
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
    fn effect_create(world: Option<WorldId>, class: EffectClass, effect: Id) -> Handle;
    fn effect_is_alive(h: Handle) -> bool;
    /// Register bundle-side cleanup `cleanup` on the running effect.
    fn on_cleanup(cleanup: Id);

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

    fn ctx_provide(key: Id, ctx: Id);
    fn ctx_inject(key: Id) -> Option<Id>;

    // ---- host-owned values (props and context crossing into a bundle) ----

    /// Encode the host-owned signal `h`'s committed value (or, with
    /// `staged`, its staged write) into `out`. `false` when `h` was not
    /// exported to bundles, or `staged` and nothing is staged.
    fn value_fetch(h: Handle, staged: bool, out: &mut Vec<u8>) -> bool;
    /// Write `bytes` into the host-owned signal `h` the way `mode` says.
    fn value_stage(h: Handle, bytes: &[u8], mode: StageMode);
    /// Encode the host context declared to bundles under `name` into `out`;
    /// `false` when none is declared or none is provided right now.
    fn ctx_fetch(name: &str, out: &mut Vec<u8>) -> bool;
}

/// How a bundle's write lands in a host-owned signal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StageMode {
    /// `set`: staged, equality-guarded.
    Set,
    /// `set_always`: staged, notifies even if equal.
    SetAlways,
    /// `set_untracked`: committed directly, notifies nobody.
    Untracked,
}

/// How an IMPORTED value (one the host owns, mirrored in the bundle) moves
/// between its host slot and the bundle's mirror. Implemented by the typed
/// layer, which knows `T` and holds the codec. The mirror is the typed
/// layer's own storage type, so reads and writes through a handle to an
/// imported signal are the same code as for any other.
pub(crate) trait ImportSync {
    /// Overwrite the mirror from host bytes: its committed value, and its
    /// staged value (`None` = nothing staged).
    fn pull(&self, mirror: &mut dyn crate::engine::AnySignal, committed: &[u8], staged: Option<&[u8]>);
    /// Take the mirror's staged write, encoded, if the operation made one.
    fn take_next(&self, mirror: &mut dyn crate::engine::AnySignal, out: &mut Vec<u8>) -> bool;
    /// Encode the staged write, if any, leaving it staged.
    fn encode_next(&self, mirror: &mut dyn crate::engine::AnySignal, out: &mut Vec<u8>) -> bool;
    /// Encode the mirror's committed value.
    fn encode_value(&self, mirror: &mut dyn crate::engine::AnySignal, out: &mut Vec<u8>);
}

/// Host → bundle: the proxies' calls back into the side that owns the value
/// or closure. Plain data only.
pub trait GuestHooks: 'static {
    /// Commit value `value`'s staged write; whether subscribers must hear.
    fn commit(value: Id, forced: bool) -> bool;
    /// The host freed the slot holding `value`.
    fn drop_value(value: Id);
    fn run_effect(effect: Id);
    fn drop_effect(effect: Id);
    fn run_cleanup(cleanup: Id);
    fn drop_cleanup(cleanup: Id);
    fn drop_context(ctx: Id);
    /// First half of PROMOTION (see `remote::receive_signal`): encode bundle
    /// value `value`'s committed value into `committed` and its staged
    /// write, if any, into `staged`, changing nothing. `None` when the
    /// bundle did not offer it (`remote_guest::offer_signal`); else whether
    /// a staged write was encoded.
    fn promote(value: Id, committed: &mut Vec<u8>, staged: &mut Vec<u8>) -> Option<bool>;
    /// Second half, once the host holds the value natively: from now on
    /// treat `value` as an import of the host's slot (its staged write now
    /// lives on the host). Two halves so a host that cannot decode the
    /// value leaves the bundle exactly as it was.
    fn promote_finish(value: Id);
}
