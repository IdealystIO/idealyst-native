//! `Primitive::Graphics` — a `<canvas>` element exposed as a
//! `raw_window_handle` surface provider.
//!
//! No GPU library lives in the backend now. The previous design
//! pulled in `wgpu` directly; that coupling is gone — the backend
//! just creates the canvas, fires the lifecycle callbacks, and lets
//! the author plug in whatever rendering library they want.
//!
//! # Lifecycle
//!
//! 1. `create` builds the `<canvas>` and stashes per-instance state
//!    keyed by a `data-graphics-id` attribute on the element. Returns
//!    the canvas as the layout node.
//! 2. One `requestAnimationFrame` later (so the canvas has been
//!    inserted into the DOM and laid out), `fire_ready` reads the
//!    canvas's size, sizes the drawable buffer to match the CSS box
//!    × `devicePixelRatio`, and invokes `on_ready` with a
//!    `GraphicsSurface` wrapping a `CanvasSurfaceProvider`.
//! 3. A `ResizeObserver` calls `fire_resize` on every box change,
//!    which re-sizes the buffer and invokes `on_resize`.
//! 4. A `webglcontextlost` listener fires `on_lost`. (Web doesn't
//!    have a true "surface destroyed" concept; context-lost is the
//!    closest analogue.)
//! 5. `release` (called from `Backend::release_graphics` when the
//!    parent scope drops) disconnects the observer, drops the
//!    closures, and removes the per-canvas instance entry.

use crate::WebBackend;
use framework_core::primitives::graphics::{
    GraphicsHandle, GraphicsOps, GraphicsSurface, OnLost, OnReady, OnReadyEvent, OnResize,
    OnResizeEvent, SurfaceProvider,
};
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle,
    RawWindowHandle, WebCanvasWindowHandle, WebDisplayHandle, WindowHandle,
};
use std::any::Any;
use std::cell::RefCell;
use std::ptr::NonNull;
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use web_sys::Node;

// ---------------------------------------------------------------------------
// SurfaceProvider impl — wraps a canvas and produces raw-window-handle
// ---------------------------------------------------------------------------

/// Surface provider for a `<canvas>` element. Holds the canvas alive
/// for as long as the user keeps the `GraphicsSurface` (via `Rc`)
/// and produces fresh `WebCanvasWindowHandle` / `WebDisplayHandle`
/// values on demand. wgpu and friends call `window_handle()` /
/// `display_handle()` once during surface creation; we don't need
/// to cache the values.
pub(crate) struct CanvasSurfaceProvider {
    canvas: web_sys::HtmlCanvasElement,
}

impl HasWindowHandle for CanvasSurfaceProvider {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let value: &JsValue = self.canvas.as_ref();
        let obj = NonNull::from(value).cast();
        let raw = RawWindowHandle::WebCanvas(WebCanvasWindowHandle::new(obj));
        // SAFETY: `self.canvas` is held alive for the duration of
        // `&self`, and `WindowHandle<'_>` is a borrow from `&self`,
        // so the obj pointer can't outlive the canvas.
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

impl HasDisplayHandle for CanvasSurfaceProvider {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        let raw = RawDisplayHandle::Web(WebDisplayHandle::new());
        // SAFETY: WebDisplayHandle has no fields and no real
        // borrow — it's a marker. `borrow_raw` requires `unsafe`
        // for the general case.
        Ok(unsafe { DisplayHandle::borrow_raw(raw) })
    }
}

// ---------------------------------------------------------------------------
// GraphicsInstance — per-canvas runtime state
// ---------------------------------------------------------------------------

