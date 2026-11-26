//! The page's side of the app's service worker.
//!
//! The web build composes one worker, `idealyst-sw.js`, from the handler
//! scripts the app's dependencies contribute (`build_web::service_worker`)
//! and marks the page; the web boot then calls [`register`]. A browser lets
//! one worker control a scope, which is why there is one composed worker
//! rather than one per SDK.
//!
//! Registration is `navigator.serviceWorker.register(url, { updateViaCache:
//! "none" })`: the browser always re-fetches the worker script itself when
//! checking for an update instead of answering from the HTTP cache, so a
//! deploy (or a dev rebuild) that changes a handler takes effect on the
//! next check rather than after the cache expires. The composed worker
//! takes over from an old version immediately (its prelude calls
//! `skipWaiting` + `clients.claim`).

use crate::{JsError, JsFuture, JsValue};

crate::import! {
    fn js_supported() -> u32 =
        "() => (typeof navigator !== 'undefined' && navigator.serviceWorker != null) ? 1 : 0";
    #[catch]
    fn js_register(p: usize, l: usize) -> u32 =
        "(p, l) => G.add(navigator.serviceWorker.register(G.str(p, l), { updateViaCache: 'none' }))";
}

/// Whether this page can register a service worker: the API exists, which
/// needs a secure context (HTTPS, or `localhost`).
pub fn is_supported() -> bool {
    unsafe { js_supported() != 0 }
}

/// Register the service worker script at `url` (relative to the page) and
/// wait for the browser to accept it. Errors when the page cannot have one
/// ([`is_supported`]), when the script cannot be fetched, or when it throws
/// while being evaluated.
pub async fn register(url: &str) -> Result<(), JsError> {
    if !is_supported() {
        return Err(JsError::from(JsValue::from_str(
            "service workers are unavailable here (no navigator.serviceWorker — an insecure context?)",
        )));
    }
    let (p, l) = crate::string::abi(url);
    let promise = unsafe { JsValue::from_raw(js_register(p, l)?) };
    JsFuture::new(&promise).await.map(|_| ())
}
