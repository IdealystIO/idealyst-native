//! Web save: the File System Access API `showSaveFilePicker()` where available
//! (Chromium), falling back to a synthetic `<a download>` click everywhere
//! else (Safari/Firefox).
//!
//! `showSaveFilePicker` and its `FileSystemFileHandle` / writable-stream types
//! aren't in `web-sys`'s stable surface, so we drive them dynamically through
//! `js_sys::Reflect` + `Function` + `JsFuture`. The fallback uses typed
//! `web-sys` (`Blob`, `Url`, `HtmlAnchorElement`).

use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Blob, BlobPropertyBag, HtmlAnchorElement, Url};

use crate::{ExportError, SaveOutcome, SaveRequest, Source};

fn js_err(ctx: &str, e: &JsValue) -> ExportError {
    ExportError::Backend(format!("{ctx}: {e:?}"))
}

pub(crate) async fn save(request: SaveRequest) -> Result<SaveOutcome, ExportError> {
    // Web has no real filesystem path — only in-memory bytes are saveable.
    let bytes = match request.source {
        Source::Bytes(b) => b,
        Source::Path(_) => return Err(ExportError::Unsupported),
    };

    let blob = make_blob(&bytes, &request.mime)?;
    let window = web_sys::window().ok_or(ExportError::NoPresenter)?;

    // Preferred path: showSaveFilePicker (a real "save as" dialog).
    let picker = js_sys::Reflect::get(&window, &JsValue::from_str("showSaveFilePicker"))
        .ok()
        .filter(|v| v.is_function());
    if let Some(picker) = picker {
        return save_via_picker(&window, picker.unchecked_into(), &request.suggested_name, &blob)
            .await;
    }

    // Fallback: trigger a browser download to the default location.
    download_via_anchor(&window, &request.suggested_name, &blob)?;
    // A plain download exposes no completion/cancel signal; report Saved with
    // an unknown location (the browser handled it).
    Ok(SaveOutcome::Saved { location: None })
}

/// Build a `Blob` from bytes + MIME type.
fn make_blob(bytes: &[u8], mime: &str) -> Result<Blob, ExportError> {
    let array = js_sys::Uint8Array::from(bytes);
    let parts = js_sys::Array::new();
    parts.push(&array);
    let opts = BlobPropertyBag::new();
    opts.set_type(mime);
    Blob::new_with_u8_array_sequence_and_options(&parts, &opts)
        .map_err(|e| js_err("create Blob", &e))
}

/// `window.showSaveFilePicker({ suggestedName })` → write the blob → close.
async fn save_via_picker(
    _window: &web_sys::Window,
    picker: js_sys::Function,
    suggested_name: &str,
    blob: &Blob,
) -> Result<SaveOutcome, ExportError> {
    // Options: { suggestedName: "<name>" }.
    let opts = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &opts,
        &JsValue::from_str("suggestedName"),
        &JsValue::from_str(suggested_name),
    );

    let handle_promise: js_sys::Promise = picker
        .call1(&JsValue::UNDEFINED, &opts)
        .map_err(|e| js_err("showSaveFilePicker", &e))?
        .unchecked_into();
    let handle = match JsFuture::from(handle_promise).await {
        Ok(h) => h,
        // The user dismissing the dialog rejects with AbortError.
        Err(e) => return Ok(classify_reject(e)),
    };

    // writable = await handle.createWritable()
    let create = reflect_fn(&handle, "createWritable")?;
    let create_promise: js_sys::Promise = create
        .call0(&handle)
        .map_err(|e| js_err("createWritable", &e))?
        .unchecked_into();
    let writable = JsFuture::from(create_promise)
        .await
        .map_err(|e| js_err("createWritable await", &e))?;

    // await writable.write(blob)
    let write = reflect_fn(&writable, "write")?;
    let write_promise: js_sys::Promise = write
        .call1(&writable, blob)
        .map_err(|e| js_err("write", &e))?
        .unchecked_into();
    JsFuture::from(write_promise)
        .await
        .map_err(|e| js_err("write await", &e))?;

    // await writable.close()
    let close = reflect_fn(&writable, "close")?;
    let close_promise: js_sys::Promise = close
        .call0(&writable)
        .map_err(|e| js_err("close", &e))?
        .unchecked_into();
    JsFuture::from(close_promise)
        .await
        .map_err(|e| js_err("close await", &e))?;

    // The File System Access API doesn't hand back a usable path.
    Ok(SaveOutcome::Saved { location: None })
}

/// Map a `showSaveFilePicker` rejection: an `AbortError` is the user
/// cancelling; anything else is a real failure.
fn classify_reject(e: JsValue) -> SaveOutcome {
    let name = js_sys::Reflect::get(&e, &JsValue::from_str("name"))
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

/// Read a JS method off an object as a callable `Function`.
fn reflect_fn(obj: &JsValue, name: &str) -> Result<js_sys::Function, ExportError> {
    js_sys::Reflect::get(obj, &JsValue::from_str(name))
        .ok()
        .filter(|v| v.is_function())
        .map(|v| v.unchecked_into())
        .ok_or_else(|| ExportError::Backend(format!("missing method `{name}`")))
}

/// Fallback: `<a href=blob download=name>` + programmatic click.
fn download_via_anchor(
    window: &web_sys::Window,
    suggested_name: &str,
    blob: &Blob,
) -> Result<(), ExportError> {
    let document = window.document().ok_or(ExportError::NoPresenter)?;
    let url = Url::create_object_url_with_blob(blob).map_err(|e| js_err("object URL", &e))?;
    let anchor: HtmlAnchorElement = document
        .create_element("a")
        .map_err(|e| js_err("create anchor", &e))?
        .unchecked_into();
    anchor.set_href(&url);
    anchor.set_download(suggested_name);
    anchor.click();
    let _ = Url::revoke_object_url(&url);
    Ok(())
}
