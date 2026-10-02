//! The kernel's ENGINE seam.
//!
//! runtime-world is two layers:
//!
//! - the **typed layer** (`lib.rs`): `Signal<T>` / `ReadSignal<T>` /
//!   `WriteSignal<T>`, `SignalData<T>` and its `PartialEq` commit, `memo`,
//!   `Value`, `IntoCleanup`, `on_scope_drop`, `Owned`'s public surface, the
//!   staged-read diagnostic. Written once, generic over values.
//! - the **engine** (this trait): everything untyped — slots and
//!   generations, value-box storage, subscriptions, the running-effect
//!   stack, staging and the glitch-free flush, ownership collectors and
//!   scopes, context stacks, and the probes over all of it.
//!
//! The typed layer reaches the engine ONLY through [`Engine`], via the
//! `Active` alias in `lib.rs`. That is the point of the seam: there is more
//! than one engine, and a change to the kernel's contract must be made to
//! every one of them or the crate does not build.
//!
//! - [`crate::native::Native`] — the in-process arena. What every app runs.
//! - the bridged engine (remote components, `crates/streaming`) keeps values
//!   and closures where they were created and forwards every graph operation
//!   to a native engine in the host app, so a remote component shares the
//!   app's one reactive graph instead of running a second one.
//!
//! The trait is static (associated functions, generic methods, no `self`):
//! an engine is per-thread state, there is one active engine per build, and
//! dispatch must compile to exactly what the direct calls compiled to — the
//! signal-read path is the framework's hottest code.
//!
//! Values cross this seam only as `Box<dyn AnySignal>` (the typed layer's
//! `SignalData<T>`, erased), closures only as boxed `FnMut` / `FnOnce`, and
//! context only as `(TypeId, Box<dyn Any>)`. Nothing here knows a `T`.

use std::any::{Any, TypeId};

use crate::SiteLoc;

/// Non-zero id of a live (or dead) world. Baked into every handle.
pub type WorldId = u32;

/// Scheduling class — the key to glitch-free flushing (see `World::flush`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EffectClass {
    /// A memo recompute: settles BEFORE reactions each round.
    Derivation,
    /// Bindings, user effects, structural drivers: run once per round, after
    /// every derivation has settled.
    Reaction,
}

/// Type-erased signal storage, so one engine can hold signals of any `T`.
/// Implemented by the typed layer's `SignalData<T>`; the engine only ever
/// commits it and hands it back.
pub(crate) trait AnySignal: Any {
    /// Flip `next` into `value` if it's an effective change (or `forced`);
    /// return whether subscribers must be notified.
    fn commit(&mut self, forced: bool) -> bool;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    /// The payload box as `Box<dyn Any>`, so a caller that has lost `T`
    /// can downcast back to `SignalData<T>`.
    ///
    /// Trait-object upcasting would give this for free on a new enough
    /// toolchain; spelling it out keeps the crate's `rust-version`
    /// floor where it is. Only the hot-reload harvest uses it.
    #[cfg(feature = "hot-reload")]
    fn into_any(self: Box<Self>) -> Box<dyn Any>;
    /// `TypeId::of::<T>()` for the payload. Recovered from the trait
    /// object because the harvest side has no `T`.
    #[cfg(feature = "hot-reload")]
    fn value_type_id(&self) -> std::any::TypeId;
}

/// What [`Engine::signal_access`] found. The typed layer turns the two
/// failure cases into its own policy: a READ of a dead world panics (there
/// is no committed value to return), a WRITE to one is a silent no-op (an
/// app or request teardown racing an async write).
pub(crate) enum Access<R> {
    Done(R),
    DeadWorld,
}

/// The engine contract. Every method is the untyped half of something the
/// public API does; the doc on each names the public API it backs, so a
/// semantic question about an engine method is answered by that API's docs.
pub(crate) trait Engine {
    /// An ownership scope's collected items, as this engine represents
    /// them. Backs [`crate::Owned`].
    type Scope: Default;

    // ---- worlds -------------------------------------------------------

    /// `World::new`: register a fresh, empty world.
    fn world_new() -> WorldId;
    /// Last `World` clone dropped: run every live effect's cleanups while
    /// the world is still registered, then unregister it.
    fn world_drop(world: WorldId);
    /// `World::enter`: make `world` the ambient creation context for `f`
    /// (a stack; panic-safe restore).
    fn world_enter<R>(world: WorldId, f: impl FnOnce() -> R) -> R;
    /// `World::flush`.
    fn world_flush(world: WorldId);
    /// `World::is_flushing`.
    fn world_is_flushing(world: WorldId) -> bool;
    /// `World::provide`: a world-lifetime context entry.
    fn world_provide(world: WorldId, key: TypeId, value: Box<dyn Any>);
    /// `World::inject`: `f` sees the newest provision of `key`, if any.
    fn world_inject<R>(world: WorldId, key: TypeId, f: impl FnOnce(&dyn Any) -> R) -> Option<R>;

    // ---- probes -------------------------------------------------------

    /// `is_flushing()`.
    fn is_flushing() -> bool;
    /// `is_entered()`.
    fn is_entered() -> bool;
    /// `in_effect()`.
    fn in_effect() -> bool;
    /// `effect_depth()`.
    fn effect_depth() -> usize;
    /// `current_effect()`, as `(world, slot, gen)`.
    fn current_effect() -> Option<(WorldId, u32, u32)>;
    /// `in_collector()`.
    fn in_collector() -> bool;

