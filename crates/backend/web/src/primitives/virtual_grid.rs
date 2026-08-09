//! `virtual_grid` — a two-axis scrolling `<div>` whose visible-rect
//! diff is handled by `runtime/js/virtual_grid.js`.
//!
//! Sibling of [`virtualizer`](super::virtualizer); read that module's
//! header for the closure-ownership contract (why the wasm-bindgen
//! `Closure` handles are *owned* here rather than `.forget()`-ed, and
//! why release is two-phase). Both apply verbatim.
//!
//! ## Where the window math lives
//!
//! In **Rust**, not in the shim.
//! `runtime_shared::primitives::virtual_grid::GridMetrics` owns the
//! prefix sums and the binary search; JS calls back through the
//! `window` closure to ask "what's visible at this offset?" and gets a
//! resolved rectangle plus the content size.
//!
//! That's the opposite split from the 1-D shim, which computes its own
//! ranges in JS — and it's deliberate. The 1-D range math is now
//! re-derived in the web shim, a UICollectionViewFlowLayout, an
//! NSCollectionViewFlowLayout and a RecyclerView LayoutManager; four
//! copies of one formula is how implementations drift (the sticky pin
//! math was the same story before `runtime_shared::sticky`). Keeping
//! the arithmetic on the Rust side means every backend's grid engine
//! windows identically by construction, and the per-scroll cost is one
//! wasm call returning six numbers — cheaper than the per-cell
//! crossings the mount path already pays.

use std::cell::RefCell;
use std::rc::Rc;

use crate::WebBackend;
use runtime_shared::primitives::virtual_grid::{GridCallbacks, GridMetrics};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::Node;

/// The size/count closures needed to rebuild metrics on a data change.
/// Held separately from the `GridCallbacks` bundle (whose mount/release
/// halves move into JS closures) so `data_changed` can re-query without
/// keeping the whole bundle alive twice.
struct Sizing {
    col_count: Rc<dyn Fn() -> usize>,
    row_count: Rc<dyn Fn() -> usize>,
    col_width: Rc<dyn Fn(usize) -> f32>,
    row_height: Rc<dyn Fn(usize) -> f32>,
}

impl Sizing {
    fn build_metrics(&self) -> GridMetrics {
        GridMetrics::build(
            (self.col_count)(),
            (self.row_count)(),
            &*self.col_width,
            &*self.row_height,
        )
    }
}

pub(crate) struct VirtualGridInstance {
    pub(crate) js: JsValue,
    /// Shared with the `window` closure JS calls per scroll, so
    /// `data_changed` can swap in fresh metrics and have the next
    /// window query see them.
    metrics: Rc<RefCell<GridMetrics>>,
    sizing: Rc<Sizing>,
    _closures: Vec<Box<dyn std::any::Any>>,
}

/// Property under which `create` parks the JS instance on its own
/// container, so `VirtualGridHandle` can reach it without borrowing
/// the backend. Same rationale as the virtualizer's.
const JS_INSTANCE_PROP: &str = "_idealystVirtualGridInstance";

