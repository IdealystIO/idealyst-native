//! Graphics handler — port of `walker/graphics.rs::build`.

use runtime_scene::{Element, MountCx, Registry};

use crate::caps::{GraphicsOps, IntrospectionOps};
use crate::prims::{GraphicsPrim, PrimCell};
use crate::style_attach::{attach_style, on_teardown, StyleServices};

/// Register the `graphics` handler (called from `register_builtins`).
pub fn register_graphics<H>(registry: &mut Registry<H>)
where
    H: GraphicsOps + StyleServices + IntrospectionOps + 'static,
{
    registry.register::<PrimCell<GraphicsPrim>, _>(|cx, p, children| {
        mount_graphics(cx, p.take(), children)
    });
}

/// Mount a `graphics` surface — port of `walker/graphics.rs::build`.
///
/// Sequence: `create_graphics(on_ready, on_resize, on_lost)` (the
/// lifecycle closures move into the backend whole) → attach_style →
/// unconditional teardown probe (`release_graphics`) → ref-fill.
///
/// The release probe is registered independently of (and after) the
/// style attach, mirroring the old `GraphicsHandleCleanup` empty-Effect:
/// an UNSTYLED graphics still gets torn down, and a styled one releases
/// after `on_node_unstyled` (both cores free effects in creation
/// order). Without the release, the backend's surface state (device,
/// queue, user render callbacks) outlives the subtree and the platform
/// keeps delivering resize/lost events into freed author state — the
/// same queued-event class the virtualizer release prevents.
pub fn mount_graphics<H>(
    cx: &mut MountCx<'_, H>,
    prim: GraphicsPrim,
    _children: Vec<Element>,
) -> H::Node
where
    H: GraphicsOps + StyleServices + IntrospectionOps,
{
    let backend = cx.backend().clone();
    let node = backend
        .borrow_mut()
        .create_graphics(prim.on_ready, prim.on_resize, prim.on_lost, &prim.a11y);
    // Passive/visual primitive: findable by test_id + kind, no actions.
    #[cfg(feature = "robot")]
    let _robot = crate::robot::register_mount(
        &backend,
        &node,
        crate::robot::ElementKind::Graphics,
        prim.test_id,
        None,
        None,
        crate::robot::MountActions::default(),
    );
    if let Some(style) = prim.style {
        attach_style(&backend, &node, style);
    }
    {
        let b = backend.clone();
        let n = node.clone();
        on_teardown(move || {
            b.borrow_mut().release_graphics(&n);
        });
    }
    if let Some(fill) = prim.ref_fill {
        let handle = backend.borrow().make_graphics_handle(&node);
        fill(handle);
    }
    node
}
