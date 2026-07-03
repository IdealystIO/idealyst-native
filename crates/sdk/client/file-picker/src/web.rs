//! Web open: the File System Access API `showOpenFilePicker()` where available
//! (Chromium), falling back to a hidden `<input type=file>` everywhere else
//! (Safari/Firefox).
//!
//! `showOpenFilePicker` isn't in `web-sys`'s stable surface, so we drive it
//! dynamically through `js_sys::Reflect` + `Function` + `JsFuture`. Either path
//! yields `File` (`Blob`) objects; there is no filesystem path on the web, so
//! [`PickedFile::path`](crate::PickedFile::path) is `None` and reads stream over
//! the `Blob`'s `ReadableStream` — a multi-GB pick is consumed chunk-by-chunk,
//! never buffered whole.

use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::Window;

use crate::{PickError, PickKind, PickRequest};

fn js_err(ctx: &str, e: &JsValue) -> PickError {
    PickError::Backend(format!("{ctx}: {e:?}"))
}

/// A file the user picked on the web: the `File` handle plus cached metadata.
pub(crate) struct PickedFile {
    file: web_sys::File,
    name: String,
    mime: String,
    size: Option<u64>,
}

impl PickedFile {
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
    pub(crate) fn mime(&self) -> &str {
        &self.mime
    }
    pub(crate) fn size(&self) -> Option<u64> {
        self.size
    }
    pub(crate) fn path(&self) -> Option<&std::path::Path> {
        // No filesystem on the web.
        None
    }
    pub(crate) async fn open(&self) -> Result<FileStream, PickError> {
        // File derefs to Blob; `stream()` yields a ReadableStream of bytes.
        let stream = self.file.stream();
        let reader: web_sys::ReadableStreamDefaultReader = stream.get_reader().unchecked_into();
        Ok(FileStream {
            reader,
            done: false,
        })
    }
}

fn picked_from_file(file: web_sys::File) -> PickedFile {
    let name = file.name();
    let mime = file.type_();
    let size = Some(file.size() as u64);
    PickedFile {
        file,
        name,
        mime,
        size,
    }
}

/// Reads a picked `File` via its `Blob` `ReadableStream`, a chunk per `chunk()`.
pub(crate) struct FileStream {
    reader: web_sys::ReadableStreamDefaultReader,
    done: bool,
}

impl FileStream {
    pub(crate) async fn chunk(&mut self) -> Result<Option<Vec<u8>>, PickError> {
        if self.done {
            return Ok(None);
        }
        let result = JsFuture::from(self.reader.read())
            .await
            .map_err(|e| js_err("read", &e))?;
        // `{ value: Uint8Array, done: bool }`
        let done = js_sys::Reflect::get(&result, &JsValue::from_str("done"))
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        if done {
            self.done = true;
            return Ok(None);
        }
        let value = js_sys::Reflect::get(&result, &JsValue::from_str("value"))
            .map_err(|e| js_err("read value", &e))?;
        let bytes = value.unchecked_into::<js_sys::Uint8Array>().to_vec();
        Ok(Some(bytes))
    }
}

impl Drop for FileStream {
    fn drop(&mut self) {
        // Release the stream lock so the underlying `Blob` isn't left locked.
        // `cancel()` returns a Promise we intentionally drop (fire-and-forget).
        let _ = self.reader.cancel();
    }
}

pub(crate) async fn pick(request: &PickRequest) -> Result<Option<Vec<PickedFile>>, PickError> {
    let window = web_sys::window().ok_or(PickError::NoPresenter)?;
    let accept = accept_list(request);
    let multiple = request.allow_multiple;

    // Preferred path: the File System Access API.
    let picker = js_sys::Reflect::get(&window, &JsValue::from_str("showOpenFilePicker"))
        .ok()
        .filter(|v| v.is_function());
    if let Some(func) = picker {
        match pick_via_fsa(func.unchecked_into(), &accept, multiple).await {
            // Got a result (files or a clean cancel) — done.
            Ok(outcome) => return Ok(outcome),
            // FSA present but unusable here (e.g. cross-origin iframe) — fall
            // through to the input fallback.
            Err(()) => {}
        }
    }

    pick_via_input(&window, &accept, multiple).await
}

/// `showOpenFilePicker({ multiple, types })`. `Ok(Some(..))` = files,
/// `Ok(None)` = user cancelled, `Err(())` = couldn't use FSA → fall back.
async fn pick_via_fsa(
    func: js_sys::Function,
    accept: &[String],
    multiple: bool,
) -> Result<Option<Vec<PickedFile>>, ()> {
    let opts = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &opts,
        &JsValue::from_str("multiple"),
        &JsValue::from_bool(multiple),
    );
    if let Some(types) = fsa_types(accept) {
        let _ = js_sys::Reflect::set(&opts, &JsValue::from_str("types"), &types);
    }

    let promise: js_sys::Promise = func.call1(&JsValue::UNDEFINED, &opts).map_err(|_| ())?.unchecked_into();
    let handles = match JsFuture::from(promise).await {
        Ok(h) => h,
        Err(e) => {
            // The user dismissing the dialog rejects with AbortError.
            return if is_abort(&e) { Ok(None) } else { Err(()) };
        }
    };

    let arr = js_sys::Array::from(&handles);
    let mut out = Vec::new();
    for handle in arr.iter() {
        // file = await handle.getFile()
        let get = reflect_fn(&handle, "getFile").map_err(|_| ())?;
        let fp: js_sys::Promise = get.call0(&handle).map_err(|_| ())?.unchecked_into();
        let file: web_sys::File = JsFuture::from(fp).await.map_err(|_| ())?.unchecked_into();
        out.push(picked_from_file(file));
    }
    Ok(Some(out))
}

