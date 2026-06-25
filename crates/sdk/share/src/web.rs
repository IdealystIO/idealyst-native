//! Web share via the **Web Share API** — `navigator.share({ title, text, url })`.
//!
//! `navigator.share` requires a secure context (https / localhost) **and** a
//! transient user activation (it must run inside a click/tap handler), and it
//! isn't implemented in every browser. Where it's missing we return
//! [`ShareError::NotSupported`] rather than silently falling back to, say, a
//! clipboard copy — a fake "share" that didn't open the OS sheet would be worse
//! than an honest "unavailable" the caller can branch on.
//!
//! File sharing (`navigator.share({ files })`) needs `File` objects, which we'd
//! have to materialize from bytes — out of scope here (this crate deals in
//! `PathBuf` references, which have no meaning on the web sandbox). So the web
//! backend shares `title`/`text`/`url`; `files` are ignored on web (documented
//! in the crate `## Scope`).
//!
//! The typed `web_sys::Navigator::share` surface isn't stable, so we drive
//! `navigator.share` dynamically through `js_sys::Reflect` + `Function` +
//! `JsFuture`, the same posture `file-export`'s web backend takes for
//! `showSaveFilePicker`.

use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use crate::{ShareContent, ShareError, ShareOutcome};

pub(crate) async fn share(content: &ShareContent) -> Result<ShareOutcome, ShareError> {
    let window = web_sys::window().ok_or(ShareError::NotSupported)?;
    let navigator = window.navigator();

    // `navigator.share` is absent in unsupporting browsers / insecure contexts.
    let share_fn = js_sys::Reflect::get(&navigator, &JsValue::from_str("share"))
        .ok()
        .filter(|v| v.is_function())
        .ok_or(ShareError::NotSupported)?;
    let share_fn: js_sys::Function = share_fn.unchecked_into();

    // Build the ShareData object: { title?, text?, url? }. Web ignores our
    // `files` (PathBuf refs have no web meaning) — documented in `## Scope`.
    let data = js_sys::Object::new();
    if let Some(title) = &content.title {
        set(&data, "title", title);
    }
    if let Some(text) = &content.text {
        set(&data, "text", text);
    }
    if let Some(url) = &content.url {
        set(&data, "url", url);
    }

    let promise: js_sys::Promise = share_fn
        .call1(&navigator, &data)
        .map_err(|e| ShareError::Backend(format!("navigator.share: {e:?}")))?
        .unchecked_into();

    match JsFuture::from(promise).await {
        Ok(_) => Ok(ShareOutcome::Completed),
        // The spec rejects with an `AbortError` `DOMException` when the user
        // dismisses the share UI; anything else is a genuine failure.
        Err(e) => {
            if reject_name(&e) == "AbortError" {
                Ok(ShareOutcome::Dismissed)
            } else {
                Err(ShareError::Backend(format!("navigator.share rejected: {e:?}")))
            }
        }
    }
}

/// Set a string property on a JS object, ignoring the (infallible-for-plain-
/// object) result.
fn set(obj: &js_sys::Object, key: &str, value: &str) {
    let _ = js_sys::Reflect::set(obj, &JsValue::from_str(key), &JsValue::from_str(value));
}

/// The `name` of a rejected `DOMException` (e.g. `"AbortError"`), or empty.
fn reject_name(e: &JsValue) -> String {
    js_sys::Reflect::get(e, &JsValue::from_str("name"))
        .ok()
        .and_then(|n| n.as_string())
        .unwrap_or_default()
}
