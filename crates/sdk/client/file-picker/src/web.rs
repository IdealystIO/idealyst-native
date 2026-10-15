//! Web open: the File System Access API `showOpenFilePicker()` where available
//! (Chromium), falling back to a hidden `<input type=file>` everywhere else
//! (Safari/Firefox, and cross-origin iframes, where Chromium refuses the
//! FSA picker).
//!
//! One `pick()` shows at most ONE dialog: the `<input>` fallback runs only when
//! the FSA picker provably never showed (see `crate::fsa_rejection` for which
//! rejections those are, per the spec). A rejection after the dialog — the
//! user's choice refused by the embedder, a handle's `getFile()` failing — is
//! logged and surfaced as an error, never answered with a second dialog.
//!
//! Every browser call is a web-glue binding (docs/proposals/own-web-bindings.md).
//! Either path yields `File` (`Blob`) objects — `web_glue::dom::File`, the same
//! handle backend-web puts in a dropped file's `DroppedFile::source`. There is
//! no filesystem path on the web, so [`PickedFile::path`](crate::PickedFile::path)
//! is `None` and reads stream over the `Blob`'s `ReadableStream` — a multi-GB
//! pick is consumed chunk-by-chunk, never buffered whole.

use web_glue::dom::File;
use web_glue::js::{Array, Uint8Array};
use web_glue::{string, JsCast, JsError, JsFuture, JsValue};

use runtime_shared::log_warn;

use crate::fsa_rejection::{classify_fsa_rejection, FsaRejection, FsaStage};
use crate::{PickError, PickKind, PickRequest};

web_glue::import! {
    // 1 when `window.showOpenFilePicker` (File System Access) is callable
    // AND this document is not a cross-origin subframe. Chromium (and Electron)
    // refuse the FSA picker in a cross-origin iframe — Electron since
    // Chromium 150 with `NotAllowedError`, a name that is otherwise a
    // post-dialog refusal (`crate::fsa_rejection`) and so must not trigger the
    // fallback. Detecting the frame up front sends it straight to the
    // `<input>`, which works there. Reading `top.location.href` throws exactly
    // when `top` is cross-origin.
    fn js_has_fsa() -> u32 =
        "() => { if (typeof window === 'undefined' || typeof window.showOpenFilePicker !== 'function') return 0; \
           try { if (window.top !== window.self) void window.top.location.href; } catch (_) { return 0; } \
           return 1; }";
    // `showOpenFilePicker({ multiple, types })` → a Promise of the picked
    // `File`s (each handle's `getFile()`). `accept` is the MIME list joined by
    // '\n'; empty means any file (no `types`).
    //
    // A rejection is re-thrown as an `Error` carrying the original `name` and
    // message plus `stage` — 'picker' (showOpenFilePicker rejected) or 'read'
    // (a returned handle's getFile() rejected, with `file` = the handle's
    // name) — so Rust can tell pre- from post-dialog (`crate::fsa_rejection`).
    //
    // A getFile() `NotAllowedError` means the handle has no read grant yet
    // (spec: getFile rejects unless read permission is "granted"). The user
    // already chose the file, so ask for the grant once through the spec's
    // `requestPermission({ mode: 'read' })` — a permission prompt at most,
    // never a file dialog — and read again; if that is refused or throws, the
    // ORIGINAL error is reported.
    #[catch]
    fn js_fsa_pick(ap: usize, al: usize, multiple: u32) -> u32 =
        "(ap, al, m) => { const acc = G.str(ap, al); const o = { multiple: m !== 0 }; \
           if (acc.length) { const a = {}; for (const t of acc.split('\\n')) a[t] = []; \
             o.types = [{ description: 'Files', accept: a }]; } \
           const tag = (stage, e, file) => { \
             const x = new Error(e != null && e.message != null ? e.message : String(e)); \
             x.name = (e != null && e.name) || 'Error'; x.stage = stage; \
             if (file != null) x.file = file; return x; }; \
           const read = async (h) => { \
             try { return await h.getFile(); } catch (e) { \
               if (e != null && e.name === 'NotAllowedError' && typeof h.requestPermission === 'function') { \
                 let st; try { st = await h.requestPermission({ mode: 'read' }); } catch (_) {} \
                 if (st === 'granted') { try { return await h.getFile(); } catch (e2) { throw tag('read', e2, h.name); } } } \
               throw tag('read', e, h.name); } }; \
           return G.add(window.showOpenFilePicker(o).then( \
             (hs) => Promise.all(Array.from(hs, read)), \
             (e) => { throw tag('picker', e); })); }";
    // The hidden `<input type=file>` fallback, whole: build it, append it,
    // `click()` it (synchronously, inside the caller's gesture), and resolve
    // the returned Promise with the selected `File`s on `change` — or an
    // empty array on `cancel` — after removing the input again. (Very old
    // browsers without a `cancel` event leave a cancel undetected; `change`
    // with an empty selection still resolves as cancelled.)
    #[catch]
    fn js_input_pick(ap: usize, al: usize, multiple: u32) -> u32 =
        "(ap, al, m) => { const acc = G.str(ap, al); const i = document.createElement('input'); \
           i.type = 'file'; if (m !== 0) i.multiple = true; if (acc.length) i.accept = acc; \
           i.setAttribute('style', 'display:none'); \
           if (document.body == null) throw new Error('no document body'); \
           document.body.appendChild(i); \
           const p = new Promise((res) => { \
             const done = () => { i.remove(); res(Array.from(i.files || [])); }; \
             i.addEventListener('change', done, { once: true }); \
             i.addEventListener('cancel', done, { once: true }); }); \
           i.click(); return G.add(p); }";
    // `file.stream().getReader()`.
    #[catch]
    fn js_reader(f: u32) -> u32 = "(f) => G.add(G.get(f).stream().getReader())";
    // `reader.read()` → a Promise of `{ value: Uint8Array, done }`.
    #[catch]
    fn js_read(r: u32) -> u32 = "(r) => G.add(G.get(r).read())";
    // `reader.cancel()` — releases the stream lock (fire-and-forget).
    fn js_cancel(r: u32) = "(r) => { const p = G.get(r).cancel(); if (p) p.catch(() => {}); }";
}