/// Per-canvas runtime state. Lives behind `Rc<RefCell<>>` so the
/// init-rAF, the ResizeObserver callback, and the context-lost
/// listener can all reach it.
pub(crate) struct GraphicsInstance {
    /// The provider is held in an Rc so the user's `GraphicsSurface`
    /// keeps the canvas alive past unmount if they want to.
    provider: Rc<CanvasSurfaceProvider>,
    /// User callbacks. `on_ready` is `FnMut` because Android can fire
    /// it more than once (we use the same trait shape on web for
    /// API uniformity even though context-lost / context-restored on
    /// web is best-effort).
    on_ready: OnReady,
    on_resize: OnResize,
    on_lost: OnLost,
    /// Latest known drawable size in physical pixels. Used to
    /// suppress duplicate `on_resize` calls when the ResizeObserver
    /// fires for an unchanged box.
    size: (u32, u32),
    /// Set to `true` once `release` has been called. Guards every
    /// callback against post-teardown firing.
    released: bool,
    /// Has `on_ready` fired yet? Until it does, ResizeObserver
    /// callbacks are dropped (they'd race the initial fire).
    ready_fired: bool,
    /// Owned closures whose lifetime must match the instance.
    /// Dropped on `release` so DOM listeners stop firing.
    resize_observer: Option<web_sys::ResizeObserver>,
    resize_closure: Option<Closure<dyn FnMut(JsValue, JsValue)>>,
    context_lost_closure: Option<Closure<dyn FnMut(web_sys::Event)>>,
}

// ---------------------------------------------------------------------------
// create
// ---------------------------------------------------------------------------

pub(crate) fn create(
    b: &mut WebBackend,
    on_ready: OnReady,
    on_resize: OnResize,
    on_lost: OnLost,
) -> Node {
    let canvas: web_sys::HtmlCanvasElement = b
        .doc
        .create_element("canvas")
        .expect("create_element canvas failed")
        .dyn_into()
        .expect("created element is not a canvas");

    // Default sizing: fill the parent. Authors can override via
    // .with_style(...) — these are just so an unstyled Graphics
    // shows up at all.
    let _ = canvas.set_attribute("style", "display: block; width: 100%; height: 100%");

    // Mint a stable id and write it on the canvas as a data
    // attribute so `make_handle` and `release` can find this
    // instance from a fresh `&Node` later. (Can't use `node_id` —
    // see graphics.rs history; that's keyed by Rust pointer
    // identity which doesn't survive return-by-value.)
    let id = b.next_graphics_id;
    b.next_graphics_id += 1;
    let _ = canvas.set_attribute("data-graphics-id", &id.to_string());

    let provider = Rc::new(CanvasSurfaceProvider { canvas: canvas.clone() });

    let instance = Rc::new(RefCell::new(GraphicsInstance {
        provider,
        on_ready,
        on_resize,
        on_lost,
        size: (0, 0),
        released: false,
        ready_fired: false,
        resize_observer: None,
        resize_closure: None,
        context_lost_closure: None,
    }));
    b.graphics_instances.insert(id, instance.clone());

    let node: Node = canvas.unchecked_into();

    // Defer the initial `on_ready` by one rAF so the canvas is
    // laid out and its CSS box has a real size to read.
    let inst_for_init = instance.clone();
    let init_raf = Closure::<dyn FnMut()>::new(move || {
        fire_ready(&inst_for_init);
    });
    let window = web_sys::window().expect("no window");
    let _ = window.request_animation_frame(init_raf.as_ref().unchecked_ref());
    init_raf.forget();

    // Install ResizeObserver. Callback fires `fire_resize`, which
    // both updates the drawable buffer and invokes the user's
    // `on_resize` closure.
    install_resize_observer(instance.clone());

    // Install context-lost listener. Fires `on_lost`.
    install_context_lost_listener(instance.clone());

    node
}

// ---------------------------------------------------------------------------
// fire_ready — run after first rAF, sizes canvas + invokes on_ready
// ---------------------------------------------------------------------------

fn fire_ready(instance: &Rc<RefCell<GraphicsInstance>>) {
    if instance.borrow().released {
        return;
    }

    let (canvas, size) = {
        let inst = instance.borrow();
        let dpr = web_sys::window()
            .map(|w| w.device_pixel_ratio())
            .unwrap_or(1.0);
        let cw = inst.provider.canvas.client_width();
        let ch = inst.provider.canvas.client_height();
        // Fallback: if the canvas hasn't been laid out yet (e.g.
        // mounted under a non-flex parent without explicit size),
        // use the HTML default 300×150 so the user gets *something*.
        let w = if cw > 0 { cw } else { 300 };
        let h = if ch > 0 { ch } else { 150 };
        let pw = ((w as f64) * dpr).round() as u32;
        let ph = ((h as f64) * dpr).round() as u32;
        (inst.provider.canvas.clone(), (pw.max(1), ph.max(1)))
    };
    canvas.set_width(size.0);
    canvas.set_height(size.1);

    // Update tracked size, mark ready, take an Rc clone of the
    // provider for the user's event before invoking the callback.
    let surface = {
        let mut inst = instance.borrow_mut();
        inst.size = size;
        inst.ready_fired = true;
        GraphicsSurface::new(inst.provider.clone() as Rc<dyn SurfaceProvider>)
    };

    invoke_on_ready(instance, OnReadyEvent { surface, size });
}

