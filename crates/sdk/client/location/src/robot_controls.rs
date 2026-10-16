//! Robot test controls: a fake position for E2E tests. Compiled only into
//! robot builds (`runtime_vocabulary::robot_select!` in `lib.rs`).
//!
//! While a fake is set, [`crate::current`] resolves it at once (no
//! permission prompt: the prompt is the OS's, and would stall the test) and
//! every live [`crate::watch`] receives it; the platform's own updates are
//! dropped until `location.clear`, so a real fix can't overwrite the fake
//! mid-test.

use std::cell::RefCell;
use std::sync::{Arc, Mutex, Weak};

use runtime_core::robot_fn;
use runtime_vocabulary::robot_fn::register as register_fn;

use crate::{BoxedCallback, Position};

/// Read from the platform's callback thread (a real update checks it), so a
/// `Mutex`, not a thread-local.
static FAKE: Mutex<Option<Position>> = Mutex::new(None);

type Shared = Arc<Mutex<BoxedCallback>>;

thread_local! {
    /// Every watch installed on this (the UI) thread, so a fake reaches the
    /// ones alive when it is set. Weak: the watch owns its callback.
    static WATCHES: RefCell<Vec<Weak<Mutex<BoxedCallback>>>> = const { RefCell::new(Vec::new()) };
}

/// Keeps a watch's callback reachable by a fake for the watch's lifetime,
/// even on a backend that drops the callback it is given.
pub(crate) struct WatchTap {
    _callback: Shared,
}

pub(crate) fn fake() -> Option<Position> {
    *FAKE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Wrap a watch's callback: the platform's updates pass through only while
/// no fake is set, and the callback joins the fake's fan-out.
pub(crate) fn tap_watch(callback: BoxedCallback) -> (BoxedCallback, WatchTap) {
    let shared: Shared = Arc::new(Mutex::new(callback));
    WATCHES.with(|w| w.borrow_mut().push(Arc::downgrade(&shared)));
    let real = shared.clone();
    // Checked under the callback's lock, which a fake's delivery also
    // takes: a real fix racing `location.set` either lands before the fake
    // or is dropped, never after it.
    let filtered: BoxedCallback = Box::new(move |pos| {
        let callback = real.lock().unwrap_or_else(|e| e.into_inner());
        if fake().is_none() {
            callback(pos);
        }
    });
    (filtered, WatchTap { _callback: shared })
}

/// Set a fake device position: `current()` resolves it and every live
/// `watch` receives it now; the platform's own updates are ignored until
/// `location.clear`. `accuracy_m` defaults to 5 m.
#[robot_fn(name = "location.set")]
fn set_position(
    latitude: f64,
    longitude: f64,
    accuracy_m: Option<f64>,
    altitude: Option<f64>,
    heading: Option<f64>,
    speed: Option<f64>,
) -> Result<(), String> {
    if !(-90.0..=90.0).contains(&latitude) || !(-180.0..=180.0).contains(&longitude) {
        return Err(format!("({latitude}, {longitude}) is not a WGS-84 position"));
    }
    let pos = Position {
        latitude,
        longitude,
        accuracy_m: accuracy_m.unwrap_or(5.0),
        altitude,
        heading,
        speed,
        timestamp_ms: runtime_core::time::epoch_millis() as f64,
    };
    *FAKE.lock().unwrap_or_else(|e| e.into_inner()) = Some(pos);
    let live: Vec<Shared> = WATCHES.with(|w| {
        let mut w = w.borrow_mut();
        w.retain(|cb| cb.strong_count() > 0);
        w.iter().filter_map(Weak::upgrade).collect()
    });
    for cb in live {
        (cb.lock().unwrap_or_else(|e| e.into_inner()))(pos);
    }
    Ok(())
}

/// Stop faking: `current()` and `watch` read the platform again.
#[robot_fn(name = "location.clear")]
fn clear() {
    *FAKE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

pub(crate) fn register() {
    register_fn(set_position::robot());
    register_fn(clear::robot());
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_core::__serde_json::{json, Value};
    use runtime_vocabulary::robot::bridge::invoke_command;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// `FAKE` is process-wide; tests that set it take turns.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn call(name: &str, args: Value) -> Result<String, String> {
        invoke_command("call_function", &json!({ "name": name, "args": args }))
    }

    fn poll_now<F: std::future::Future>(f: F) -> F::Output {
        let mut f = std::pin::pin!(f);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        match f.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(v) => v,
            std::task::Poll::Pending => panic!("a faked `current()` resolves without waiting"),
        }
    }

    #[test]
    fn a_fake_position_reaches_current_and_live_watches_and_masks_the_platform() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        // A mounted app has the wall clock; a bare test installs it.
        runtime_core::time::install_wall_clock_source(Box::new(runtime_core::time::SystemWallClockSource));
        crate::register_robot_controls();
        let got = Arc::new(Mutex::new(Vec::<Position>::new()));
        let sink = got.clone();
        let (platform_feed, _tap) = tap_watch(Box::new(move |p| sink.lock().unwrap().push(p)));

        call("location.set", json!({ "latitude": 52.52, "longitude": 13.405, "accuracy_m": 3.0 })).unwrap();
        let fake = poll_now(crate::current()).expect("current() resolves the fake");
        assert_eq!((fake.latitude, fake.longitude, fake.accuracy_m), (52.52, 13.405, 3.0));
        assert!(fake.timestamp_ms > 0.0, "stamped with the wall clock");
        assert_eq!(got.lock().unwrap().as_slice(), &[fake], "the live watch received it");

        // A real fix arriving meanwhile is dropped.
        let real = Position { latitude: 1.0, ..fake };
        platform_feed(real);
        assert_eq!(got.lock().unwrap().len(), 1, "the platform can't overwrite the fake");

        call("location.clear", json!({})).unwrap();
        platform_feed(real);
        assert_eq!(got.lock().unwrap().last(), Some(&real), "after clear the platform feeds through");
    }

    #[test]
    fn a_dropped_watch_gets_no_fake_and_a_bad_position_is_refused() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        crate::register_robot_controls();
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let watch = crate::watch(move |_| {
            c.fetch_add(1, Ordering::SeqCst);
        });
        call("location.set", json!({ "latitude": 10.0, "longitude": 10.0 })).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        drop(watch);
        call("location.set", json!({ "latitude": 11.0, "longitude": 11.0 })).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1, "a dropped watch is gone");

        let err = call("location.set", json!({ "latitude": 91.0, "longitude": 0.0 })).unwrap_err();
        assert!(err.contains("not a WGS-84 position"), "{err}");
        call("location.clear", json!({})).unwrap();
    }
}
