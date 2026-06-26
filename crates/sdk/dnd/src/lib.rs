//! # `dnd` — cross-platform drag and drop
//!
//! In-app drag and drop — reorderable lists, kanban boards, drag-into-trash,
//! sortable grids — that behaves identically on every backend with no
//! per-platform code. Three small handles compose it:
//!
//! - [`DragContext<T>`] — the shared registry every draggable and droppable in
//!   a scope reads. It holds the live drag session and the set of drop
//!   targets. Clone it into each participant. `T` is your payload type.
//! - [`Draggable<T>`] — a drag source. It carries a payload, follows the
//!   finger via a bound offset, and reports the outcome on release.
//! - [`Droppable<T>`] — a drop target. It exposes a reactive
//!   [`Droppable::is_over`] flag and receives the payload via
//!   [`Droppable::on_drop`].
//!
//! ## Why it needs no per-platform code
//!
//! The framework converges pointer input below this crate: web Pointer Events
//! fold mouse + touch + pen into one stream, and the native backends deliver
//! their touches through the same [`runtime_core::TouchEvent`], each carrying
//! a `window_position`. A drag is therefore just a gesture
//! ([`DragRecognizer`], built on the same `Recognizer` FSM as `pan`), the
//! dragged element follows the finger through the `AnimatedValue → Translate`
//! path every backend implements, and drop targets are hit-tested in window
//! space via [`runtime_core::ViewHandle::absolute_frame`] — the same geometry
//! read overlay anchoring and scroll-spy already use. There is nothing to
//! branch on, so this is one pure-Rust crate.
//!
//! ## Sketch
//!
//! ```ignore
//! use dnd::{DragContext, Draggable, Droppable};
//! use runtime_core::{Ref, ViewHandle};
//!
//! #[component]
//! fn board() -> Element {
//!     // One context for the whole board; payload is the card id.
//!     let ctx: DragContext<u64> = DragContext::new();
//!
//!     // A card you can pick up:
//!     let card_ref: Ref<ViewHandle> = Ref::new();
//!     let card = Draggable::new(&ctx, move || 42u64)
//!         .on_release(|outcome| { /* Landed / Missed / Cancelled */ });
//!     card.bind(card_ref);
//!
//!     // A column that accepts cards:
//!     let col_ref: Ref<ViewHandle> = Ref::new();
//!     let column = Droppable::new(&ctx)
//!         .on_drop(|id| move_card_to_column(id));
//!     column.bind(col_ref);
//!     let over = column.is_over();
//!
//!     ui! {
//!         view() {
//!             // highlight the column while a card hovers it
//!             view() { /* column, styled on `over.get()` */ }
//!                 .bind(col_ref)
//!             view() { /* the card */ }
//!                 .on_touch(card.handler())
//!                 .bind(card_ref)
//!         }
//!     }
//! }
//! ```
//!
//! ## What it deliberately does not do
//!
//! Auto-scrolling a list while dragging near its edge, reorder animations, and
//! multi-select drag are **policy** — build them on the lifecycle hooks and
//! the reactive [`DragContext::dragging`] / [`Droppable::is_over`] signals,
//! the same way `pan` leaves momentum to the caller.
//!
//! ## Native per-platform drag (the seam)
//!
//! *Cross-application* drag and the browser's native HTML5 drag/`DataTransfer`
//! are a separate, additive capability — see [`native`]. This crate is the
//! universal in-app engine; the native layer is a documented follow-on.

mod context;
mod draggable;
mod droppable;
mod layer;
mod recognizer;

pub mod native;

#[doc(hidden)]
mod recipes;

pub use context::{DragContext, DroppableId};
pub use draggable::{Draggable, DropOutcome, SNAP_BACK_DAMPING, SNAP_BACK_STIFFNESS};
pub use droppable::Droppable;
pub use layer::drag_layer;
pub use recognizer::{
    Activation, DragPhase, DragRecognizer, DragSample, ScrollAxis, DEFAULT_DRAG_LONG_PRESS_MS,
    DEFAULT_DRAG_LONG_PRESS_SLOP_PX, DEFAULT_DRAG_SLOP_PX,
};

#[cfg(test)]
mod tests;
