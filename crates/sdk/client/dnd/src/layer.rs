//! [`drag_layer`] — renders a [`Draggable`]'s drag preview (the "ghost")
//! following the pointer, above the content.
//!
//! Mount it **once**, as (or near) the **last** child of your app root, passing
//! the same [`DragContext`] your draggables/droppables use:
//!
//! ```ignore
//! ui! {
//!     view() {
//!         // ...your screen...
//!         { dnd::drag_layer(&ctx) }   // last child → paints on top
//!     }
//! }
//! ```
//!
//! ## Why a plain `pointer-events: none` layer, not a portal/overlay
//!
//! The ghost has two jobs: render *above* the content, and **never** intercept
//! input. A fullscreen overlay/portal gets the first right but fails the second
//! — its viewport-covering root swallows the `pointermove`/`pointerup` that the
//! drag depends on, so the drag silently never ends (a "stuck" drag that looks
//! like a freeze). The fix is the standard one for a drag layer: render the
//! ghost as an `position: absolute`, `pointer-events: none` element. Mounted as
//! the last child of the root it paints over everything (sibling order, and a
//! high `z_index` — [`DRAG_LAYER_Z`] — for content that has `z_index`es of its
//! own), `position: absolute` keeps it out of
//! flow (no layout impact), and `pointer-events: none` lets every event pass
//! straight through to the dragged element. The ghost is positioned purely by a
//! bound translate (window coords) — a translate never affects layout, so it
//! can't expand any container as you drag.
//!
//! ## Clipped to the window
//!
//! The ghost sits inside a clip box pinned to all four edges of the window
//! (`top`/`right`/`bottom`/`left: 0`, `overflow: hidden`, also
//! `pointer-events: none`). A ghost picked up mid-row or carried to the edge
//! extends past the window; unclipped it widened the page and gave it a
//! scrollbar. The box is absolutely placed, so the app root needs no
//! `overflow` or positioning of its own for the clip to hold: on the web
//! with an unpositioned root it is sized to the viewport; on native the
//! root is the window. It mounts only while a preview drag is in flight.
//!
//! ## Placed before its first paint
//!
//! The ghost's translate is written as it mounts
//! ([`DragContext::place_ghost`]), not left to the bound value: a binding
//! reaches a node only on the value's next change, so the ghost painted a
//! frame at the window's top-left first.
//!
//! The source element stays where it is (dim it via [`DragContext::dragging`] /
//! [`Draggable::is_dragging`] if you want); only the ghost moves.

use std::rc::Rc;

use runtime_core::{
    fragment, ui, view, Element, IntoElement, Length, Overflow, PointerEvents, Position, Ref,
    StyleRules, StyleSheet, Tokenized, ViewHandle,
};

use crate::context::DragContext;

/// Render the drag layer for `ctx`. Shows the active draggable's preview while a
/// drag with a preview is in flight, positioned under the pointer; renders
/// nothing otherwise. Mount once, as the last child of your app root.
pub fn drag_layer<T: Clone + 'static>(ctx: &DragContext<T>) -> Element {
    let dragging = ctx.dragging();
    let ctx = ctx.clone();
    let ghost = move || {
        // Only in-flight *preview* drags get a ghost; an in-place draggable
        // (no preview) leaves this empty and moves its own element instead.
        if !ctx.has_preview() {
            return fragment(Vec::new());
        }
        let ghost_ref: Ref<ViewHandle> = Ref::new();
        // Positioned entirely by the bound translate (window coords), and
        // placed at the current position as it mounts (module docs).
        ctx.bind_ghost(ghost_ref);
        let content = ctx.build_preview();
        let place = ctx.clone();
        let ghost = view(vec![content])
            .with_style(ghost_sheet())
            .bind(ghost_ref)
            .on_mount(move |h| place.place_ghost(h))
            .into_element();
        view(vec![ghost]).with_style(clip_sheet()).into_element()
    };
    ui! {
        if dragging.get() {
            ghost()
        }
    }
}

/// The ghost's `z_index`. Sibling order alone puts the layer over content
/// that has no `z_index` of its own; an app shell that stacks its regions
/// (a pinned sidebar under the page, a drawer over a scrim) gives them
/// positive ones, and those paint over any later sibling without one.
pub const DRAG_LAYER_Z: i32 = 10_000;

/// The window clip around the ghost: pinned to all four edges, clipping
/// what overhangs them, above whatever the app stacks, and — crucially —
/// `pointer-events: none` so it never swallows the moves/release the drag
/// needs. See the module docs, "Clipped to the window".
pub(crate) fn clip_sheet() -> Rc<StyleSheet> {
    let zero = || Some(Tokenized::Literal(Length::Px(0.0)));
    Rc::new(StyleSheet::r#static(StyleRules {
        position: Some(Position::Absolute),
        top: zero(),
        right: zero(),
        bottom: zero(),
        left: zero(),
        overflow: Some(Overflow::Hidden),
        pointer_events: Some(PointerEvents::None),
        z_index: Some(DRAG_LAYER_Z),
        ..Default::default()
    }))
}

/// The ghost wrapper: out of flow at the clip's top-left (the window's), moved
/// by the bound translate to track the finger, and `pointer-events: none`
/// like its clip.
pub(crate) fn ghost_sheet() -> Rc<StyleSheet> {
    Rc::new(StyleSheet::r#static(StyleRules {
        position: Some(Position::Absolute),
        top: Some(Tokenized::Literal(Length::Px(0.0))),
        left: Some(Tokenized::Literal(Length::Px(0.0))),
        pointer_events: Some(PointerEvents::None),
        ..Default::default()
    }))
}
