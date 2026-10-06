//! Web renderer — the `WebBackend`-concrete scene handler.
//!
//! One `<canvas>` per mount, a shared latest-`Scene` cell, the
//! [`make_2d_rasterizer`](crate::web::make_2d_rasterizer) from
//! [`crate::web`] (2d context + texture layers + `captureStream`
//! self-capture), a `ResizeObserver` guarded for teardown, and a reactive
//! repaint effect created during realize (= world entered, so it runs
//! once immediately and is collected into the enclosing subtree —
//! dropping the subtree drops the effect, whose closure owns the observer
//! guard, so the disconnect-on-unmount contract holds).
//!
//! No `schedule_flush` wrapping is needed here: the canvas has no author
//! callbacks — the painter runs INSIDE the repaint effect (flush
//! context), and the `ResizeObserver` callback only replays the cached
//! scene (no author code, no signal writes).

use std::cell::RefCell;
use std::rc::Rc;

use backend_web::WebBackend;
use canvas_core::{CanvasPrim, Scene};
use runtime_scene::{Element, MountCx};
use web_glue::dom::{HtmlCanvasElement, ResizeObserver};
use web_glue::{Closure, JsCast};

use crate::web::{rasterizer_2d, ObserverGuard};

pub(crate) fn mount_canvas(
    cx: &mut MountCx<'_, WebBackend>,
    prim: &Rc<CanvasPrim>,
    _children: Vec<Element>,
) -> web_glue::dom::Node {
    let backend = cx.backend().clone();
    let document = web_glue::dom::window()
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
    // the shared function `canvas-vello` also uses as its Canvas2D
    // fallback, so both paths produce identical output.
    let rasterize = Rc::new(RefCell::new(rasterizer_2d(canvas.clone(), &prim.props)));

    // The observer learns the laid-out CSS box: report it (the painter reads
    // it as `Scene::size`, and a change re-runs the paint effect) and replay
    // the cached scene so the resized backing store isn't left blank meanwhile.
    let sizing = prim.size_reporter();
    let cb = Closure::new({
        let rasterize = rasterize.clone();
        let cell = cell.clone();
        let canvas = canvas.clone();
        move |_entries| {
            sizing.report(canvas.client_width() as f32, canvas.client_height() as f32);
            (rasterize.borrow_mut())(&cell.borrow())
        }
    });
    let observer = ResizeObserver::new(cb.as_js().unchecked_ref()).expect("ResizeObserver::new");
    observer.observe(&el);
    let guard = ObserverGuard { observer, _cb: cb };

    // Reactive repaint. Realize runs world-entered, so this effect is
    // collected into the mounting subtree — it (and the guard +
    // rasterizer it owns) live until unmount, exactly like the old
    // walker-scope-owned `effect!`.
    let paint_prim = prim.clone();
    runtime_world::effect(move || {
        // Capture the observer guard into the subtree-owned effect so it
        // is dropped (→ disconnected) exactly when the canvas unmounts.
        let _keep = &guard;
        *cell.borrow_mut() = paint_prim.paint();
        (rasterize.borrow_mut())(&cell.borrow());
    });

    let node: web_glue::dom::Node = el.into();
    crate::finish_mount(&backend, &node, prim);
    node
}
