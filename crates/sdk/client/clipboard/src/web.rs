//! Web clipboard backend — `navigator.clipboard` through web-glue bindings
//! declared here (own-web-bindings phase 3); the Promises are awaited with
//! `web_glue::JsFuture`.
//!
//! Browser security note: `readText` (our [`text`]) requires a user
//! gesture (it must run in the call stack of a click/keypress) and may
//! prompt for the `clipboard-read` permission. A denial — or a call
//! without a gesture — rejects the Promise, which we surface as
//! [`ClipboardError::Backend`]. `writeText` is more permissive but is
//! still subject to the same gesture/permission model in some browsers.
//! This is a runtime concern, not a build-time manifest one. This backend
//! is the genuinely-runnable path for this SDK.

use web_glue::{string, JsError, JsFuture, JsValue};

use crate::ClipboardError;

web_glue::import! {
    // `navigator.clipboard`, or 0 when it is absent. It IS absent outside a
    // secure context (plain http on a non-localhost origin) and in browsers
    // without the async Clipboard API.
    fn js_clipboard() -> u32 =
        "() => { if (typeof window === 'undefined') return 0; \
           const c = window.navigator.clipboard; return c == null ? 0 : G.add(c); }";
    #[catch]
    fn js_write_text(c: u32, p: usize, l: usize) -> u32 =
        "(c, p, l) => G.add(G.get(c).writeText(G.str(p, l)))";
    #[catch]
    fn js_read_text(c: u32) -> u32 = "(c) => G.add(G.get(c).readText())";
}

/// `window.navigator.clipboard`, or a `Backend` error if unavailable
/// (no window, or an insecure context where the Clipboard API is absent).
///
/// The web-sys port assumed the getter could not miss ("web-sys models it
/// as always-present") and called `writeText` on `undefined`, which threw
/// a TypeError through the wasm frames instead of returning this error.
/// Regression: `tests/web_clipboard.rs`.
fn clipboard() -> Result<JsValue, ClipboardError> {
    match unsafe { js_clipboard() } {
        0 => Err(ClipboardError::Backend(
            "navigator.clipboard is unavailable (no window, or not a secure context)".into(),
        )),
        // SAFETY: a fresh `G.add` slot the snippet minted for us.
        h => Ok(unsafe { JsValue::from_raw(h) }),
    }
}

fn js_err(e: JsError) -> ClipboardError {
    ClipboardError::Backend(e.message())
}

pub(crate) async fn set_text(text: &str) -> Result<(), ClipboardError> {
    let promise = {
        let clip = clipboard()?;
        let (p, l) = string::abi(text);
        // SAFETY: `from_raw` adopts the promise slot the snippet minted.
        unsafe { js_write_text(clip.raw(), p, l) }.map(|h| unsafe { JsValue::from_raw(h) })
    }
    .map_err(js_err)?;
    JsFuture::new(&promise).await.map_err(js_err)?;
    Ok(())
}

pub(crate) async fn text() -> Result<Option<String>, ClipboardError> {
    let promise = {
        let clip = clipboard()?;
        // SAFETY: `from_raw` adopts the promise slot the snippet minted.
        unsafe { js_read_text(clip.raw()) }.map(|h| unsafe { JsValue::from_raw(h) })
    }
    .map_err(js_err)?;
    let value = JsFuture::new(&promise).await.map_err(js_err)?;
    // `readText` resolves to a string; an empty clipboard resolves to "".
    // Treat the empty string as "no text" to match the native backends,
    // which report an absent string as `None`.
    match value.as_string() {
        Some(s) if s.is_empty() => Ok(None),
        Some(s) => Ok(Some(s)),
        None => Ok(None),
    }
}
