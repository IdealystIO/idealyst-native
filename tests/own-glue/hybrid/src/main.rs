//! Hybrid mode: one module using BOTH `web-glue` and wasm-bindgen
//! (web-sys `console::log_1`, a `#[wasm_bindgen]` export). Packaged by
//! `build_web::own_glue::package_hybrid`: glue extracted and stripped,
//! wasm-bindgen run over the rest, `pkg/__idealyst_glue.js` written for the
//! `./__idealyst_glue.js` import namespace wasm-bindgen passes through.
//!
//! What it proves (the E2E asserts each):
//! * both namespaces are supplied and both work in one instance;
//! * glue callbacks and glue strings reach the web-glue exports
//!   (`__glue_invoke`, `__glue_alloc`) through wasm-bindgen's instance;
//! * static constructors ran exactly once even after many JS → Rust glue
//!   calls — i.e. wasm-bindgen's handling of the bin's command exports
//!   also covers web-glue's exports (`ctor_runs`).

#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};

use wasm_bindgen::prelude::*;
use web_glue::{import, spawn_local, string, Closure, JsFuture, JsValue};

static CTOR_RUNS: AtomicU32 = AtomicU32::new(0);

extern "C" fn count_ctor() {
    CTOR_RUNS.fetch_add(1, Ordering::Relaxed);
}

/// A static constructor (what `inventory::submit!` expands to). If any
/// export re-ran `__wasm_call_ctors`, this would count past 1.
#[cfg(target_arch = "wasm32")]
#[used]
#[unsafe(link_section = ".init_array.00099")]
static CTOR: extern "C" fn() = count_ctor;

import! {
    fn js_body() -> u32 = "() => G.add(document.body)";
    fn js_div(parent: u32, p: usize, l: usize) -> u32 =
        "(par, p, l) => { const d = document.createElement('div'); d.id = G.str(p, l); G.get(par).appendChild(d); return G.add(d); }";
    fn js_set_text(el: u32, p: usize, l: usize) = "(e, p, l) => { G.get(e).textContent = G.str(p, l); }";
    fn js_text(el: u32, out: usize) = "(e, o) => G.retStr(G.get(e).textContent, o)";
    fn js_on_click(el: u32, f: u32) = "(e, f) => { G.get(e).addEventListener('click', G.get(f)); }";
    fn js_sleep(ms: f64) -> u32 = "(ms) => G.add(new Promise((r) => setTimeout(() => r(ms), ms)))";
}

fn div(parent: &JsValue, id: &str) -> JsValue {
    let (p, l) = string::abi(id);
    unsafe { JsValue::from_raw(js_div(parent.raw(), p, l)) }
}

fn set_text(el: &JsValue, s: &str) {
    let (p, l) = string::abi(s);
    unsafe { js_set_text(el.raw(), p, l) }
}

fn text(el: &JsValue) -> String {
    string::receive(|o| unsafe { js_text(el.raw(), o) })
}

thread_local! {
    static KEEP: std::cell::RefCell<Vec<Closure>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn main() {
    web_sys::console::log_1(&"hybrid: hello from web-sys".into());

    let body = unsafe { JsValue::from_raw(js_body()) };
    let glue = div(&body, "hybrid-glue");
    set_text(&glue, "from web-glue ✓");
    // JS → Rust string through __glue_alloc, inside wasm-bindgen's instance.
    let echoed = div(&body, "hybrid-echo");
    set_text(&echoed, &format!("echo {}", text(&glue)));

    let button = div(&body, "hybrid-btn");
    set_text(&button, "click");
    let clicks = div(&body, "hybrid-clicks");
    let n = Rc::new(Cell::new(0u32));
    let on_click = Closure::new(move |_| {
        n.set(n.get() + 1);
        set_text(&clicks, &format!("{} clicks", n.get()));
    });
    unsafe { js_on_click(button.raw(), on_click.as_js().raw()) };
    KEEP.with(|k| k.borrow_mut().push(on_click));

    let asyncd = div(&body, "hybrid-async");
    set_text(&asyncd, "pending");
    spawn_local(async move {
        let ms = JsFuture::from(unsafe { JsValue::from_raw(js_sleep(20.0)) }).await;
        set_text(&asyncd, &format!("timer {}", ms.ok().and_then(|v| v.as_f64()).unwrap_or(-1.0)));
    });
}

/// A wasm-bindgen export, called from the page through wasm-bindgen's JS.
#[wasm_bindgen]
pub fn bindgen_greet(name: &str) -> String {
    format!("hello {name} from wasm-bindgen ✓")
}

/// How many times static constructors have run.
#[wasm_bindgen]
pub fn ctor_runs() -> u32 {
    CTOR_RUNS.load(Ordering::Relaxed)
}
