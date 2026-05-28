// BINDGEN side, otherwise identical to `side`: depends on runtime-core, uses
// `format!`, builds an `Element`. The ONLY difference vs the working
// non-bindgen `side` is the `#[wasm_bindgen]` items + the wasm-bindgen
// post-process. Linked against the fmt-having non-bindgen `main`.
use runtime_core::signal;
use runtime_core::{text, view, Element, IntoElement};
use dynlink_shared::DYNLINK_COUNTER;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console)]
    fn log(s: &str);
}

// Static-string web API call — proven to work in a bindgen side already.
#[wasm_bindgen]
pub fn sidebg_log_static() {
    log("sidebg: static string over the dynamic link");
}

// The fmt path: format! in a bindgen side. THIS is the suspected failure.
#[wasm_bindgen]
pub fn sidebg_log_fmt(n: i32) {
    log(&format!("sidebg: format! n={}", n));
}

#[no_mangle]
pub extern "C" fn sidebg_bump() -> i32 {
    let c = &DYNLINK_COUNTER.0;
    c.set(c.get() + 100);
    c.get()
}

#[no_mangle]
pub extern "C" fn sidebg_signal() -> i32 {
    let s = signal!(9i32);
    s.get()
}

// format! → Element, the realistic lazy-body shape (same as `side`'s
// side_make_view, but in a bindgen module).
#[no_mangle]
pub extern "C" fn sidebg_make_view() -> *mut Element {
    let p: Element =
        view(vec![text(format!("bindgen side #{}", 7)).into_element()]).into_element();
    Box::into_raw(Box::new(p))
}

// wgpu-CLASS proxy: real web-sys object/externref machinery from the side —
// create a <canvas>, append it to #gpu-slot, acquire a WebGL2 context, and
// clear it to a color. This is the exact bindgen surface (DOM creation,
// getContext returning a JS object held as externref, method calls with
// object args) that the simulator's wgpu init exercises. Returns 1 on success,
// a negative code on each failure point.
#[wasm_bindgen]
pub fn sidebg_make_canvas() -> i32 {
    use wasm_bindgen::JsCast;
    let win = match web_sys::window() { Some(w) => w, None => return -1 };
    let doc = match win.document() { Some(d) => d, None => return -2 };
    let slot = match doc.get_element_by_id("gpu-slot") { Some(e) => e, None => return -3 };
    let canvas = match doc.create_element("canvas") { Ok(c) => c, Err(_) => return -4 };
    let canvas: web_sys::HtmlCanvasElement = match canvas.dyn_into() { Ok(c) => c, Err(_) => return -5 };
    canvas.set_width(240);
    canvas.set_height(160);
    let _ = canvas.set_attribute("id", "gpu-canvas");
    if slot.append_child(&canvas).is_err() { return -6; }
    let ctx = match canvas.get_context("webgl2") { Ok(Some(c)) => c, _ => return -7 };
    let gl: web_sys::WebGl2RenderingContext = match ctx.dyn_into() { Ok(g) => g, Err(_) => return -8 };
    log(&format!("sidebg: got WebGL2 context, clearing (n={})", 3));
    gl.clear_color(0.10, 0.55, 0.85, 1.0);
    gl.clear(web_sys::WebGl2RenderingContext::COLOR_BUFFER_BIT);
    1
}
