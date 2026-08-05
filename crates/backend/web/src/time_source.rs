//! Web `TimeSource`: `performance.now()` via raw `js_sys` reflection.
//!
//! Reflection avoids taking a hard dep on the `web-sys` `Performance`
//! type — keeps this module tiny and decoupled from web-sys's feature
//! list. The framework's `debug-stats` feature reads through this
//! every per-frame event; cache the resolved `Function` once at
//! install rather than looking it up on every call.

use std::cell::RefCell;

use runtime_shared::time::{TimeSource, WallClockSource};
use wasm_bindgen::prelude::*;

/// Register this backend's time source with `runtime-core`.
/// Idempotent — first install wins. Should run before any
/// `debug-stats` measurement starts.
pub fn install_time_source() {
    runtime_shared::time::install_time_source(Box::new(WebTimeSource::new()));
}

/// Register this backend's wall clock (`js Date`) with `runtime-core`.
/// Idempotent — first install wins. Required on web: the shared
/// `SystemWallClockSource` default is never installed here
/// (`SystemTime::now()` panics on wasm32-unknown-unknown), so without
/// this install `runtime_core::time::epoch_millis()` reads `0` and
/// every civil-date UI thinks it's 1970.
pub fn install_wall_clock_source() {
    runtime_shared::time::install_wall_clock_source(Box::new(WebWallClockSource));
}

/// `js Date`-backed [`WallClockSource`]: `Date.now()` for the epoch
/// instant; `getTimezoneOffset()` (negated — JS reports UTC−local, the
/// trait wants local−UTC) read off a fresh `Date` per call so DST
/// transitions are honored mid-session. `js_sys::Date` is a direct
/// binding, so no reflection is needed here (the reflection below is
/// about avoiding a `web-sys` `Performance` dep, which has no `Date`
/// analogue in `js-sys`).
// (Unlike `WebTimeSource` it holds no cached `JsValue`s — each call
// constructs its `Date` fresh — so the auto `Send`/`Sync` impls apply
// and no unsafe assertion is needed.)
struct WebWallClockSource;

impl WallClockSource for WebWallClockSource {
    fn epoch_millis(&self) -> i64 {
        js_sys::Date::now() as i64
    }

    fn local_offset_minutes(&self) -> i32 {
        -(js_sys::Date::new_0().get_timezone_offset() as i32)
    }
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