fn js_err(ctx: &str, e: &JsError) -> PickError {
    PickError::Backend(format!("{ctx}: {}", e.message()))
}

/// A file the user picked on the web: the `File` handle plus cached metadata.
pub(crate) struct PickedFile {
    file: File,
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
        // A `File` is a `Blob`; `stream()` yields a ReadableStream of bytes.
        // SAFETY: a live file handle; the result is a fresh reader handle.
        let reader = unsafe { js_reader(self.file.as_js().raw()) }
            .map(|r| unsafe { JsValue::from_raw(r) })
            .map_err(|e| js_err("stream", &e))?;
        Ok(FileStream { reader, done: false })
    }
}

fn picked_from_file(file: File) -> PickedFile {
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

/// Convert a dropped OS file into a `PickedFile`. On web a dropped file has no
/// filesystem path, so backend-web stashes the `web_glue::dom::File` in
/// `DroppedFile::source`; we downcast it and reuse the picker's `File` reader.
#[cfg(feature = "drop")]
pub(crate) fn picked_from_dropped(f: &runtime_shared::DroppedFile) -> Option<PickedFile> {
    let file = f.source.as_ref()?.downcast_ref::<File>()?.clone();
    Some(picked_from_file(file))
}

/// Reads a picked `File` via its `Blob` `ReadableStream`, a chunk per `chunk()`.
pub(crate) struct FileStream {
    /// The `ReadableStreamDefaultReader`.
    reader: JsValue,
    done: bool,
}

impl FileStream {
    pub(crate) async fn chunk(&mut self) -> Result<Option<Vec<u8>>, PickError> {
        if self.done {
            return Ok(None);
        }
        // SAFETY: a live reader handle; the result is a fresh Promise handle.
        let promise = unsafe { js_read(self.reader.raw()) }
            .map(|p| unsafe { JsValue::from_raw(p) })
            .map_err(|e| js_err("read", &e))?;
        let result = JsFuture::new(&promise).await.map_err(|e| js_err("read", &e))?;
        // `{ value: Uint8Array, done: bool }`
        let done = result.get("done").ok().and_then(|v| v.as_bool()).unwrap_or(true);
        if done {
            self.done = true;
            return Ok(None);
        }
        let value = result.get("value").map_err(|e| js_err("read value", &e))?;
        Ok(Some(value.unchecked_into::<Uint8Array>().to_vec()))
    }
}

impl Drop for FileStream {
    fn drop(&mut self) {
        // Release the stream lock so the underlying `Blob` isn't left locked.
        // SAFETY: a live reader handle.
        unsafe { js_cancel(self.reader.raw()) };
    }
}

pub(crate) async fn pick(request: &PickRequest) -> Result<Option<Vec<PickedFile>>, PickError> {
    let accept = accept_list(request).join("\n");
    let multiple = request.allow_multiple;

    // Preferred path: the File System Access API.
    // SAFETY: no handles involved.
    if unsafe { js_has_fsa() } != 0 {
        match pick_via_fsa(&accept, multiple).await {
            // Got a result (files or a clean cancel) — done.
            Ok(outcome) => return Ok(outcome),
            // A dialog may have been shown: report, never open a second one.
            Err(FsaFailure::Fail(e)) => return Err(e),
            // FSA provably never showed a dialog — the input may still work.
            Err(FsaFailure::Fallback) => {}
        }
    }

    pick_via_input(&accept.replace('\n', ","), multiple).await
}

/// Why [`pick_via_fsa`] produced no outcome.
enum FsaFailure {
    /// No dialog was shown — try the `<input type=file>`.
    Fallback,
    /// A dialog may have been shown — this is the call's result.
    Fail(PickError),
}

/// `showOpenFilePicker({ multiple, types })`. `Ok(Some(..))` = files,
/// `Ok(None)` = user cancelled, `Err(..)` per [`classify_fsa_rejection`].
async fn pick_via_fsa(accept: &str, multiple: bool) -> Result<Option<Vec<PickedFile>>, FsaFailure> {
    let (ap, al) = string::abi(accept);
    // SAFETY: a borrowed string for the call; the result is a fresh Promise.
    let promise = match unsafe { js_fsa_pick(ap, al, multiple as u32) } {
        Ok(p) => unsafe { JsValue::from_raw(p) },
        // A synchronous throw: `showOpenFilePicker` never returned a promise,
        // so no dialog exists.
        Err(e) => {
            log_warn!("file-picker: showOpenFilePicker threw ({}); using <input type=file>", e.message());
            return Err(FsaFailure::Fallback);
        }
    };
    let e = match JsFuture::new(&promise).await {
        Ok(files) => return Ok(Some(files_of(files))),
        Err(e) => e,
    };
    let str_prop = |k: &str| e.get(k).ok().and_then(|v| v.as_string());
    let stage = match str_prop("stage").as_deref() {
        Some("read") => FsaStage::Read,
        _ => FsaStage::Picker,
    };
    let name = str_prop("name").unwrap_or_default();
    let detail = e.message();
    match classify_fsa_rejection(stage, &name) {
        FsaRejection::Dismissed => Ok(None),
        FsaRejection::Fallback => {
            log_warn!("file-picker: showOpenFilePicker refused before showing ({detail}); using <input type=file>");
            Err(FsaFailure::Fallback)
        }
        FsaRejection::Fail => {
            let msg = match stage {
                FsaStage::Read => format!(
                    "could not read the picked file {:?} ({detail})",
                    str_prop("file").unwrap_or_default()
                ),
                FsaStage::Picker => format!(
                    "the browser refused the pick ({detail}); an embedded browser may deny file-system access"
                ),
            };
            log_warn!("file-picker: {msg}");
            Err(FsaFailure::Fail(PickError::Backend(msg)))
        }
    }
}

/// Fallback: a hidden `<input type=file>` (see [`js_input_pick`]).
async fn pick_via_input(accept: &str, multiple: bool) -> Result<Option<Vec<PickedFile>>, PickError> {
    let (ap, al) = string::abi(accept);
    // SAFETY: a borrowed string for the call; the result is a fresh Promise.
    let promise = unsafe { js_input_pick(ap, al, multiple as u32) }
        .map(|p| unsafe { JsValue::from_raw(p) })
        .map_err(|_| PickError::NoPresenter)?;
    let files = JsFuture::new(&promise).await.map_err(|e| js_err("file input", &e))?;
    let out = files_of(files);
    if out.is_empty() {
        Ok(None)
    } else {
        Ok(Some(out))
    }
}

/// An array of `File`s as picked files.
fn files_of(files: JsValue) -> Vec<PickedFile> {
    files
        .unchecked_into::<Array>()
        .iter()
        .filter_map(|f| f.dyn_into::<File>().ok())
        .map(picked_from_file)
        .collect()
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
