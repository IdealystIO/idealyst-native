//! Web clipboard backend — `navigator.clipboard` via web-sys + JsFuture.
//!
//! `writeText` / `readText` return Promises; we await them with
//! `wasm_bindgen_futures::JsFuture`.
//!
//! Browser security note: `readText` (our [`text`]) requires a user
//! gesture (it must run in the call stack of a click/keypress) and may
//! prompt for the `clipboard-read` permission. A denial — or a call
//! without a gesture — rejects the Promise, which we surface as
//! [`ClipboardError::Backend`]. `writeText` is more permissive but is
//! still subject to the same gesture/permission model in some browsers.
//! This is a runtime concern, not a build-time manifest one. This backend
//! is the genuinely-runnable path for this SDK.

use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;

use crate::ClipboardError;

/// `window.navigator.clipboard`, or a `Backend` error if unavailable
/// (no window, or an insecure context where the Clipboard API is absent).
fn clipboard() -> Result<web_sys::Clipboard, ClipboardError> {
    let window = web_sys::window()
        .ok_or_else(|| ClipboardError::Backend("no window".into()))?;
    // `Navigator::clipboard()` is only present in secure contexts (https /
    // localhost). web-sys models it as always-present, so a missing API
    // would show up as a rejected Promise rather than here; this getter
    // can't itself fail in the binding, but we keep the indirection so the
    // call sites read uniformly.
    Ok(window.navigator().clipboard())
}

fn js_err(v: JsValue) -> ClipboardError {
    ClipboardError::Backend(format!("{v:?}"))
}

pub(crate) async fn set_text(text: &str) -> Result<(), ClipboardError> {
    let clip = clipboard()?;
    let promise = clip.write_text(text);
    JsFuture::from(promise).await.map_err(js_err)?;
    Ok(())
}

pub(crate) async fn text() -> Result<Option<String>, ClipboardError> {
    let clip = clipboard()?;
    let promise = clip.read_text();
    let value = JsFuture::from(promise).await.map_err(js_err)?;
    // `readText` resolves to a string; an empty clipboard resolves to "".
    // Treat the empty string as "no text" to match the native backends,
    // which report an absent string as `None`.
    match value.as_string() {
        Some(s) if s.is_empty() => Ok(None),
        Some(s) => Ok(Some(s)),
        None => Ok(None),
    }
}