    // ---- signals ------------------------------------------------------

    /// Allocate a signal slot holding `data` in `world` (`None` = the
    /// ambient world, resolved once — creation is hot), and register it
    /// with the innermost ownership collector. Returns `(world, slot, gen,
    /// collected)` — `collected` is false when the signal is world-root
    /// owned.
    fn signal_create(world: Option<WorldId>, data: Box<dyn AnySignal>, site: SiteLoc) -> (WorldId, u32, u32, bool);
    /// Run `f` against the slot's storage with no engine borrow held (so `f`
    /// may run user code that re-enters the kernel). Panics on a stale
    /// handle or a re-entrant access to the same slot; reports a dead world
    /// to the caller.
    fn signal_access<R>(
        world: WorldId,
        slot: u32,
        gen: u32,
        f: impl FnOnce(&mut dyn AnySignal) -> R,
    ) -> Access<R>;
    /// A read: if `track`, record `(slot, gen)` as a dependency of the
    /// innermost running effect (when that effect belongs to `world` and
    /// tracking is not suspended), then run `f` against the storage — told
    /// whether the read subscribed — exactly like [`signal_access`].
    ///
    /// One operation rather than track + access because it is the
    /// framework's hottest path: resolving the world once and holding it
    /// across both steps measured 12% faster at opt-level 3 than two calls
    /// that each resolve it (no difference at the shipping "z").
    ///
    /// [`signal_access`]: Engine::signal_access
    fn signal_read<R>(
        world: WorldId,
        slot: u32,
        gen: u32,
        track: bool,
        f: impl FnOnce(&mut dyn AnySignal, bool) -> R,
    ) -> Access<R>;
    /// A staged write (`set` / `update`): run `f` against the storage like
    /// [`signal_access`], then mark the signal staged for commit at
    /// `world`'s next flush (`force` notifies even if the value nets to
    /// unchanged). One operation for the same reason as [`signal_read`]:
    /// the world is resolved once for both steps.
    ///
    /// [`signal_access`]: Engine::signal_access
    /// [`signal_read`]: Engine::signal_read
    fn signal_write<R>(
        world: WorldId,
        slot: u32,
        gen: u32,
        force: bool,
        f: impl FnOnce(&mut dyn AnySignal) -> R,
    ) -> Access<R>;
    /// `touch`: notify subscribers at the next flush without writing.
    /// Panics on a stale handle (a recycled slot's new occupant must not be
    /// woken); a no-op on a dead world.
    fn signal_touch(world: WorldId, slot: u32, gen: u32);
    /// `Signal::is_alive`.
    fn signal_is_alive(world: WorldId, slot: u32, gen: u32) -> bool;
    /// `Signal::subscriber_count`.
    fn signal_subscriber_count(world: WorldId, slot: u32, gen: u32) -> usize;
    /// Where the signal in `slot` was created — the staged-read warning
    /// names it.
    #[cfg(debug_assertions)]
    fn signal_created_at(world: WorldId, slot: u32) -> Option<SiteLoc>;

    // ---- effects ------------------------------------------------------

    /// Allocate an effect in `world` (`None` = ambient), register it with
    /// the innermost collector, and run it once to collect its
    /// dependencies. Returns `(world, slot, gen)`.
    fn effect_create(world: Option<WorldId>, class: EffectClass, body: Box<dyn FnMut()>) -> (WorldId, u32, u32);
    /// `Effect::is_alive`.
    fn effect_is_alive(world: WorldId, slot: u32, gen: u32) -> bool;
    /// `on_cleanup`: register on the innermost RUNNING effect; panics with
    /// the canonical message outside one.
    fn on_cleanup(f: Box<dyn FnOnce()>);

    // ---- regions ------------------------------------------------------

    /// `untrack`.
    fn untrack<R>(f: impl FnOnce() -> R) -> R;
    /// `unscoped`.
    fn unscoped<R>(f: impl FnOnce() -> R) -> R;
    /// `unanchored`.
    fn unanchored<R>(f: impl FnOnce() -> R) -> R;

    // ---- ownership ----------------------------------------------------

    /// `collect_owned`: everything `f` creates is collected into the
    /// returned scope. If `f` panics, what it collected is freed first.
    fn collect<R>(f: impl FnOnce() -> R) -> (R, Self::Scope);
    /// `Owned::merge`: `other`'s items now live and die with `into`.
    fn scope_merge(into: &mut Self::Scope, other: Self::Scope);
    /// `Owned::len`.
    fn scope_len(scope: &Self::Scope) -> usize;
    /// `Owned`'s drop: retract context, free effects (cleanups first), then
    /// free signals.
    fn scope_drop(scope: &mut Self::Scope);

    // ---- context ------------------------------------------------------

    /// `provide`: push onto the ambient world's context stack for `key`,
    /// owned by the innermost collector.
    fn ctx_provide(key: TypeId, value: Box<dyn Any>);
    /// `inject`: `f` sees the newest live provision of `key` in the ambient
    /// world.
    fn ctx_inject<R>(key: TypeId, f: impl FnOnce(&dyn Any) -> R) -> Option<R>;
}
