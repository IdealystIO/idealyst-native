//! Compile-checked usage **recipes** for the drag-and-drop SDK.
//!
//! Each `recipe!(Target, fn ...)` is a real, type-checked example of how to use
//! the SDK. Because the fn compiles against the live API, a signature change
//! that isn't reflected here is a compile error (whenever the catalog is
//! built), so these examples can't silently rot — and the MCP / docs surface
//! them as trustworthy "how do I use this?" context.
//!
//! `recipe!` self-gates on the `catalog` feature: with it off (every production
//! build) these expand to nothing — the recipes, and the imports inside them,
//! don't compile at all. So there's no `#[cfg]` here and no cost in shipped
//! apps. Recipes are self-contained (imports live inside each fn) so the
//! captured source reads as a complete, copy-pasteable example.

use runtime_core::recipe;

recipe!(
    DragContext,
    /// The shared registry for one drag scope — the provider every draggable
    /// and droppable reads. Create it once, clone it into each participant, and
    /// mount [`drag_layer`] once so the dragged preview renders above all
    /// content. `dragging()` is a reactive flag for the whole scope.
    pub fn drag_context_scope() -> ::runtime_core::Element {
        use crate::{drag_layer, DragContext, Draggable, Droppable};
        use ::runtime_core::{text, view, IntoElement};

        // One context per scope; payload here is a card id.
        let ctx: DragContext<u64> = DragContext::new();

        let source = Draggable::new(&ctx, || 1u64)
            .preview(|| view(vec![text("•").into()]).into_element())
            .attach(view(vec![text("drag me").into()]));

        let target = Droppable::new(&ctx)
            .on_drop(|_id| { /* move the dropped item here */ })
            .attach(view(vec![text("drop here").into()]));

        // Mount the drag layer ONCE so the ghost renders above everything.
        view(vec![source, target, drag_layer(&ctx)]).into_element()
    }
);

recipe!(
    Draggable,
    /// A drag source. It carries a typed payload, shows a ghost preview in the
    /// drag layer while dragging, and dims the parked source so the two copies
    /// read as "original parked, ghost live". `attach` installs the touch
    /// handler and binds the element in one call; read `is_dragging()` if you
    /// want the source to restyle itself instead.
    pub fn draggable_source() -> ::runtime_core::Element {
        use crate::{Activation, DragContext, Draggable};
        use ::runtime_core::{text, view, IntoElement};

        let ctx: DragContext<u64> = DragContext::new();

        Draggable::new(&ctx, || 42u64)
            // Long-press to pick up on touch, immediate on desktop.
            .activation(Activation::platform_default())
            // The ghost that follows the pointer (rendered by `drag_layer`).
            .preview(|| view(vec![text("Dragging…").into()]).into_element())
            // Fade the parked source while its ghost flies.
            .dim_source(0.4)
            .attach(view(vec![text("Drag me").into()]))
    }
);

recipe!(
    Droppable,
    /// A drop target. `accepts` filters which payloads it takes, `is_over` is a
    /// reactive flag you read to highlight the zone while a compatible payload
    /// hovers, and `on_drop` receives the payload. `attach` registers + binds
    /// the zone in one call.
    pub fn droppable_target() -> ::runtime_core::Element {
        use crate::{DragContext, Droppable};
        use ::runtime_core::{text, view};

        let ctx: DragContext<u64> = DragContext::new();

        let zone = Droppable::new(&ctx)
            .accepts(|id| *id > 0)
            .on_enter(|_id| { /* e.g. animate the zone's highlight on */ })
            .on_leave(|| { /* …and off */ })
            .on_drop(|_id| { /* accept the dropped item */ });

        // Read `is_over` in your view (reactive style / animated bg) to highlight.
        let _hovering = zone.is_over();

        zone.attach(view(vec![text("Drop zone").into()]))
    }
);

recipe!(
    drag_layer,
    /// Mount this ONCE near the app root, sharing the same `DragContext` as your
    /// draggables. It renders each in-flight drag's preview ("ghost") in a
    /// top-level overlay — above all content and never clipped by an ancestor,
    /// the cross-platform way to elevate the dragged element.
    pub fn mount_drag_layer() -> ::runtime_core::Element {
        use crate::{drag_layer, DragContext};

        let ctx: DragContext<u64> = DragContext::new();
        drag_layer(&ctx)
    }
);
