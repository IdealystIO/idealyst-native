//! runtime-scene — the scene core of the idea-lite migration (P1).
//!
//! The scene sits between the reactive kernel (`runtime-world`) and the
//! primitive vocabulary (P2): it defines *structure* and nothing else.
//!
//! - [`Element`] — the abstract blueprint a component returns: five
//!   variants (`Item`/`Fragment`/`Dyn`/`Keyed`/`Owned`), of which only
//!   `Item` ever crosses the platform boundary. Payloads are type-erased;
//!   the scene never interprets them.
//! - [`Host`] — the entire structural seam a platform must implement:
//!   7 methods (`insert`, `insert_many`, `insert_at`, `remove_child`,
//!   `clear_children`, `create_anchor`, `supports_splice`), extracted
//!   verbatim from the old `Backend` trait's structural subset.
//! - [`Registry`] — TypeId-keyed handlers, one per payload type. Built-in
//!   and third-party primitives mount through the same contract; the old
//!   `Element::External` concept dissolves into "a payload type whose
//!   handler ships outside the framework".
//! - [`realize`] / [`Realized`] / [`LiveNode`] — walk an `Element` tree
//!   once, delegate every `Item` to its handler, spawn one driver effect
//!   per structural hole. Dropping the `Realized` IS unmount: every
//!   effect/signal created during realization is collected into its
//!   [`Owned`](runtime_world::Owned) and freed together.
//! - Structural drivers (in [`realize`](mod@crate::realize)) — direct
//!   ports of the old walker's `when_switch.rs`/`dynamic.rs`/`each.rs`
//!   with their op-sequence invariants pinned by the `scene-parity`
//!   golden suite (see that crate's README for the contract).

mod element;
mod host;
mod realize;
mod registry;

#[cfg(test)]
mod tests;

pub use element::{
    component_scope, dyn_element, dyn_keyed, fragment, item, keyed, owned, DynSpec, Element, Key,
    RetireHook,
};
pub use host::Host;
pub use realize::{realize, DynLive, KeyedLive, KeyedState, LiveNode, MountCx, Realized};
pub use registry::{Handler, Registry};
