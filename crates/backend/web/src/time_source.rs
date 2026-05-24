//! Web `TimeSource`: `performance.now()` via raw `js_sys` reflection.
//!
//! Reflection avoids taking a hard dep on the `web-sys` `Performance`
//! type — keeps this module tiny and decoupled from web-sys's feature
//! list. The framework's `debug-stats` feature reads through this
//! every per-frame event; cache the resolved `Function` once at
//! install rather than looking it up on every call.

use std::cell::RefCell;

use runtime_core::time::TimeSource;
use wasm_bindgen::prelude::*;

/// Register this backend's time source with `runtime-core`.
/// Idempotent — first install wins. Should run before any
/// `debug-stats` measurement starts.
pub fn install_time_source() {
    runtime_core::time::install_time_source(Box::new(WebTimeSource::new()));
}

struct WebTimeSource {
    // `performance.now` resolved at install time and cached, plus
    // the `performance` object it must be invoked against. Both
    // `!Send`, but the trait method is only ever called on the JS
    // main thread on wasm32 so we wrap them in a `RefCell` and
    // suppress the auto-impl Send/Sync via the trait's required
    // bounds (see the unsafe impls below).
    state: RefCell<Option<PerfAccess>>,
}

struct PerfAccess {
    performance: JsValue,
    now: js_sys::Function,
}

// SAFETY: wasm32 is single-threaded; `Send`/`Sync` exist only to
// satisfy `OnceLock<Box<dyn TimeSource>>`'s storage bounds. The
// inner JsValue / Function are never actually moved between
// threads at runtime.
unsafe impl Send for WebTimeSource {}
unsafe impl Sync for WebTimeSource {}

impl WebTimeSource {
    fn new() -> Self {
        let access = resolve_performance();
        Self {
            state: RefCell::new(access),
        }
    }
}

impl TimeSource for WebTimeSource {
    fn now_micros(&self) -> u64 {
        let state = self.state.borrow();
        let Some(access) = state.as_ref() else {
            return 0;
        };
        match access.now.call0(&access.performance) {
            Ok(ret) => match ret.as_f64() {
                Some(ms) => (ms * 1000.0).max(0.0) as u64,
                None => 0,
            },
            Err(_) => 0,
        }
    }
}

fn resolve_performance() -> Option<PerfAccess> {
    let global = js_sys::global();
    let perf = js_sys::Reflect::get(&global, &JsValue::from_str("performance")).ok()?;
    if perf.is_undefined() || perf.is_null() {
        return None;
    }
    let now = js_sys::Reflect::get(&perf, &JsValue::from_str("now")).ok()?;
    let now: js_sys::Function = now.dyn_into().ok()?;
    Some(PerfAccess { performance: perf, now })
}