// ---------------------------------------------------------------------------
// ResizeObserver — fires on_resize after the initial on_ready
// ---------------------------------------------------------------------------

fn install_resize_observer(instance: Rc<RefCell<GraphicsInstance>>) {
    let weak = Rc::downgrade(&instance);
    let cb = Closure::<dyn FnMut(JsValue, JsValue)>::new(move |_entries, _observer| {
        let Some(inst) = weak.upgrade() else { return };
        fire_resize(&inst);
    });
    let observer = match web_sys::ResizeObserver::new(cb.as_ref().unchecked_ref()) {
        Ok(o) => o,
        Err(e) => {
            web_sys::console::warn_1(
                &format!("[graphics] ResizeObserver::new failed: {e:?}").into(),
            );
            return;
        }
    };
    {
        let inst = instance.borrow();
        observer.observe(&inst.provider.canvas);
    }
    let mut inst = instance.borrow_mut();
    inst.resize_observer = Some(observer);
    inst.resize_closure = Some(cb);
}

fn fire_resize(instance: &Rc<RefCell<GraphicsInstance>>) {
    let new_size = {
        let inst = instance.borrow();
        if inst.released || !inst.ready_fired {
            return;
        }
        let dpr = web_sys::window()
            .map(|w| w.device_pixel_ratio())
            .unwrap_or(1.0);
        let cw = inst.provider.canvas.client_width();
        let ch = inst.provider.canvas.client_height();
        if cw <= 0 || ch <= 0 {
            return;
        }
        let pw = ((cw as f64) * dpr).round() as u32;
        let ph = ((ch as f64) * dpr).round() as u32;
        let new_size = (pw.max(1), ph.max(1));
        if new_size == inst.size {
            return;
        }
        new_size
    };

    {
        let mut inst = instance.borrow_mut();
        inst.size = new_size;
        inst.provider.canvas.set_width(new_size.0);
        inst.provider.canvas.set_height(new_size.1);
    }

    invoke_on_resize(instance, OnResizeEvent { size: new_size });
}

// ---------------------------------------------------------------------------
// Context-lost listener — fires on_lost
// ---------------------------------------------------------------------------

fn install_context_lost_listener(instance: Rc<RefCell<GraphicsInstance>>) {
    let weak = Rc::downgrade(&instance);
    let cb = Closure::<dyn FnMut(web_sys::Event)>::new(move |_evt| {
        let Some(inst) = weak.upgrade() else { return };
        invoke_on_lost(&inst);
    });
    {
        let inst = instance.borrow();
        // `webglcontextlost` is the standard event for WebGL
        // context loss; WebGPU uses `lost` on the device, but
        // since we don't own the device we let the author handle
        // that themselves. Still, listening on the canvas covers
        // the common WebGL path.
        let _ = inst.provider.canvas.add_event_listener_with_callback(
            "webglcontextlost",
            cb.as_ref().unchecked_ref(),
        );
    }
    instance.borrow_mut().context_lost_closure = Some(cb);
}

// ---------------------------------------------------------------------------
// Callback invocation helpers
// ---------------------------------------------------------------------------
//
// All three `invoke_*` helpers swap the user closure out of the
// instance, drop the borrow, run the closure, then put it back. This
// is essential because the closure body may call back into the
// framework (request_redraw, signal updates, etc.), which would
// re-enter the RefCell — taking the closure out releases the borrow
// before the call.

