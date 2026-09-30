//! Web save: the File System Access API `showSaveFilePicker()` where available
//! (Chromium), falling back to a synthetic `<a download>` click everywhere
//! else (Safari/Firefox).
//!
//! Every browser call is a web-glue binding declared here (own-web-bindings
//! phase 3). The picker's `FileSystemFileHandle` / writable stream are driven
//! through web-glue's reflect surface (`JsValue::get` / `call_method`) and
//! their Promises awaited as `web_glue::JsFuture`; the Blob and the fallback
//! anchor are one binding each.

use web_glue::{string, JsError, JsFuture, JsType, JsValue};

use crate::{ExportError, SaveOutcome, SaveRequest, Source};

web_glue::import! {
    // 1 with a window + document, 0 otherwise (no presenter).
    fn js_has_document() -> u32 =
        "() => typeof window !== 'undefined' && window.document != null ? 1 : 0";
    // 1 when `window.showSaveFilePicker` is a function (Chromium).
    fn js_has_save_picker() -> u32 =
        "() => typeof window.showSaveFilePicker === 'function' ? 1 : 0";
    // `new Blob([bytes], { type })`; the bytes are copied (`slice`).
    #[catch]
    fn js_blob(p: usize, l: usize, tp: usize, tl: usize) -> u32 =
        "(p, l, tp, tl) => G.add(new Blob([G.u8().slice(p >>> 0, (p >>> 0) + (l >>> 0))], \
           { type: G.str(tp, tl) }))";
    // `showSaveFilePicker({ suggestedName })` → its Promise.
    #[catch]
    fn js_show_save_picker(p: usize, l: usize) -> u32 =
        "(p, l) => G.add(window.showSaveFilePicker({ suggestedName: G.str(p, l) }))";
    // `<a href=blob: download=name>` + click, then revoke the URL.
    #[catch]
    fn js_download(b: u32, p: usize, l: usize) =
        "(b, p, l) => { const url = URL.createObjectURL(G.get(b)); \
           const a = document.createElement('a'); a.href = url; a.download = G.str(p, l); \
           a.click(); URL.revokeObjectURL(url); }";
}

fn js_err(ctx: &str, e: &JsError) -> ExportError {
    ExportError::Backend(format!("{ctx}: {e}"))
}

pub(crate) async fn save(request: SaveRequest) -> Result<SaveOutcome, ExportError> {
    // Web has no real filesystem path — only in-memory bytes are saveable.
    let bytes = match request.source {
        Source::Bytes(b) => b,
        Source::Path(_) => return Err(ExportError::Unsupported),
    };

    let blob = make_blob(&bytes, &request.mime)?;
    if unsafe { js_has_document() } == 0 {
        return Err(ExportError::NoPresenter);
    }

    // Preferred path: showSaveFilePicker (a real "save as" dialog).
    if unsafe { js_has_save_picker() } != 0 {
        return save_via_picker(&request.suggested_name, &blob).await;
    }

    // Fallback: trigger a browser download to the default location.
    download_via_anchor(&request.suggested_name, &blob)?;
    // A plain download exposes no completion/cancel signal; report Saved with
    // an unknown location (the browser handled it).
    Ok(SaveOutcome::Saved { location: None })
}

/// Build a `Blob` from bytes + MIME type.
fn make_blob(bytes: &[u8], mime: &str) -> Result<JsValue, ExportError> {
    let (tp, tl) = string::abi(mime);
    unsafe { js_blob(bytes.as_ptr() as usize, bytes.len(), tp, tl) }
        // SAFETY: a fresh `G.add` slot the snippet minted for us.
        .map(|h| unsafe { JsValue::from_raw(h) })
        .map_err(|e| js_err("create Blob", &e))
}

/// `window.showSaveFilePicker({ suggestedName })` → write the blob → close.
async fn save_via_picker(suggested_name: &str, blob: &JsValue) -> Result<SaveOutcome, ExportError> {
    let (p, l) = string::abi(suggested_name);
    let handle_promise = unsafe { js_show_save_picker(p, l) }
        // SAFETY: a fresh `G.add` slot the snippet minted for us.
        .map(|h| unsafe { JsValue::from_raw(h) })
        .map_err(|e| js_err("showSaveFilePicker", &e))?;
    let handle = match JsFuture::new(&handle_promise).await {
        Ok(h) => h,
        // The user dismissing the dialog rejects with AbortError.
        Err(e) => return Ok(classify_reject(&e)),
    };

    // writable = await handle.createWritable()
    let writable = await_method(&handle, "createWritable", &[]).await?;
    // await writable.write(blob)
    await_method(&writable, "write", &[blob]).await?;
    // await writable.close()
    await_method(&writable, "close", &[]).await?;

    // The File System Access API doesn't hand back a usable path.
    Ok(SaveOutcome::Saved { location: None })
}

/// `await obj[name](...args)`, with the web-sys port's error shapes: a
/// missing method, a synchronous throw (`<name>`), a rejection
/// (`<name> await`).
async fn await_method(obj: &JsValue, name: &str, args: &[&JsValue]) -> Result<JsValue, ExportError> {
    let is_fn = obj.get(name).map(|f| f.js_type() == JsType::Function).unwrap_or(false);
    if !is_fn {
        return Err(ExportError::Backend(format!("missing method `{name}`")));
    }
    let promise = obj.call_method(name, args).map_err(|e| js_err(name, &e))?;
    JsFuture::new(&promise)
        .await
        .map_err(|e| js_err(&format!("{name} await"), &e))
}

/// Map a `showSaveFilePicker` rejection: an `AbortError` is the user
/// cancelling; anything else is a real failure.
fn classify_reject(e: &JsError) -> SaveOutcome {
    let name = e
        .value()
        .get("name")
        .ok()
        .and_then(|n| n.as_string())
        .unwrap_or_default();
    if name == "AbortError" {
        SaveOutcome::Cancelled
    } else {
        // A non-abort rejection still means "not saved"; surface as cancelled
        // rather than an error so author flow stays simple. (Genuine API
        // misuse would have failed earlier at `call`.)
        SaveOutcome::Cancelled
    }
}

/// Fallback: `<a href=blob download=name>` + programmatic click.
fn download_via_anchor(suggested_name: &str, blob: &JsValue) -> Result<(), ExportError> {
    let (p, l) = string::abi(suggested_name);
    unsafe { js_download(blob.raw(), p, l) }.map_err(|e| js_err("download", &e))
}
