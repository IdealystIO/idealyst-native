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
//! the last child of the root it paints over everything (paint order = sibling
//! order, no stacking-context tricks), `position: absolute` keeps it out of
//! flow (no layout impact), and `pointer-events: none` lets every event pass
//! straight through to the dragged element. The ghost is positioned purely by a
//! bound translate (window coords) — a translate never affects layout, so it
//! can't expand any container as you drag.
//!
//! The source element stays where it is (dim it via [`DragContext::dragging`] /
//! [`Draggable::is_dragging`] if you want); only the ghost moves.

use std::rc::Rc;

use runtime_core::{
    fragment, view, when, Element, IntoElement, Length, PointerEvents, Position, Ref, StyleRules,
    StyleSheet, Tokenized, ViewHandle,
};

use crate::context::DragContext;

/// Render the drag layer for `ctx`. Shows the active draggable's preview while a
/// drag with a preview is in flight, positioned under the pointer; renders
/// nothing otherwise. Mount once, as the last child of your app root.
pub fn drag_layer<T: Clone + 'static>(ctx: &DragContext<T>) -> Element {
    let dragging = ctx.dragging();
    let ctx = ctx.clone();
    when(
        move || dragging.get(),
        move || {
            // Only in-flight *preview* drags get a ghost; an in-place draggable
            // (no preview) leaves this empty and moves its own element instead.
            if !ctx.has_preview() {
                return fragment(Vec::new());
            }
            let ghost_ref: Ref<ViewHandle> = Ref::new();
            // Positioned entirely by the bound translate (window coords); see
            // `set_preview` for the one-frame-flash handling (microtask re-apply).
            ctx.bind_ghost(ghost_ref);
            let content = ctx.build_preview();
            view(vec![content])
                .with_style(ghost_sheet())
                .bind(ghost_ref)
                .into_element()
        },
        || fragment(Vec::new()),
    )
}

/// The ghost wrapper: out of flow at the root's top-left, moved by the bound
/// translate to track the finger, and — crucially — `pointer-events: none` so
/// it never swallows the moves/release the drag needs.
fn ghost_sheet() -> Rc<StyleSheet> {
    Rc::new(StyleSheet::r#static(StyleRules {
        position: Some(Position::Absolute),
        top: Some(Tokenized::Literal(Length::Px(0.0))),
        left: Some(Tokenized::Literal(Length::Px(0.0))),
        pointer_events: Some(PointerEvents::None),
        ..Default::default()
    }))
}
