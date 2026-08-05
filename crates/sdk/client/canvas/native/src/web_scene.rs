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
use canvas_core::{paint_scene, CanvasPrim, Scene};
use runtime_scene::{Element, MountCx};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::{HtmlCanvasElement, ResizeObserver};

use crate::web::{make_2d_rasterizer, ObserverGuard};

pub(crate) fn mount_canvas(
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
    // the shared function `canvas-vello` also uses as its Canvas2D
    // fallback, so both paths produce identical output.
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
    crate::finish_mount(&backend, &node, prim);
    node
}
