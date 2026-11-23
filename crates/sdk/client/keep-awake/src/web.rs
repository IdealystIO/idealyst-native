//! Web: the Screen Wake Lock API.
//!
//! The browser drops a wake lock when the page is hidden; the portable policy
//! releases then too and asks again when the page is shown. A request can be
//! refused (low battery, a permissions policy, before the first gesture in
//! Safari): that is logged, and the policy asks again at the next change in
//! the app's situation.

use std::cell::{Cell, RefCell};

use runtime_shared::logging::{log, LogLevel};
use web_glue::{spawn_local, JsFuture, JsValue};

web_glue::import! {
    // 1 when the page may ask for a wake lock (absent in an insecure
    // context and in browsers without the API).
    fn js_supported() -> u32 =
        "() => typeof window !== 'undefined' && window.navigator.wakeLock != null ? 1 : 0";
    // `navigator.wakeLock.request('screen')` → its Promise.
    #[catch]
    fn js_request() -> u32 = "() => G.add(window.navigator.wakeLock.request('screen'))";
}

runtime_tls::thread_local! {
    static WANTED: Cell<bool> = const { Cell::new(false) };
    static REQUESTING: Cell<bool> = const { Cell::new(false) };
    /// The `WakeLockSentinel`.
    static HELD: RefCell<Option<JsValue>> = const { RefCell::new(None) };
}

pub(crate) fn set(on: bool) {
    WANTED.with(|w| w.set(on));
    if on {
        request();
    } else if let Some(sentinel) = HELD.with(|h| h.borrow_mut().take()) {
        release(&sentinel);
    }
}

fn live() -> bool {
    HELD.with(|h| {
        h.borrow().as_ref().is_some_and(|s| !s.get("released").is_ok_and(|r| r.truthy()))
    })
}

fn request() {
    if !is_supported() || live() || REQUESTING.with(|r| r.replace(true)) {
        return;
    }
    spawn_local(async {
        let asked = match unsafe { js_request() } {
            // SAFETY: a fresh `G.add` slot the snippet minted for us.
            Ok(h) => JsFuture::new(&unsafe { JsValue::from_raw(h) }).await,
            Err(e) => Err(e),
        };
        REQUESTING.with(|r| r.set(false));
        match asked {
            // Released while the request was out.
            Ok(sentinel) if !WANTED.with(Cell::get) => release(&sentinel),
            Ok(sentinel) => HELD.with(|h| *h.borrow_mut() = Some(sentinel)),
            Err(e) => log(LogLevel::Warn, &format!("keep-awake: the browser refused a wake lock: {}", e.message())),
        }
    });
}

fn release(sentinel: &JsValue) {
    let Ok(done) = sentinel.call_method("release", &[]) else { return };
    // Awaited so a rejection is not reported as unhandled.
    spawn_local(async move {
        let _ = JsFuture::new(&done).await;
    });
}

pub(crate) fn is_supported() -> bool {
    unsafe { js_supported() != 0 }
}
