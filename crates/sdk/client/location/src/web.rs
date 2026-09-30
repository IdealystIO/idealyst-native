//! Web geolocation via `navigator.geolocation`.
//!
//! `getCurrentPosition(success, error, options)` resolves a single fix and
//! `watchPosition(success, error, options)` streams them; `clearWatch(id)`
//! stops a watch. Both success/error are JS callbacks, so a [`current`] fix
//! bridges its single callback to our `async` result through a
//! [`oneshot`](crate::oneshot) channel, and a [`watch`] keeps its success
//! closure alive for the watch's lifetime inside the handle.
//!
//! The browser surfaces the permission prompt implicitly on the first
//! `getCurrentPosition` / `watchPosition`. That reconciles with the
//! `permissions` SDK, whose web `request(LocationWhenInUse)` has no explicit
//! prompt to fire and instead reports the queryable status — an
//! `Undetermined` there means "will prompt on first use", which is exactly
//! this call. So [`current`](crate::current)'s `is_granted()` gate can be
//! `false` on a first visit; in that case `current` returns `NotAuthorized`
//! and the host re-calls after the user grants, or uses [`watch`] which
//! triggers the prompt directly. (Documented, not faked: the web permission
//! model genuinely has no pre-prompt.)
//!
//! Every browser call is a web-glue binding declared here (own-web-bindings
//! phase 3); the success/error callbacks are `web_glue::Closure`s.
//!
//! [`current`]: crate::current
//! [`watch`]: crate::watch

use web_glue::{string, Closure, JsValue};

use crate::oneshot;
use crate::{BoxedCallback, LocationError, Position};

web_glue::import! {
    // `navigator.geolocation`, or 0 without a window / the API.
    fn js_geolocation() -> u32 =
        "() => { if (typeof window === 'undefined') return 0; \
           const g = window.navigator.geolocation; return g == null ? 0 : G.add(g); }";
    // The high-accuracy options shared by both calls: prefer GPS-grade
    // precision (the platform falls back to coarse if it can't satisfy it)
    // and a generous timeout, which avoids a spurious `Unavailable` on a
    // cold GPS.
    #[catch]
    fn js_get_current_position(g: u32, ok: u32, err: u32) =
        "(g, ok, err) => { G.get(g).getCurrentPosition(G.get(ok), G.get(err), \
           { enableHighAccuracy: true, timeout: 30000 }); }";
    #[catch]
    fn js_watch_position(g: u32, ok: u32, err: u32) -> i32 =
        "(g, ok, err) => G.get(g).watchPosition(G.get(ok), G.get(err), \
           { enableHighAccuracy: true, timeout: 30000 })";
    fn js_clear_watch(g: u32, id: i32) = "(g, id) => { G.get(g).clearWatch(id); }";
    // A `GeolocationPosition` as 8 f64s at `out` (8-byte aligned):
    // latitude, longitude, accuracy, altitude, heading, speed, timestamp,
    // and a presence mask for the three nullable fields (1 altitude,
    // 2 heading, 4 speed). A mask rather than NaN-for-null because a
    // present heading may itself be NaN (the spec's "not moving").
    // 0 when `p` is not a position (no `coords`). Duck-typed on purpose:
    // the web-sys port checked `instanceof Position`, the class's pre-2020
    // name, which no current browser defines — so every `watch` update was
    // silently discarded (regression: `tests/web_geolocation.rs`).
    fn js_read_position(p: u32, out: usize) -> u32 =
        "(p, o) => { const v = G.get(p); if (v == null || v.coords == null) return 0; \
           const c = v.coords; \
           const f = new Float64Array(G.u8().buffer, o >>> 0, 8); \
           f[0] = c.latitude; f[1] = c.longitude; f[2] = c.accuracy; \
           f[3] = c.altitude ?? 0; f[4] = c.heading ?? 0; f[5] = c.speed ?? 0; \
           f[6] = v.timestamp; \
           f[7] = (c.altitude == null ? 0 : 1) | (c.heading == null ? 0 : 2) | (c.speed == null ? 0 : 4); \
           return 1; }";
    // `GeolocationPositionError.code` (0 when absent) and `.message`.
    fn js_error_code(e: u32) -> u32 = "(e) => { const v = G.get(e); return v == null ? 0 : (v.code >>> 0); }";
    fn js_error_message(e: u32, out: usize) =
        "(e, o) => { const v = G.get(e); G.retStr(v == null || v.message == null ? '' : String(v.message), o); }";
}

/// Map a JS `GeolocationPosition` to our platform-agnostic [`Position`];
/// `None` if the value isn't one.
///
/// `accuracy` is always present per the spec; `altitude`/`heading`/`speed`
/// are nullable and pass through as `Option<f64>`.
fn from_js(js: &JsValue) -> Option<Position> {
    let mut f = [0f64; 8];
    if unsafe { js_read_position(js.raw(), f.as_mut_ptr() as usize) } == 0 {
        return None;
    }
    let mask = f[7] as u32;
    let opt = |v: f64, bit: u32| (mask & bit != 0).then_some(v);
    Some(Position {
        latitude: f[0],
        longitude: f[1],
        accuracy_m: f[2],
        altitude: opt(f[3], 1),
        heading: opt(f[4], 2),
        speed: opt(f[5], 4),
        // `timestamp` is an EpochTimeStamp = ms since the Unix epoch.
        timestamp_ms: f[6],
    })
}

