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
//! Safari/Firefox, so its binding reads it defensively and we fall back to
//! [`Transport::Other`] when it (or a usable field) is missing. We never key
//! online-ness off it — only the transport hint.
//!
//! Every browser call is a web-glue binding declared here (own-web-bindings
//! phase 3); the listeners are `web_glue::dom::Listener`s.

use std::rc::Rc;

use web_glue::dom::{self, Listener, ListenerOptions};
use web_glue::string;

use crate::{Connectivity, Transport, WatchCallback};

web_glue::import! {
    // `navigator.onLine` as 1/0, or 2 without a window.
    fn js_on_line() -> u32 =
        "() => typeof window === 'undefined' ? 2 : (window.navigator.onLine ? 1 : 0)";
    // `navigator.connection[key]` written to `out` when it is a string;
    // 0 when there is no window, no `connection` (Safari / Firefox), or no
    // string under `key`.
    fn js_connection_str(kp: usize, kl: usize, out: usize) -> u32 =
        "(kp, kl, o) => { if (typeof window === 'undefined') return 0; \
           const c = window.navigator.connection; if (c == null) return 0; \
           const v = c[G.str(kp, kl)]; if (typeof v !== 'string') return 0; \
           G.retStr(v, o); return 1; }";
    fn js_console_error(p: usize, l: usize) = "(p, l) => { console.error(G.str(p, l)); }";
}

/// Read `navigator.onLine`. Defaults to `true` if `window`/`navigator` is
/// somehow unavailable (e.g. a worker without `WorkerNavigator.onLine`) —
/// the same "assume reachable" best-effort the rest of the SDK uses.
fn navigator_online() -> bool {
    unsafe { js_on_line() != 0 }
}

/// Best-effort transport from `navigator.connection`. The NetworkInformation
/// object exposes a `type` (`"wifi"`/`"cellular"`/`"ethernet"`/…) on some
/// engines and an `effectiveType` (`"4g"`/`"3g"`/…) more widely; neither is
/// guaranteed. We prefer the concrete `type`, treat any cellular-ish
/// `effectiveType` as cellular, and otherwise report [`Transport::Other`].
fn navigator_transport() -> Transport {
    if let Some(kind) = connection_string("type") {
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
    if let Some(eff) = connection_string("effectiveType") {
        if matches!(eff.as_str(), "slow-2g" | "2g" | "3g" | "4g" | "5g") {
            return Transport::Cellular;
        }
    }

    Transport::Other
}

/// A string-valued property of `navigator.connection`, or `None` if the
/// object or the property is missing / not a string.
fn connection_string(key: &str) -> Option<String> {
    let (kp, kl) = string::abi(key);
    let mut hit = 0;
    let s = string::receive(|o| hit = unsafe { js_connection_str(kp, kl, o) });
    (hit != 0).then_some(s)
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
    // One handler serves both `online` and `offline`; it re-reads the full
    // snapshot so the transport hint is refreshed too, then forwards it.
    let handler: Rc<dyn Fn()> = Rc::new(move || {
        // FFI boundary: a panic in `callback` must not unwind into the JS
        // event dispatch (UB across the wasm/JS boundary). Catch + log; the
        // listener stays registered for the next event.
        let snap = snapshot();
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(snap))).is_err() {
            let msg = "connectivity: watch callback panicked (swallowed at the JS boundary)";
            let (p, l) = string::abi(msg);
            unsafe { js_console_error(p, l) }
        }
    });

    // `Listener` detaches itself before its closure drops, so dropping the
    // subscription can never leave `window` holding a dead callback.
    // `new_fn`: the handler is re-entrant, as the `dyn Fn` closure it
    // replaces was.
    let listeners = dom::window()
        .map(|w| {
            let on = handler.clone();
            let off = handler;
            [
                Listener::new_fn(w.clone().into(), "online", ListenerOptions::default(), move |_| on()),
                Listener::new_fn(w.into(), "offline", ListenerOptions::default(), move |_| off()),
            ]
        });

    Subscription { _listeners: listeners }
}

/// Web subscription: the two window listeners, removed (and their closures
/// released) when the guard drops. Holding them here (not `forget`ting
/// them) is what keeps the listener live exactly as long as the
/// subscription.
pub(crate) struct Subscription {
    _listeners: Option<[Listener; 2]>,
}
