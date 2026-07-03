//! Native, per-platform drag systems — the planned seam (not yet implemented).
//!
//! The handles in this crate ([`DragContext`](crate::DragContext),
//! [`Draggable`](crate::Draggable), [`Droppable`](crate::Droppable)) are
//! driven today by a single universal engine: the touch-layer
//! [`DragRecognizer`](crate::DragRecognizer) plus window-space hit-testing.
//! That engine is correct on every backend and is all an *in-app* drag needs.
//!
//! Some platforms expose a **native drag system** that the universal engine
//! deliberately does not use, because each renders through its own OS chrome
//! and only some of them can cross the application boundary:
//!
//! | Platform | Native system | Buys you |
//! |----------|---------------|----------|
//! | Web      | HTML5 `dragstart`/`drop` + `DataTransfer` | OS drag image, drop onto/from other apps and the desktop, file drag-in |
//! | iOS/iPadOS | `UIDragInteraction` / `UIDropInteraction` + `NSItemProvider` | Spring-loading, multi-app drag on iPad, system drag preview |
//! | Android  | `View.startDragAndDrop` + `ClipData` / `OnDragListener` | System drag shadow, cross-app drag on large screens |
//! | macOS    | `NSDraggingSource` / `NSDraggingDestination` + `NSPasteboard` | Drag to/from Finder and other apps, file promises |
//!
//! ## Why this is a separate phase, not a hack in the engine
//!
//! Wiring these in is **not** pure-SDK work and **not** free:
//!
//! 1. It requires new methods on the `Backend` trait — begin a native drag
//!    session, register a node as a native drop target with its accepted
//!    types, read/write the platform pasteboard — implemented across all
//!    backends. That is framework-core surface, not something this crate can
//!    reach on its own (it depends only on `runtime-core`).
//! 2. The output is, by design, **not** identical across platforms: a native
//!    drag shows the OS's drag image and follows OS conventions. That is the
//!    opposite of the "converge in output" rule the in-app engine follows, and
//!    it is exactly what you want when the goal is to interoperate with *other*
//!    apps rather than to look the same everywhere.
//!
//! So per project rule §3 (peripheral capability) and §7 (no per-platform
//! divergence in the universal path), the native systems live behind an
//! opt-in [`DragStrategy`] rather than leaking platform branches into
//! [`Draggable`]/[`Droppable`].
//!
//! ## The intended shape
//!
//! A future [`DragStrategy`] trait abstracts *who drives the drag*, so author
//! call sites never change:
//!
//! ```ignore
//! pub trait DragStrategy<T> {
//!     /// Install the source behavior on `node` (touch handler for the
//!     /// universal engine; `UIDragInteraction` / `draggable=true` /
//!     /// `startDragAndDrop` for a native driver).
//!     fn attach_source(&self, node: Ref<ViewHandle>, src: &Draggable<T>);
//!     /// Register `node` as a drop target (window-rect hit-test for the
//!     /// universal engine; native drop-interaction for a native driver).
//!     fn attach_target(&self, node: Ref<ViewHandle>, tgt: &Droppable<T>);
//! }
//! ```
//!
//! - `TouchStrategy` (this crate's current behavior) is the cross-platform
//!   default and the only one that needs no `Backend` changes.
//! - `NativeStrategy` (per backend) would be selected explicitly —
//!   `Draggable::new(&ctx, ..).strategy(native())` — when the app wants OS
//!   drag chrome or cross-app transfer, and would require the payload `T` to
//!   be serializable to a platform pasteboard type (`NSItemProvider`,
//!   `DataTransfer`, `ClipData`).
//!
//! ## Where the `Backend` work lands when this phase happens
//!
//! - `Backend::begin_native_drag(node, payload_types, drag_image)`
//! - `Backend::register_native_drop_target(node, accepted_types, handler)`
//! - `Backend::pasteboard_read` / `Backend::pasteboard_write`
//!
//! with implementations:
//!   web → `dragstart`/`dragover`/`drop` + `DataTransfer`;
//!   iOS → `UIDragInteraction`/`UIDropInteraction`;
//!   Android → `startDragAndDrop` + `OnDragListener`;
//!   macOS → `NSDraggingSource`/`registerForDraggedTypes`.
//!
//! Until then this module is intentionally empty of code — it documents the
//! contract so the seam is visible at the point a native driver is added,
//! rather than discovered later.
