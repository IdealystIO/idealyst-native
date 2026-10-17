//! runtime-scene — the scene core.
//!
//! The scene sits between the reactive kernel (`runtime-world`) and the
//! primitive vocabulary (`runtime-vocabulary`): it defines *structure* and
//! nothing else.
//!
//! - [`Element`] — the abstract blueprint a component returns: six
//!   variants (`Item`/`Fragment`/`Dyn`/`Keyed`/`Owned`/`Many`), of which
//!   only `Item` and `Many` ever cross the platform boundary. Payloads are
//!   type-erased; the scene never interprets them.
//! - [`Host`] — the entire structural seam a platform must implement:
//!   `insert`, `insert_many`, `insert_at`, `remove_child`,
//!   `clear_children`, `release_subtree`, `create_anchor`,
//!   `supports_splice`.
//! - [`Registry`] — TypeId-keyed handlers, one per payload type. Built-in
//!   and third-party primitives mount through the same contract: a
//!   third-party primitive is just a payload type whose handler ships
//!   outside the framework.
//! - **Late-bound payloads** ([`Registry::defer`],
//!   [`Registry::register_deferred`], [`defer_registration`]) — the
//!   bundle-size seam. An app declares a payload kind late-bound at boot;
//!   realize then PARKS items of that kind behind a placeholder instead of
//!   panicking, and the handler installs itself later — typically from
//!   inside a lazily loaded wasm chunk — at which point the parked items
//!   realize in place, in document order, with no remount. It exists so
//!   a heavy third-party
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
//!   named panic instead. See `realize::depth` for the numbers.
//! - Structural drivers (in [`realize`](mod@crate::realize)) — the
//!   Dyn (`when`/`switch`/dynamic) and Keyed (`for`) drivers, with their
//!   op-sequence invariants pinned by the `scene-parity` golden suite
//!   (see that crate's README for the contract).

mod element;
mod host;
mod id_frame;
mod late;
mod realize;
mod region_context;
mod registry;

#[cfg(test)]
mod tests;

pub use element::{
    component_scope, dyn_element, dyn_guarded, dyn_keyed, payload_type_name, fragment, item, keyed, many, owned, with_realize_hook,
    DynKind, DynSpec, Element, Key, RealizeHook, RetireHook,
};
/// The overlay's node-origin surface — only under `ui-overlay`.
#[cfg(feature = "ui-overlay")]
pub use element::{item_tagged, with_tag, NodeTag};
#[cfg(feature = "ui-overlay")]
pub use element::with_rebuild;
pub use host::Host;
pub use id_frame::{use_id, use_id_keyed};
pub use region_context::{install_region_context, RegionScope};
pub use late::{defer_registration, drain_registrations, has_pending_registrations};
pub use realize::{
    realize, DeferredLive, DynLive, DynWatch, KeyedLive, KeyedState, LiveNode, MountCx, Realized,
    Retired, MAX_DEPTH,
};
/// Every mounted node a `ui!` site built, reachable by tag.
///
/// The overlay's live path finds an instance from a `(site, node)`
/// pair. Walking down from one `Realized` cannot: a handler may realize
/// a subtree into its OWN storage rather than into the tree it was
/// called from, and the navigator handlers do — a screen's `Realized`
/// is held by the navigator. So registration happens in `mount_item`,
/// the one place every mounted node passes through, and the registry
/// holds `Weak` while the `Rc` lives in the `LiveNode`: dropping a
/// `Realized` is unmount, so a popped screen's entries go dead with no
/// cleanup hook to forget to fire.
#[cfg(feature = "ui-overlay")]
pub mod live;
#[cfg(feature = "ui-overlay")]
pub use live::{instances, live_count, root_origin, LiveOrigin};
#[cfg(feature = "ui-overlay")]
pub use realize::visit_tagged;
pub use registry::{Handler, ManyHandler, Registry};
