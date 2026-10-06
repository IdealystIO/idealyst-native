//! `Window` frame / timer handles in a real browser.
//!
//! The handle a host returns from `requestAnimationFrame` / `setTimeout` is
//! an opaque JS number. Browsers hand out small integers, but a fake clock
//! does not: Playwright's `page.clock` (sinon fake-timers' scheme) numbers
//! its timers from `1e12`. These tests install exactly that shape of fake
//! clock on `window`, so they prove the handle survives the round trip
//! through wasm unchanged.

#![cfg(target_arch = "wasm32")]

use std::cell::Cell;
use std::rc::Rc;

use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};
use web_glue::js::Function;
use web_glue::{Closure, JsValue};

wasm_bindgen_test_configure!(run_in_browser);

/// Replace `window`'s frame / timer functions with a manual fake clock whose
/// ids start at `1e12` (Playwright's `idCounterStart`). `window.__fake`
/// exposes `pending()` (timers not yet run or cleared), `run()` (fire them
/// all, returning the messages of any that threw) and `restore()`.
fn install_fake_clock() {
    Function::new_no_args(
        "const w = window; \
         const real = { raf: w.requestAnimationFrame, caf: w.cancelAnimationFrame, \
                        st: w.setTimeout, ct: w.clearTimeout }; \
         let next = 1e12; const timers = new Map(); \
         const add = (f) => { const id = next++; timers.set(id, f); return id; }; \
         const clear = (id) => { timers.delete(Number(id)); }; \
         w.requestAnimationFrame = add; w.cancelAnimationFrame = clear; \
         w.setTimeout = (f) => add(f); w.clearTimeout = clear; \
         w.__fake = { \
           pending: () => timers.size, \
           run: () => { const errs = []; const due = Array.from(timers.entries()); timers.clear(); \
                        for (const [, f] of due) { try { f(0); } catch (e) { errs.push(String(e.message || e)); } } \
                        return errs.join('\\n'); }, \
           restore: () => { w.requestAnimationFrame = real.raf; w.cancelAnimationFrame = real.caf; \
                            w.setTimeout = real.st; w.clearTimeout = real.ct; delete w.__fake; }, \
         };",
    )
    .call0(&JsValue::UNDEFINED)
    .unwrap();
}

fn fake(method: &str) -> JsValue {
    Function::new_no_args(&format!("return window.__fake.{method}();"))
        .call0(&JsValue::UNDEFINED)
        .unwrap()
}

/// Regression (CrewForge kiosk e2e, web-glue 1.6.0): the camera's frame pump
/// logged `callback #N called after its Rust owner dropped it` on teardown
/// under Playwright's pinned clock. The handles were typed `i32`, so the
/// fake clock's `1e12 + n` id was ToInt32-truncated on the way into wasm;
/// `cancelAnimationFrame` then received a number the clock never issued,
/// the frame stayed queued, and it fired into the dropped `Closure`. A
/// cancelled frame and a cleared timeout must leave nothing pending.
#[wasm_bindgen_test]
fn regression_cancel_reaches_a_host_handle_past_i32_range() {
    install_fake_clock();
    let win = web_glue::dom::window().unwrap();

    let ran = Rc::new(Cell::new(0));
    let r = ran.clone();
    let frame = Closure::new(move |_| r.set(r.get() + 1));
    let raf = win.request_animation_frame(&frame);
    let r = ran.clone();
    let timeout = Closure::once(move |_| r.set(r.get() + 1));
    let st = win.set_timeout(&timeout, 10);

    let raf_is_exact = raf == 1e12;
    let st_is_exact = st == 1e12 + 1.0;
    win.cancel_animation_frame(raf);
    win.clear_timeout(st);
    drop(frame);
    drop(timeout);

    let pending = fake("pending").as_f64();
    let errors = fake("run").as_string().unwrap_or_default();
    fake("restore");

    assert_eq!(pending, Some(0.0), "cancel/clear must remove the host's timers");
    assert_eq!(errors, "", "no callback may fire after its owner dropped it");
    assert!(raf_is_exact, "the rAF handle must reach Rust unchanged, got {raf}");
    assert!(st_is_exact, "the setTimeout handle must reach Rust unchanged, got {st}");
    assert_eq!(ran.get(), 0);
}

/// The plain browser path still round-trips: a real rAF handle cancels.
#[wasm_bindgen_test]
async fn a_real_animation_frame_cancels() {
    let win = web_glue::dom::window().unwrap();
    let ran = Rc::new(Cell::new(false));
    let r = ran.clone();
    let frame = Closure::new(move |_| r.set(true));
    let h = win.request_animation_frame(&frame);
    win.cancel_animation_frame(h);
    // Two frames later the cancelled one has had every chance to run.
    for _ in 0..2 {
        let p = Function::new_no_args("return new Promise((r) => requestAnimationFrame(r));")
            .call0(&JsValue::UNDEFINED)
            .unwrap();
        let _ = web_glue::JsFuture::new(&p).await;
    }
    assert!(!ran.get(), "a cancelled frame must not run");
    drop(frame);
}