pub(crate) fn create(
    b: &mut WebBackend,
    callbacks: GridCallbacks<Node>,
    overscan: f32,
) -> Node {
    b.ensure_virtual_grid_shim();

    let container = b
        .doc
        .create_element("div")
        .expect("create_element div failed");
    let grid_id = b.next_virtual_grid_id;
    b.next_virtual_grid_id += 1;
    let _ = container.set_attribute("data-virtual-grid-id", &grid_id.to_string());
    let container_node: Node = container.clone().unchecked_into();

    let sizing = Rc::new(Sizing {
        col_count: callbacks.col_count.clone(),
        row_count: callbacks.row_count.clone(),
        col_width: callbacks.col_width.clone(),
        row_height: callbacks.row_height.clone(),
    });
    let metrics = Rc::new(RefCell::new(sizing.build_metrics()));

    // `window(scrollX, scrollY, vpW, vpH)` → the resolved rectangle +
    // content size. This is the one call JS makes per scroll event.
    let window_cb = {
        let metrics = metrics.clone();
        Closure::<dyn FnMut(JsValue, JsValue, JsValue, JsValue) -> JsValue>::new(
            move |sx: JsValue, sy: JsValue, vw: JsValue, vh: JsValue| {
                let f = |v: JsValue| v.as_f64().unwrap_or(0.0) as f32;
                let m = metrics.borrow();
                let w = m.visible_window((f(sx), f(sy)), (f(vw), f(vh)), overscan);
                let (cw, ch) = m.content_size();
                let obj = js_sys::Object::new();
                let set = |k: &str, v: f64| {
                    let _ = js_sys::Reflect::set(&obj, &JsValue::from_str(k), &JsValue::from_f64(v));
                };
                // An empty window arrives as `colStart > colEnd`
                // (`GridWindow::EMPTY` is `1..=0`), which the shim's
                // `for` loops correctly treat as "mount nothing" —
                // where `0..=0` would mount a phantom cell.
                set("colStart", w.col_start as f64);
                set("colEnd", w.col_end as f64);
                set("rowStart", w.row_start as f64);
                set("rowEnd", w.row_end as f64);
                set("contentW", cw as f64);
                set("contentH", ch as f64);
                obj.into()
            },
        )
    };

    let cell_key_cb = {
        let f = callbacks.cell_key.clone();
        Closure::<dyn FnMut(JsValue, JsValue) -> JsValue>::new(
            move |c: JsValue, r: JsValue| {
                let c = c.as_f64().unwrap_or(0.0) as usize;
                let r = r.as_f64().unwrap_or(0.0) as usize;
                JsValue::from_f64(f(c, r) as f64)
            },
        )
    };

    // `mountCell(col, row)` → `[node, scopeId, x, y, w, h]`. Geometry
    // rides along with the node so the shim never has to ask for it
    // separately — one crossing per mounted cell, not two.
    let mount_cell_cb = {
        let f = callbacks.mount_cell.clone();
        let metrics = metrics.clone();
        Closure::<dyn FnMut(JsValue, JsValue) -> JsValue>::new(
            move |c: JsValue, r: JsValue| {
                let col = c.as_f64().unwrap_or(0.0) as usize;
                let row = r.as_f64().unwrap_or(0.0) as usize;
                let (node, scope_id) = f(col, row);
                let (x, y, w, h) = {
                    let m = metrics.borrow();
                    let (x, y) = m.cell_origin(col, row);
                    let (w, h) = m.cell_size(col, row);
                    (x, y, w, h)
                };
                let arr = js_sys::Array::new_with_length(6);
                arr.set(0, node.into());
                arr.set(1, JsValue::from_f64(scope_id as f64));
                arr.set(2, JsValue::from_f64(x as f64));
                arr.set(3, JsValue::from_f64(y as f64));
                arr.set(4, JsValue::from_f64(w as f64));
                arr.set(5, JsValue::from_f64(h as f64));
                arr.into()
            },
        )
    };

    // `cellOrigin(col, row)` → `[x, y]`. Exists so the handle's
    // `scroll_to_cell` can resolve a target offset from the live
    // metrics WITHOUT reaching into the backend (see the ops impl).
    let cell_origin_cb = {
        let metrics = metrics.clone();
        Closure::<dyn FnMut(JsValue, JsValue) -> JsValue>::new(
            move |c: JsValue, r: JsValue| {
                let col = c.as_f64().unwrap_or(0.0) as usize;
                let row = r.as_f64().unwrap_or(0.0) as usize;
                let (x, y) = metrics.borrow().cell_origin(col, row);
                let arr = js_sys::Array::new_with_length(2);
                arr.set(0, JsValue::from_f64(x as f64));
                arr.set(1, JsValue::from_f64(y as f64));
                arr.into()
            },
        )
    };

    let release_cell_cb = {
        let f = callbacks.release_cell.clone();
        Closure::<dyn FnMut(JsValue)>::new(move |scope_id: JsValue| {
            f(scope_id.as_f64().unwrap_or(0.0) as u64);
        })
    };

    // Built only when the author asked for one — the shim guards on
    // `cb.onScroll` before calling, so a grid without `.on_scroll(..)`
    // never crosses the wasm boundary on scroll.
    let on_scroll_cb = callbacks.on_scroll.clone().map(|f| {
        Closure::<dyn FnMut(JsValue, JsValue)>::new(move |x: JsValue, y: JsValue| {
            f(
                x.as_f64().unwrap_or(0.0) as f32,
                y.as_f64().unwrap_or(0.0) as f32,
            );
        })
    });

    let cb_obj = js_sys::Object::new();
    let set_fn = |name: &str, v: &JsValue| {
        let _ = js_sys::Reflect::set(&cb_obj, &JsValue::from_str(name), v);
    };
    set_fn("window", window_cb.as_ref());
    set_fn("cellKey", cell_key_cb.as_ref());
    set_fn("mountCell", mount_cell_cb.as_ref());
    set_fn("cellOrigin", cell_origin_cb.as_ref());
    set_fn("releaseCell", release_cell_cb.as_ref());
    if let Some(cb) = on_scroll_cb.as_ref() {
        set_fn("onScroll", cb.as_ref());
    }

    let win = web_sys::window().expect("no window");
    let ctor_raw = js_sys::Reflect::get(&win, &JsValue::from_str("__idealystVirtualGrid"))
        .expect("Reflect::get(window, __idealystVirtualGrid)");
    if !ctor_raw.is_function() {
        web_sys::console::error_1(&JsValue::from_str(
            "[virtual_grid] window.__idealystVirtualGrid is not a function — shim never installed",
        ));
        panic!("virtual_grid shim missing");
    }
    let ctor: js_sys::Function = ctor_raw.unchecked_into();
    let args = js_sys::Array::new_with_length(2);
    args.set(0, container.clone().into());
    args.set(1, cb_obj.into());
    let instance = js_sys::Reflect::construct(&ctor, &args).expect("construct VirtualGrid");

    let mut closures: Vec<Box<dyn std::any::Any>> = vec![
        Box::new(window_cb),
        Box::new(cell_key_cb),
        Box::new(mount_cell_cb),
        Box::new(cell_origin_cb),
        Box::new(release_cell_cb),
    ];
    if let Some(cb) = on_scroll_cb {
        closures.push(Box::new(cb));
    }

    let _ = js_sys::Reflect::set(&container, &JsValue::from_str(JS_INSTANCE_PROP), &instance);

    b.virtual_grid_instances.insert(
        grid_id,
        VirtualGridInstance {
            js: instance,
            metrics,
            sizing,
            _closures: closures,
        },
    );

    container_node
}

