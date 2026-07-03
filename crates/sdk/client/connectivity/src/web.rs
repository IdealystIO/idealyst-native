//! Web reachability via `navigator.onLine` + `online`/`offline` window
//! events, with a transport hint from `navigator.connection`
//! (NetworkInformation) where the browser exposes it.
//!
//! `navigator.onLine` is the only universally-available signal and it's
//! coarse — it's `false` only when the browser knows it has no network, and
//! `true` otherwise (even on a captive portal). That matches this SDK's
//! "reachability category, best-effort" contract. The `online`/`offline`
//! events fire on that same flag flipping, so they drive [`watch`].
//!
//! NetworkInformation (`navigator.connection`) is non-standard and absent in
//! Safari/Firefox, so we read it defensively via `Reflect` and fall back to
//! [`Transport::Other`] when it (or a usable field) is missing. We never key
//! online-ness off it — only the transport hint.

use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};

use crate::{Connectivity, Transport, WatchCallback};

/// Read `navigator.onLine`. Defaults to `true` if `window`/`navigator` is
/// somehow unavailable (e.g. a worker without `WorkerNavigator.onLine`) —
/// the same "assume reachable" best-effort the rest of the SDK uses.
fn navigator_online() -> bool {
    web_sys::window()
        .map(|w| w.navigator().on_line())
        .unwrap_or(true)
}

/// Best-effort transport from `navigator.connection`. The NetworkInformation
/// object exposes a `type` (`"wifi"`/`"cellular"`/`"ethernet"`/…) on some
/// engines and an `effectiveType` (`"4g"`/`"3g"`/…) more widely; neither is
/// guaranteed. We prefer the concrete `type`, treat any cellular-ish
/// `effectiveType` as cellular, and otherwise report [`Transport::Other`].
fn navigator_transport() -> Transport {
    let Some(window) = web_sys::window() else {
        return Transport::Other;
    };
    let navigator = window.navigator();

    // `navigator.connection` — not in web-sys's typed surface on all
    // versions, and absent at runtime in several browsers, so reach it via
    // Reflect and bail to Other on any miss.
    let conn = match js_sys::Reflect::get(navigator.as_ref(), &JsValue::from_str("connection")) {
        Ok(c) if !c.is_undefined() && !c.is_null() => c,
        _ => return Transport::Other,
    };

    if let Some(kind) = reflect_string(&conn, "type") {
        match kind.as_str() {
            "wifi" => return Transport::Wifi,
            "cellular" => return Transport::Cellular,
            "ethernet" => return Transport::Ethernet,
            // "none" would mean offline; the online/offline flag is
            // authoritative for that, so just fall through to the hint below.
            _ => {}
        }
    }

    // `effectiveType` is a speed bucket, not a medium, but a present value is
    // a strong signal of a mobile-data link on engines that omit `type`.
    if let Some(eff) = reflect_string(&conn, "effectiveType") {
        if matches!(eff.as_str(), "slow-2g" | "2g" | "3g" | "4g" | "5g") {
            return Transport::Cellular;
        }
    }

    Transport::Other
}

/// Read a string-valued property off a JS object, or `None` if missing /
/// not a string.
fn reflect_string(obj: &JsValue, key: &str) -> Option<String> {
    js_sys::Reflect::get(obj, &JsValue::from_str(key))
        .ok()
        .and_then(|v| v.as_string())
}

/// Compose a [`Connectivity`] from the `onLine` flag + transport hint,
/// keeping the online/transport pair consistent.
fn snapshot() -> Connectivity {
    if navigator_online() {
        Connectivity {
            online: true,
            transport: navigator_transport(),
        }
    } else {
        Connectivity::OFFLINE
    }
}

pub(crate) fn current() -> Connectivity {
    snapshot()
}

pub(crate) fn watch(callback: WatchCallback) -> Subscription {
    // One closure handles both `online` and `offline`; it re-reads the full
    // snapshot so the transport hint is refreshed too, then forwards it.
    let handler = Closure::<dyn Fn()>::new(move || {
        // FFI boundary: a panic in `callback` must not unwind into the JS
        // event dispatch (UB across the wasm/JS boundary). Catch + log; the
        // listener stays registered for the next event.
        let snap = snapshot();
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(snap))).is_err() {
            web_sys::console::error_1(&JsValue::from_str(
                "connectivity: watch callback panicked (swallowed at the JS boundary)",
            ));
        }
    });

    let target: Option<web_sys::EventTarget> =
        web_sys::window().map(|w| w.unchecked_into::<web_sys::EventTarget>());

    if let Some(t) = &target {
        let f = handler.as_ref().unchecked_ref();
        let _ = t.add_event_listener_with_callback("online", f);
        let _ = t.add_event_listener_with_callback("offline", f);
    }

    Subscription {
        target,
        handler: Some(handler),
    }
}

/// Web subscription: removes both event listeners and drops the JS closure on
/// teardown. Holding the `Closure` here (not `forget`ting it) is what keeps
/// the listener live exactly as long as the subscription — and frees it when
/// the caller drops the guard.
pub(crate) struct Subscription {
    target: Option<web_sys::EventTarget>,
    handler: Option<Closure<dyn Fn()>>,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let (Some(t), Some(handler)) = (&self.target, &self.handler) {
            let f = handler.as_ref().unchecked_ref();
            let _ = t.remove_event_listener_with_callback("online", f);
            let _ = t.remove_event_listener_with_callback("offline", f);
        }
        // `handler` drops here, releasing the JS closure — no leak.
    }
}
