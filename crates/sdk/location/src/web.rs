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
//! [`current`]: crate::current
//! [`watch`]: crate::watch

use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};

use crate::oneshot;
use crate::{BoxedCallback, LocationError, Position};

/// Map a `web_sys::Position` to our platform-agnostic [`Position`].
///
/// `accuracy` is always present per the spec; `altitude`/`heading`/`speed`
/// are `Option<f64>` and pass through as-is.
fn from_js(js: &web_sys::Position) -> Position {
    let c = js.coords();
    Position {
        latitude: c.latitude(),
        longitude: c.longitude(),
        accuracy_m: c.accuracy(),
        altitude: c.altitude(),
        heading: c.heading(),
        speed: c.speed(),
        // `Position.timestamp` is DOMTimeStamp = ms since the Unix epoch.
        timestamp_ms: js.timestamp(),
    }
}

/// Map a `web_sys::PositionError` to a [`LocationError`].
///
/// `code 1` = PERMISSION_DENIED → [`LocationError::NotAuthorized`]; `2`
/// (POSITION_UNAVAILABLE) and `3` (TIMEOUT) → [`LocationError::Unavailable`].
fn error_from_js(err: &web_sys::PositionError) -> LocationError {
    const PERMISSION_DENIED: u16 = 1;
    if err.code() == PERMISSION_DENIED {
        LocationError::NotAuthorized
    } else {
        LocationError::Unavailable(err.message())
    }
}

/// The browser `Geolocation`, or a `NotSupported` error when absent (no
/// `window`, or a context without geolocation).
fn geolocation() -> Result<web_sys::Geolocation, LocationError> {
    web_sys::window()
        .and_then(|w| w.navigator().geolocation().ok())
        .ok_or(LocationError::NotSupported)
}

/// High-accuracy options shared by `current_fix` and the watch.
fn options() -> web_sys::PositionOptions {
    let opts = web_sys::PositionOptions::new();
    // Prefer GPS-grade precision; the platform falls back to coarse if it
    // can't satisfy it. A generous timeout avoids a spurious `Unavailable`
    // on a cold GPS.
    opts.set_enable_high_accuracy(true);
    opts.set_timeout(30_000);
    opts
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
    // `FnMut` closures.
    let slot = std::rc::Rc::new(std::cell::RefCell::new(Some(tx)));

    // `once_into_js` hands ownership of each closure to the JS side and frees
    // it after its single invocation — exactly the getCurrentPosition shape
    // (success XOR error fires once). The `Sender` is move-once, so it lives
    // in a shared slot both closures `take()` from; whichever fires first
    // wins and the other's `take()` yields `None`.
    let slot_ok = slot.clone();
    let on_ok = Closure::once_into_js(move |pos: web_sys::Position| {
        if let Some(tx) = slot_ok.borrow_mut().take() {
            tx.send(Ok(from_js(&pos)));
        }
    });

    let slot_err = slot.clone();
    let on_err = Closure::once_into_js(move |err: web_sys::PositionError| {
        if let Some(tx) = slot_err.borrow_mut().take() {
            tx.send(Err(error_from_js(&err)));
        }
    });

    let opts = options();
    if geo
        .get_current_position_with_error_callback_and_options(
            on_ok.unchecked_ref(),
            Some(on_err.unchecked_ref()),
            &opts,
        )
        .is_err()
    {
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
    registered: Option<(web_sys::Geolocation, i32)>,
    _on_ok: Closure<dyn FnMut(JsValue)>,
    _on_err: Closure<dyn FnMut(JsValue)>,
}

impl Drop for WatchHandle {
    fn drop(&mut self) {
        // Stop the native position feed. The closures drop right after,
        // releasing the JS callbacks.
        if let Some((geo, watch_id)) = &self.registered {
            geo.clear_watch(*watch_id);
        }
    }
}

pub(crate) fn start_watch(callback: BoxedCallback) -> WatchHandle {
    let on_ok = Closure::wrap(Box::new(move |pos: JsValue| {
        if let Ok(p) = pos.dyn_into::<web_sys::Position>() {
            callback(from_js(&p));
        }
    }) as Box<dyn FnMut(JsValue)>);
    // The error closure for a watch is intentionally a no-op: a transient
    // error (lost signal) shouldn't tear the watch down — the platform keeps
    // trying and the next success fires `on_ok`. `watch` is fire-and-forget
    // updates, not a fallible request.
    let on_err = Closure::wrap(Box::new(move |_err: JsValue| {}) as Box<dyn FnMut(JsValue)>);

    // No geolocation in this context: install nothing; the caller's callback
    // never fires (matching the unsupported-target stub). Still return a guard
    // owning the closures.
    let registered = geolocation().ok().and_then(|geo| {
        let opts = options();
        geo.watch_position_with_error_callback_and_options(
            on_ok.as_ref().unchecked_ref(),
            Some(on_err.as_ref().unchecked_ref()),
            &opts,
        )
        .ok()
        .map(|id| (geo, id))
    });

    WatchHandle {
        registered,
        _on_ok: on_ok,
        _on_err: on_err,
    }
}