/// Map a JS `GeolocationPositionError` to a [`LocationError`].
///
/// `code 1` = PERMISSION_DENIED → [`LocationError::NotAuthorized`]; `2`
/// (POSITION_UNAVAILABLE) and `3` (TIMEOUT) → [`LocationError::Unavailable`].
fn error_from_js(err: &JsValue) -> LocationError {
    const PERMISSION_DENIED: u32 = 1;
    if unsafe { js_error_code(err.raw()) } == PERMISSION_DENIED {
        LocationError::NotAuthorized
    } else {
        LocationError::Unavailable(string::receive(|o| unsafe { js_error_message(err.raw(), o) }))
    }
}

/// The browser `Geolocation`, or a `NotSupported` error when absent (no
/// `window`, or a context without geolocation).
fn geolocation() -> Result<JsValue, LocationError> {
    match unsafe { js_geolocation() } {
        0 => Err(LocationError::NotSupported),
        // SAFETY: a fresh `G.add` slot the snippet minted for us.
        h => Ok(unsafe { JsValue::from_raw(h) }),
    }
}

pub(crate) async fn current_fix() -> Result<Position, LocationError> {
    let geo = geolocation()?;

    // Bridge the success/error JS callbacks to one async result. The fallback
    // (sender dropped without firing) is `Unavailable` — a callback that
    // never fires reads as "no fix" rather than hanging.
    let (tx, rx) = oneshot::channel::<Result<Position, LocationError>>(Err(
        LocationError::Unavailable("geolocation callback never fired".into()),
    ));
    // One `Sender` shared between the success and error closures; whichever
    // fires first wins, the other becomes a no-op (`send` is once-only). An
    // `Rc<RefCell<Option<Sender>>>` carries the move-once `Sender` into two
    // closures.
    let slot = std::rc::Rc::new(std::cell::RefCell::new(Some(tx)));

    // `once_into_js` hands ownership of each closure to the JS side and frees
    // it after its single invocation — exactly the getCurrentPosition shape
    // (success XOR error fires once). The `Sender` is move-once, so it lives
    // in a shared slot both closures `take()` from; whichever fires first
    // wins and the other's `take()` yields `None`.
    let slot_ok = slot.clone();
    let on_ok = Closure::once_into_js(move |pos: JsValue| {
        if let Some(tx) = slot_ok.borrow_mut().take() {
            tx.send(from_js(&pos).ok_or_else(|| {
                LocationError::Unavailable("getCurrentPosition delivered no position".into())
            }));
        }
    });

    let slot_err = slot.clone();
    let on_err = Closure::once_into_js(move |err: JsValue| {
        if let Some(tx) = slot_err.borrow_mut().take() {
            tx.send(Err(error_from_js(&err)));
        }
    });

    let called = unsafe { js_get_current_position(geo.raw(), on_ok.raw(), on_err.raw()) };
    // The handles above are slab slots only; the JS functions stay alive
    // inside the pending geolocation request.
    drop((geo, on_ok, on_err));
    if called.is_err() {
        return Err(LocationError::Unavailable("getCurrentPosition threw".into()));
    }

    rx.await
}

/// Keeps the watch's success/error closures alive and clears the native watch
/// on drop. Holding the closures is what keeps the JS callbacks valid for the
/// watch's lifetime; `clearWatch` stops the underlying feed.
///
/// `registered` is `None` in a context without geolocation (the closures are
/// still owned so `watch`'s contract — "you get a guard" — holds, but there's
/// nothing to clear on drop).
pub(crate) struct WatchHandle {
    registered: Option<(JsValue, i32)>,
    _on_ok: Closure,
    _on_err: Closure,
}

impl Drop for WatchHandle {
    fn drop(&mut self) {
        // Stop the native position feed. The closures drop right after,
        // releasing the JS callbacks.
        if let Some((geo, watch_id)) = &self.registered {
            unsafe { js_clear_watch(geo.raw(), *watch_id) }
        }
    }
}

pub(crate) fn start_watch(callback: BoxedCallback) -> WatchHandle {
    let on_ok = Closure::new(move |pos: JsValue| {
        if let Some(p) = from_js(&pos) {
            callback(p);
        }
    });
    // The error closure for a watch is intentionally a no-op: a transient
    // error (lost signal) shouldn't tear the watch down — the platform keeps
    // trying and the next success fires `on_ok`. `watch` is fire-and-forget
    // updates, not a fallible request.
    let on_err = Closure::new(move |_err: JsValue| {});

    // No geolocation in this context: install nothing; the caller's callback
    // never fires (matching the unsupported-target stub). Still return a guard
    // owning the closures.
    let registered = geolocation().ok().and_then(|geo| {
        unsafe { js_watch_position(geo.raw(), on_ok.as_js().raw(), on_err.as_js().raw()) }
            .ok()
            .map(|id| (geo, id))
    });

    WatchHandle {
        registered,
        _on_ok: on_ok,
        _on_err: on_err,
    }
}
