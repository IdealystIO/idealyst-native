//! own-glue-demo — the phase-1 proof that the framework can own the web JS
//! boundary: DOM, events, a timer promise, a caught JS exception and clean
//! handle/callback teardown, on `web-glue` alone. There is no
//! wasm-bindgen, web-sys or js-sys anywhere in this crate's graph, and it
//! is packaged by `build_web::own_glue` without the wasm-bindgen CLI.
//!
//! `crates/tools/build/web/tests/own_glue_e2e.rs` builds it, loads it in
//! headless Chrome and asserts on what it renders. The `bench_*` exports are
//! the call-overhead microbenchmark; `websys-demo` has the web-sys twins.

// wasm32-only app; off wasm32 it compiles (so workspace-wide checks pass)
// but every glue call would panic.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

mod dom;
#[cfg(all(feature = "selftest", target_arch = "wasm32"))]
mod selftest;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use web_glue::{spawn_local, Closure, JsFuture, JsValue};

thread_local! {
    /// Callbacks that live as long as the page.
    static KEEP: RefCell<Vec<Closure>> = const { RefCell::new(Vec::new()) };
    static BENCH_HOST: RefCell<Option<JsValue>> = const { RefCell::new(None) };
    static BENCH_ROWS: RefCell<Vec<JsValue>> = const { RefCell::new(Vec::new()) };
}

fn main() {
    // The same module also runs inside the self-check's Worker
    // (`web_glue::worker`), where there is no DOM to build.
    if web_glue::worker::in_worker() {
        return;
    }
    let body = dom::body();
    let title = dom::child(&body, "h1", "title");
    dom::set_text(&title, "own-glue demo ✓");

    // Click → Rust state → DOM.
    let button = dom::child(&body, "button", "btn");
    dom::set_text(&button, "click me");
    let count_el = dom::child(&body, "span", "count");
    dom::set_text(&count_el, "0");
    let count = Rc::new(Cell::new(0u32));
    let on_click = Closure::new(move |event: JsValue| {
        count.set(count.get() + 1);
        let kind = event.get("type").ok().and_then(|t| t.as_string()).unwrap_or_default();
        dom::set_text(&count_el, &format!("{} ({kind})", count.get()));
    });
    dom::add_listener(&button, "click", on_click.as_js());
    KEEP.with(|k| k.borrow_mut().push(on_click));

    // A caught JS exception.
    let exc = dom::child(&body, "div", "exception");
    match dom::throw_range_error("bad value ✓") {
        Ok(()) => dom::set_text(&exc, "no exception?"),
        Err(e) => dom::set_text(&exc, &format!("caught {}", e.message())),
    }

    // A promise awaited on the glue executor, plus a rejection.
    let async_el = dom::child(&body, "div", "async");
    dom::set_text(&async_el, "pending");
    let status = dom::child(&body, "div", "status");
    BENCH_HOST.with(|h| *h.borrow_mut() = Some(dom::child(&body, "div", "bench")));
    spawn_local(async move {
        let v = JsFuture::from(dom::sleep(30.0)).await;
        let slept = v.ok().and_then(|v| v.as_f64()).unwrap_or(-1.0);
        let promise = JsValue::global().get("Promise").expect("Promise");
        let rejected = promise
            .call_method("reject", &[&JsValue::from_str("nope ✓")])
            .expect("Promise.reject");
        let rejection = match JsFuture::from(rejected).await {
            Ok(_) => "resolved?".to_string(),
            Err(e) => e.message(),
        };
        dom::set_text(&async_el, &format!("timer {slept}ms; rejection {rejection}"));
        #[cfg(all(feature = "selftest", target_arch = "wasm32"))]
        selftest::run(&body);
        #[cfg(all(feature = "selftest", target_arch = "wasm32"))]
        selftest::worker(&body).await;
        dom::set_text(&status, "ready");
    });

}

fn bench_host() -> JsValue {
    BENCH_HOST.with(|h| h.borrow().clone().expect("main ran"))
}

/// Create `n` rows, each with three attributes and text.
#[unsafe(no_mangle)]
pub extern "C" fn bench_create(n: u32) {
    let host = bench_host();
    let mut rows = Vec::with_capacity(n as usize);
    for i in 0..n {
        let el = dom::create("div");
        dom::set_attr(&el, "class", "row");
        dom::set_attr(&el, "data-index", &i.to_string());
        dom::set_attr(&el, "title", "benchmark row");
        dom::set_text(&el, &format!("row {i}"));
        dom::append(&host, &el);
        rows.push(el);
    }
    BENCH_ROWS.with(|r| *r.borrow_mut() = rows);
}

/// Set every row's text again.
#[unsafe(no_mangle)]
pub extern "C" fn bench_update() {
    BENCH_ROWS.with(|r| {
        for (i, el) in r.borrow().iter().enumerate() {
            dom::set_text(el, &format!("updated {i}"));
        }
    });
}

/// Drop every row handle and empty the host.
#[unsafe(no_mangle)]
pub extern "C" fn bench_clear() {
    BENCH_ROWS.with(|r| r.borrow_mut().clear());
    dom::clear(&bench_host());
}

static CTOR_RUNS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

extern "C" fn count_ctor() {
    CTOR_RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// A static constructor (what `inventory::submit!` expands to). The loader
/// runs `__wasm_call_ctors` once; the reactor link keeps LLD from re-running
/// it at the top of every export.
#[cfg(target_arch = "wasm32")]
#[used]
#[unsafe(link_section = ".init_array.00099")]
static CTOR: extern "C" fn() = count_ctor;

/// How many times static constructors have run.
#[unsafe(no_mangle)]
pub extern "C" fn ctor_runs() -> u32 {
    CTOR_RUNS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Handles Rust owns right now (the E2E checks the bench releases them).
#[unsafe(no_mangle)]
pub extern "C" fn live_handles() -> u32 {
    JsValue::live_count() as u32
}

/// Slots in use in the JS slab.
#[unsafe(no_mangle)]
pub extern "C" fn js_live_handles() -> u32 {
    JsValue::js_live_count()
}