/// Counts or sizes changed: rebuild the prefix sums, THEN tell JS to
/// re-diff. Order matters — the shim's `refresh` immediately queries
/// `window`, which reads these metrics, so rebuilding after the call
/// would diff one frame against stale geometry.
pub(crate) fn data_changed(b: &mut WebBackend, node: &Node) {
    let Some(id) = grid_id_of(node) else { return };
    let Some(instance) = b.virtual_grid_instances.get(&id) else { return };
    *instance.metrics.borrow_mut() = instance.sizing.build_metrics();
    call0(&instance.js, "dataChanged");
}

/// Two-phase teardown, identical in shape to the virtualizer's: flip
/// the JS guard synchronously so queued scroll events stop calling
/// into Rust, then microtask-defer the heavy release (which unmounts
/// cells, and those per-cell scope drops can re-enter
/// `backend.borrow_mut()` through `on_node_unstyled`).
pub(crate) fn release(b: &mut WebBackend, node: &Node) {
    let Some(id) = grid_id_of(node) else { return };
    let Some(instance) = b.virtual_grid_instances.remove(&id) else { return };

    // Drop the handle's back-reference so a `VirtualGridHandle`
    // outliving its grid can't keep the instance (and its `_closures`)
    // reachable through the container forever.
    if let Ok(el) = node.clone().dyn_into::<web_sys::Element>() {
        let _ = js_sys::Reflect::delete_property(&el, &JsValue::from_str(JS_INSTANCE_PROP));
    }

    let _ = js_sys::Reflect::set(
        &instance.js,
        &JsValue::from_str("_released"),
        &JsValue::from_bool(true),
    );

    runtime_shared::schedule_microtask(move || {
        call0(&instance.js, "release");
        drop(instance);
    });
}