fn invoke_on_ready(instance: &Rc<RefCell<GraphicsInstance>>, event: OnReadyEvent) {
    let mut on_ready = std::mem::replace(
        &mut instance.borrow_mut().on_ready,
        Box::new(|_| {}),
    );
    on_ready(event);
    let mut inst = instance.borrow_mut();
    if !inst.released {
        inst.on_ready = on_ready;
    }
}

fn invoke_on_resize(instance: &Rc<RefCell<GraphicsInstance>>, event: OnResizeEvent) {
    let mut on_resize = std::mem::replace(
        &mut instance.borrow_mut().on_resize,
        Box::new(|_| {}),
    );
    on_resize(event);
    let mut inst = instance.borrow_mut();
    if !inst.released {
        inst.on_resize = on_resize;
    }
}

fn invoke_on_lost(instance: &Rc<RefCell<GraphicsInstance>>) {
    if instance.borrow().released {
        return;
    }
    let mut on_lost = std::mem::replace(
        &mut instance.borrow_mut().on_lost,
        Box::new(|| {}),
    );
    on_lost();
    let mut inst = instance.borrow_mut();
    if !inst.released {
        inst.on_lost = on_lost;
    }
}

// ---------------------------------------------------------------------------
// Handle / Ops
// ---------------------------------------------------------------------------

pub(crate) fn make_handle(b: &WebBackend, node: &Node) -> GraphicsHandle {
    // The handle has no methods today (the surface-provider model
    // means everything flows through the lifecycle callbacks), but
    // we still wire it up to the right instance so future
    // imperative ops can route correctly. Lookup is by
    // `data-graphics-id` attribute — same trick as the prior
    // implementation, since `&Node` Rust pointer identity doesn't
    // survive return-by-value.
    let id = node
        .clone()
        .dyn_into::<web_sys::Element>()
        .ok()
        .and_then(|el| el.get_attribute("data-graphics-id"))
        .and_then(|s| s.parse::<u32>().ok());
    let instance: Rc<RefCell<GraphicsInstance>> = match id.and_then(|i| b.graphics_instances.get(&i))
    {
        Some(rc) => rc.clone(),
        None => {
            // Unreachable in practice — `make_*_handle` is called
            // immediately after `create_*`. Hand back a dummy.
            let canvas: web_sys::HtmlCanvasElement = b
                .doc
                .create_element("canvas")
                .expect("create_element canvas failed")
                .dyn_into()
                .expect("not a canvas");
            Rc::new(RefCell::new(GraphicsInstance {
                provider: Rc::new(CanvasSurfaceProvider { canvas }),
                on_ready: Box::new(|_| {}),
                on_resize: Box::new(|_| {}),
                on_lost: Box::new(|| {}),
                size: (0, 0),
                released: false,
                ready_fired: false,
                resize_observer: None,
                resize_closure: None,
                context_lost_closure: None,
            }))
        }
    };
    GraphicsHandle::new(Rc::new(instance), &WebGraphicsOps)
}

struct WebGraphicsOps;
impl GraphicsOps for WebGraphicsOps {}

// ---------------------------------------------------------------------------
// release — called from Backend::release_graphics on unmount
// ---------------------------------------------------------------------------

pub(crate) fn release(b: &mut WebBackend, node: &Node) {
    let id = match node
        .clone()
        .dyn_into::<web_sys::Element>()
        .ok()
        .and_then(|el| el.get_attribute("data-graphics-id"))
        .and_then(|s| s.parse::<u32>().ok())
    {
        Some(i) => i,
        None => return,
    };
    let Some(instance) = b.graphics_instances.remove(&id) else { return };
    let mut inst = instance.borrow_mut();
    inst.released = true;
    if let Some(observer) = inst.resize_observer.take() {
        observer.disconnect();
    }
    inst.resize_closure = None;
    inst.context_lost_closure = None;
    // Replace the user closures with no-ops. Dropping the originals
    // releases anything they capture (including the user's render
    // state if they put it in the closure).
    inst.on_ready = Box::new(|_| {});
    inst.on_resize = Box::new(|_| {});
    inst.on_lost = Box::new(|| {});
}

// Suppress the "unused" warning on Any when nothing in this file
// references it. (Used internally by the GraphicsHandle wrapper.)
#[allow(dead_code)]
fn _silence_any(_: &dyn Any) {}
