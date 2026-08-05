//! runtime-scene — the scene core of the idea-lite migration (P1).
//!
//! The scene sits between the reactive kernel (`runtime-world`) and the
//! primitive vocabulary (P2): it defines *structure* and nothing else.
//!
//! - [`Element`] — the abstract blueprint a component returns: six
//!   variants (`Item`/`Fragment`/`Dyn`/`Keyed`/`Owned`/`Many`), of which
//!   only `Item` and `Many` ever cross the platform boundary. Payloads are
//!   type-erased; the scene never interprets them.
//! - [`Host`] — the entire structural seam a platform must implement:
//!   7 methods (`insert`, `insert_many`, `insert_at`, `remove_child`,
//!   `clear_children`, `create_anchor`, `supports_splice`), extracted
//!   verbatim from the old `Backend` trait's structural subset.
//! - [`Registry`] — TypeId-keyed handlers, one per payload type. Built-in
//!   and third-party primitives mount through the same contract; the old
//!   `Element::External` concept dissolves into "a payload type whose
//!   handler ships outside the framework".
//! - **Late-bound payloads** ([`Registry::defer`],
//!   [`Registry::register_deferred`], [`defer_registration`]) — the
//!   bundle-size seam. An app declares a payload kind late-bound at boot;
//!   realize then PARKS items of that kind behind a placeholder instead of
//!   panicking, and the handler installs itself later — typically from
//!   inside a lazily loaded wasm chunk — at which point the parked items
//!   realize in place, in document order, with no remount. This is the
//!   scene-model successor to the old core's
//!   `defer_external_registration`; it exists so a heavy third-party
//!   handler's code can live in a chunk instead of being anchored in
//!   `main.wasm` by its own registration
//!   (measured by `tests/lazy-payload-split`).
//! - [`realize`] / [`Realized`] / [`LiveNode`] — walk an `Element` tree
//!   once, delegate every `Item` to its handler, spawn one driver effect
//!   per structural hole. Dropping the `Realized` IS unmount: every
//!   effect/signal created during realization is collected into its
//!   [`Owned`](runtime_world::Owned) and freed together.
//! - [`MAX_DEPTH`] — the realize recursion budget. The walk descends by
//!   recursion, so a pathologically deep tree (an unbounded-recursion
//!   component body) would exhaust the stack; past the cap it raises a
//!   named panic instead. See `realize::depth` for the numbers and for
//!   what the old walker did (a frame-size budget, and no cap at all).
//! - Structural drivers (in [`realize`](mod@crate::realize)) — direct
//!   ports of the old walker's `when_switch.rs`/`dynamic.rs`/`each.rs`
//!   with their op-sequence invariants pinned by the `scene-parity`
//!   golden suite (see that crate's README for the contract).

mod element;
mod host;
mod late;
mod realize;
mod registry;

#[cfg(test)]
mod tests;

pub use element::{
    component_scope, dyn_element, dyn_keyed, fragment, item, keyed, many, owned, DynSpec, Element,
    Key, RetireHook,
};
pub use host::Host;
pub use late::{defer_registration, drain_registrations, has_pending_registrations};
pub use realize::{
    realize, DeferredLive, DynLive, DynWatch, KeyedLive, KeyedState, LiveNode, MountCx, Realized,
    Retired, MAX_DEPTH,
};
pub use registry::{Handler, ManyHandler, Registry};