fn call0(js: &JsValue, method: &str) {
    let _ = js_sys::Reflect::get(js, &JsValue::from_str(method))
        .ok()
        .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
        .map(|f| f.call0(js));
}

fn grid_id_of(node: &Node) -> Option<u32> {
    node.clone()
        .dyn_into::<web_sys::Element>()
        .ok()
        .and_then(|el| el.get_attribute("data-virtual-grid-id"))
        .and_then(|s| s.parse::<u32>().ok())
}

// =========================================================================
// Imperative handle
// =========================================================================

pub(crate) struct WebVirtualGridOps;

impl runtime_shared::primitives::virtual_grid::VirtualGridOps for WebVirtualGridOps {
    /// Delegates to the shim's `scrollToCell`, which resolves the
    /// origin through the `cellOrigin` callback — i.e. from the LIVE
    /// metrics, since the author may have changed column widths since
    /// mount.
    ///
    /// Routing through the instance rather than reading the metrics
    /// here keeps the handle backend-free: a handle can outlive the
    /// mount, and reaching into `WebBackend` from one risks a
    /// re-entrant `borrow_mut` if it's called from inside an event
    /// callback.
    fn scroll_to_cell(&self, node: &dyn std::any::Any, col: usize, row: usize) {
        let Some(el) = node.downcast_ref::<web_sys::HtmlElement>() else {
            return;
        };
        let Ok(instance) = js_sys::Reflect::get(el, &JsValue::from_str(JS_INSTANCE_PROP)) else {
            return;
        };
        if instance.is_undefined() || instance.is_null() {
            return;
        }
        let _ = js_sys::Reflect::get(&instance, &JsValue::from_str("scrollToCell"))
            .ok()
            .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
            .map(|f| {
                f.call2(
                    &instance,
                    &JsValue::from_f64(col as f64),
                    &JsValue::from_f64(row as f64),
                )
            });
    }

    fn scroll_offset(&self, node: &dyn std::any::Any) -> (f32, f32) {
        match node.downcast_ref::<web_sys::HtmlElement>() {
            Some(el) => (el.scroll_left() as f32, el.scroll_top() as f32),
            None => (0.0, 0.0),
        }
    }

    fn scroll_to(&self, node: &dyn std::any::Any, x: f32, y: f32) {
        if let Some(el) = node.downcast_ref::<web_sys::HtmlElement>() {
            el.set_scroll_left(x as i32);
            el.set_scroll_top(y as i32);
        }
    }
}

pub(crate) static WEB_VIRTUAL_GRID_OPS: WebVirtualGridOps = WebVirtualGridOps;

pub(crate) fn make_handle(
    node: &Node,
) -> runtime_shared::primitives::virtual_grid::VirtualGridHandle {
    let el: web_sys::HtmlElement = node
        .clone()
        .dyn_into()
        .expect("virtual_grid node is not an HtmlElement");
    runtime_shared::primitives::virtual_grid::VirtualGridHandle::new(
        Rc::new(el),
        &WEB_VIRTUAL_GRID_OPS,
    )
}
