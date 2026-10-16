//! Robot test controls: fake network reachability for E2E tests (take the
//! app offline, put it on cellular). Compiled only into robot builds
//! (`runtime_vocabulary::robot_select!` in `lib.rs`).
//!
//! While a fake is set, [`crate::current`] reports it and every live
//! [`crate::watch`] receives it; the platform's own change reports are
//! dropped until `connectivity.clear`, which hands the watches the real
//! state again if it differs.

use std::cell::RefCell;
use std::sync::{Arc, Mutex, Weak};

use runtime_core::robot_fn;
use runtime_vocabulary::robot_fn::register as register_fn;

use crate::{Connectivity, Transport, WatchCallback};

/// Read from the platform's delivery thread (a real change checks it), so
/// a `Mutex`, not a thread-local.
static FAKE: Mutex<Option<Connectivity>> = Mutex::new(None);

/// The callback behind a lock: a fake delivered from the UI thread and a
/// real one from the platform's thread never run it at once.
type Shared = Arc<Mutex<WatchCallback>>;

runtime_tls::thread_local! {
    /// Every watch installed on this (the UI) thread. Weak: the watch owns
    /// its callback.
    static WATCHES: RefCell<Vec<Weak<Mutex<WatchCallback>>>> = const { RefCell::new(Vec::new()) };
}

/// Keeps a watch's callback reachable by a fake for the watch's lifetime,
/// even on a backend that drops the callback it is given.
pub(crate) struct WatchTap {
    _callback: Shared,
}

pub(crate) fn fake() -> Option<Connectivity> {
    *FAKE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Wrap a watch's callback: the platform's reports pass through only while
/// no fake is set, and the callback joins the fake's fan-out.
pub(crate) fn tap_watch(callback: WatchCallback) -> (WatchCallback, WatchTap) {
    let shared: Shared = Arc::new(Mutex::new(callback));
    WATCHES.with(|w| w.borrow_mut().push(Arc::downgrade(&shared)));
    let real = shared.clone();
    // Checked under the callback's lock, which a fake's delivery also
    // takes: a real report racing `connectivity.set` either lands before
    // the fake or is dropped, never after it.
    let filtered: WatchCallback = Box::new(move |net| {
        let callback = real.lock().unwrap_or_else(|e| e.into_inner());
        if fake().is_none() {
            callback(net);
        }
    });
    (filtered, WatchTap { _callback: shared })
}

fn deliver(net: Connectivity) {
    let live: Vec<Shared> = WATCHES.with(|w| {
        let mut w = w.borrow_mut();
        w.retain(|cb| cb.strong_count() > 0);
        w.iter().filter_map(Weak::upgrade).collect()
    });
    for cb in live {
        (cb.lock().unwrap_or_else(|e| e.into_inner()))(net);
    }
}

fn parse_transport(name: &str) -> Result<Transport, String> {
    Ok(match name {
        "wifi" => Transport::Wifi,
        "cellular" => Transport::Cellular,
        "ethernet" => Transport::Ethernet,
        "other" => Transport::Other,
        "none" => Transport::None,
        other => return Err(format!("unknown transport '{other}' (wifi|cellular|ethernet|other|none)")),
    })
}

/// Fake the network: `online: false` takes the app offline; online,
/// `transport` is `wifi` (the default), `cellular`, `ethernet` or `other`.
/// `current()` reports it and every live `watch` receives it now; the
/// platform's own reports are ignored until `connectivity.clear`.
#[robot_fn(name = "connectivity.set")]
fn set(online: bool, transport: Option<String>) -> Result<(), String> {
    let net = match (online, transport.as_deref()) {
        (false, None | Some("none")) => Connectivity::OFFLINE,
        (false, Some(t)) => return Err(format!("an offline network has no transport (got '{t}')")),
        (true, Some("none")) => return Err("an online network needs a transport other than 'none'".into()),
        (true, t) => Connectivity { online: true, transport: parse_transport(t.unwrap_or("wifi"))? },
    };
    let changed = fake() != Some(net);
    *FAKE.lock().unwrap_or_else(|e| e.into_inner()) = Some(net);
    if changed {
        deliver(net);
    }
    Ok(())
}

/// Stop faking: `current()` and `watch` read the platform again, and the
/// watches receive the real state if it differs from the fake.
#[robot_fn(name = "connectivity.clear")]
fn clear() {
    let Some(was) = FAKE.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
    let real = crate::imp::current();
    if real != was {
        deliver(real);
    }
}

pub(crate) fn register() {
    register_fn(set::robot());
    register_fn(clear::robot());
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_core::__serde_json::{json, Value};
    use runtime_vocabulary::robot::bridge::invoke_command;

    /// `FAKE` is process-wide; tests that set it take turns.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn call(name: &str, args: Value) -> Result<String, String> {
        invoke_command("call_function", &json!({ "name": name, "args": args }))
    }

    #[test]
    fn a_fake_network_reaches_current_and_live_watches_and_masks_the_platform() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        crate::register_robot_controls();
        let got = Arc::new(Mutex::new(Vec::<Connectivity>::new()));
        let sink = got.clone();
        let (platform_feed, _tap) = tap_watch(Box::new(move |n| sink.lock().unwrap().push(n)));

        call("connectivity.set", json!({ "online": false })).unwrap();
        assert_eq!(crate::current(), Connectivity::OFFLINE);
        assert_eq!(got.lock().unwrap().as_slice(), &[Connectivity::OFFLINE]);

        // The same fake again is no change; a different one is.
        call("connectivity.set", json!({ "online": false })).unwrap();
        call("connectivity.set", json!({ "online": true, "transport": "cellular" })).unwrap();
        let cellular = Connectivity { online: true, transport: Transport::Cellular };
        assert_eq!(got.lock().unwrap().as_slice(), &[Connectivity::OFFLINE, cellular]);

        // The platform can't overwrite the fake.
        platform_feed(Connectivity::ASSUME_ONLINE);
        assert_eq!(got.lock().unwrap().len(), 2);

        call("connectivity.clear", json!({})).unwrap();
        let real = crate::current();
        let seen = got.lock().unwrap().clone();
        if real != cellular {
            assert_eq!(seen.last(), Some(&real), "clear hands the watch the real state");
        }
        platform_feed(Connectivity::OFFLINE);
        assert_eq!(got.lock().unwrap().last(), Some(&Connectivity::OFFLINE), "after clear the platform feeds through");
    }

    #[test]
    fn inconsistent_fakes_are_refused() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        crate::register_robot_controls();
        for (args, needle) in [
            (json!({ "online": false, "transport": "wifi" }), "no transport"),
            (json!({ "online": true, "transport": "none" }), "other than 'none'"),
            (json!({ "online": true, "transport": "carrier-pigeon" }), "unknown transport"),
        ] {
            let err = call("connectivity.set", args).unwrap_err();
            assert!(err.contains(needle), "{err}");
        }
        assert_eq!(fake(), None, "a refused fake sets nothing");
    }
}
