//! Web renderer, new-core leg — the old `web.rs` handler re-expressed
//! over the scene registry (idea-lite migration, External-SDK wave).
//!
//! Mount mechanics are the old `build_canvas` call-for-call: one
//! `<canvas>` per mount, the shared latest-`Scene` cell, the shared
//! [`make_2d_rasterizer`](crate::web::make_2d_rasterizer) (2d context +
//! texture layers + `captureStream` self-capture — all core-free and
//! literally the same functions), a `ResizeObserver` guarded for
//! teardown, and a reactive repaint effect. Only the reactive substrate
//! differs: the old `runtime_core::effect!` becomes a
//! `runtime_world::effect` (created during realize = world entered, runs
//! once immediately, collected into the enclosing subtree — dropping the
//! subtree drops the effect, whose closure owns the observer guard, so
//! the disconnect-on-unmount contract is preserved verbatim).
//!
//! No `schedule_flush` wrapping is needed here: the canvas has no author
//! callbacks — the painter runs INSIDE the repaint effect (flush
//! context), and the `ResizeObserver` callback only replays the cached
//! scene (no author code, no signal writes).

use std::cell::RefCell;
use std::rc::Rc;

use backend_web::WebBackend;
use canvas_core::{paint_scene, CanvasPrim, Scene};
use runtime_scene::{Element, MountCx, Registry};
use runtime_vocabulary::caps::ExternalOps;
use runtime_vocabulary::style_attach::{attach_style, on_teardown};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::{HtmlCanvasElement, ResizeObserver};

use crate::web::{make_2d_rasterizer, ObserverGuard};

/// Register the native (Canvas2D) renderer's scene handler for
/// [`CanvasPrim`] on the web backend's registry. Pass as (part of) the
/// boot registration seam: `backend_web::newcore::start_in("#app",
/// canvas_native::register, app)`.
pub fn register(registry: &mut Registry<WebBackend>) {
    canvas_core::ensure_wire_serde();
    registry.register::<CanvasPrim, _>(mount_canvas);
}

fn mount_canvas(
    cx: &mut MountCx<'_, WebBackend>,
    prim: &Rc<CanvasPrim>,
    _children: Vec<Element>,
) -> web_sys::Node {
    let backend = cx.backend().clone();
    let document = web_sys::window()
        .expect("no window")
        .document()
        .expect("no document");
    let el = document
        .create_element("canvas")
        .expect("create_element(canvas) failed");
    let _ = el.set_attribute("data-external-kind", "canvas_core::CanvasProps");

    let canvas: HtmlCanvasElement = el.clone().dyn_into().expect("canvas element cast");

    // Latest painted scene — written by the content effect, read by both
    // the effect's own render and the resize observer.
    let cell: Rc<RefCell<Scene>> = Rc::new(RefCell::new(Scene::new()));

    // Per-frame rasterizer (2d ctx + texture layers + captureStream) —
    // the SAME shared function the old-core handler uses.
    let rasterize = Rc::new(RefCell::new(make_2d_rasterizer(canvas, &prim.props)));

    let cb = Closure::<dyn FnMut()>::new({
        let rasterize = rasterize.clone();
        let cell = cell.clone();
        move || (rasterize.borrow_mut())(&cell.borrow())
    });
    let observer = ResizeObserver::new(cb.as_ref().unchecked_ref()).expect("ResizeObserver::new");
    observer.observe(&el);
    let guard = ObserverGuard { observer, _cb: cb };

    // Reactive repaint. Realize runs world-entered, so this effect is
    // collected into the mounting subtree — it (and the guard +
    // rasterizer it owns) live until unmount, exactly like the old
    // walker-scope-owned `effect!`.
    let props = prim.props.clone();
    runtime_world::effect(move || {
        // Capture the observer guard into the subtree-owned effect so it
        // is dropped (→ disconnected) exactly when the canvas unmounts.
        let _keep = &guard;
        *cell.borrow_mut() = paint_scene(&props);
        (rasterize.borrow_mut())(&cell.borrow());
    });

    let node: web_sys::Node = el.into();
    if let Some(style) = prim.take_style() {
        attach_style(&backend, &node, style);
    }
    // Old walker parity: every External mount installed a cleanup guard
    // calling `release_external` at scope teardown.
    let backend_for_drop = backend.clone();
    let node_for_drop = node.clone();
    on_teardown(move || {
        backend_for_drop.borrow_mut().release_external(&node_for_drop);
    });
    node
}