/// Fallback: a hidden `<input type=file>` whose `change`/`cancel` events we
/// await. (Very old browsers without a `cancel` event leave a cancel
/// undetected; `change` + empty selection still resolves as cancelled.)
async fn pick_via_input(
    window: &Window,
    accept: &[String],
    multiple: bool,
) -> Result<Option<Vec<PickedFile>>, PickError> {
    let document = window.document().ok_or(PickError::NoPresenter)?;
    let input: web_sys::HtmlInputElement = document
        .create_element("input")
        .map_err(|e| js_err("create input", &e))?
        .unchecked_into();
    input.set_type("file");
    if multiple {
        input.set_multiple(true);
    }
    if !accept.is_empty() {
        input.set_accept(&accept.join(","));
    }
    let _ = input.set_attribute("style", "display:none");
    let body = document.body().ok_or(PickError::NoPresenter)?;
    let _ = body.append_child(&input);

    // Resolve the promise when the user picks (`change`) or dismisses
    // (`cancel`). The closures are kept alive in locals (no `mem::forget`) and
    // drop when this fn returns, after the await.
    let input_for_listeners = input.clone();
    let mut on_change: Option<Closure<dyn FnMut()>> = None;
    let mut on_cancel: Option<Closure<dyn FnMut()>> = None;
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        let r1 = resolve.clone();
        let change = Closure::wrap(Box::new(move || {
            let _ = r1.call0(&JsValue::UNDEFINED);
        }) as Box<dyn FnMut()>);
        let r2 = resolve;
        let cancel = Closure::wrap(Box::new(move || {
            let _ = r2.call0(&JsValue::UNDEFINED);
        }) as Box<dyn FnMut()>);
        let _ = input_for_listeners
            .add_event_listener_with_callback("change", change.as_ref().unchecked_ref());
        let _ = input_for_listeners
            .add_event_listener_with_callback("cancel", cancel.as_ref().unchecked_ref());
        on_change = Some(change);
        on_cancel = Some(cancel);
    });

    input.click();
    let _ = JsFuture::from(promise).await;
    let _ = body.remove_child(&input);

    let mut out = Vec::new();
    if let Some(list) = input.files() {
        for i in 0..list.length() {
            if let Some(file) = list.item(i) {
                out.push(picked_from_file(file));
            }
        }
    }
    // `on_change` / `on_cancel` drop here, detaching the listeners.
    if out.is_empty() {
        Ok(None)
    } else {
        Ok(Some(out))
    }
}

/// The MIME/`accept` strings for the request (documents → the filters as given;
/// media → image/video wildcards).
fn accept_list(request: &PickRequest) -> Vec<String> {
    match &request.kind {
        PickKind::Documents(m) => m.clone(),
        PickKind::Media(k) => crate::mime::media_mimes(*k)
            .iter()
            .map(|s| s.to_string())
            .collect(),
    }
}

/// Build the `types` option for `showOpenFilePicker`, or `None` (any file).
fn fsa_types(accept: &[String]) -> Option<JsValue> {
    if accept.is_empty() {
        return None;
    }
    let accept_obj = js_sys::Object::new();
    for mime in accept {
        let _ = js_sys::Reflect::set(
            &accept_obj,
            &JsValue::from_str(mime),
            &js_sys::Array::new(),
        );
    }
    let entry = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &entry,
        &JsValue::from_str("description"),
        &JsValue::from_str("Files"),
    );
    let _ = js_sys::Reflect::set(&entry, &JsValue::from_str("accept"), &accept_obj);
    let types = js_sys::Array::new();
    types.push(&entry);
    Some(types.into())
}

/// Read a JS method off an object as a callable `Function`.
fn reflect_fn(obj: &JsValue, name: &str) -> Result<js_sys::Function, ()> {
    js_sys::Reflect::get(obj, &JsValue::from_str(name))
        .ok()
        .filter(|v| v.is_function())
        .map(|v| v.unchecked_into())
        .ok_or(())
}

/// Is this rejection an `AbortError` (the user cancelling)?
fn is_abort(e: &JsValue) -> bool {
    js_sys::Reflect::get(e, &JsValue::from_str("name"))
        .ok()
        .and_then(|n| n.as_string())
        .map(|n| n == "AbortError")
        .unwrap_or(false)
}
