//! Web backend over Web NFC (`NDEFReader`).
//!
//! Web NFC exists only in Chrome on Android, in a secure context, and only
//! speaks NDEF: there are no raw tag commands, so
//! [`TagHandle::transceive`] is [`NfcError::NotAvailable`]. Elsewhere
//! [`availability`] is [`Availability::Unsupported`].
//!
//! A session is one `NDEFReader` scanning under an `AbortController`. Each
//! `reading` event is one tag: the browser has already read its NDEF
//! message, so [`TagHandle::read_ndef`] returns that capture. Web NFC
//! records are typed (`"text"`, `"url"`, `"mime"`, …) rather than raw; they
//! are mapped onto raw [`NdefRecord`]s here and back again on write.
//!
//! **Writes go to the tag in proximity.** `NDEFReader.write` has no handle to
//! the tag that produced a `reading` event: it writes to whatever tag is in
//! range, or waits for the next one tapped. While the user keeps the tag
//! that was just read against the phone, that is the same tag.
//!
//! VERIFICATION: `tests/web_nfc.rs` runs this backend in a headless browser
//! against a JS stand-in for `NDEFReader` (no browser on a test machine has
//! NFC hardware): record mapping both ways, the serial-number parse, the
//! `Busy` guard, abort-on-close and error mapping. Not exercised against a
//! real tag on Chrome for Android.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use web_glue::callback::Closure;
use web_glue::{string, JsError, JsFuture, JsValue};

use crate::mailbox::Mailbox;
use crate::{Availability, NdefMessage, NdefRecord, NfcError, SessionOptions, Tag, Technology, Tnf};

web_glue::import! {
    fn js_supported() -> u32 =
        "() => typeof window !== 'undefined' && 'NDEFReader' in window ? 1 : 0";
    fn js_reader_new() -> u32 = "() => G.add(new window.NDEFReader())";
    fn js_controller_new() -> u32 = "() => G.add(new AbortController())";
    #[catch]
    fn js_scan(reader: u32, controller: u32) -> u32 =
        "(r, c) => G.add(G.get(r).scan({ signal: G.get(c).signal }))";
    fn js_listen(reader: u32, reading: u32, error: u32) =
        "(r, a, b) => { const x = G.get(r); x.onreading = G.get(a); x.onreadingerror = G.get(b); }";
    fn js_unlisten(reader: u32) =
        "(r) => { const x = G.get(r); x.onreading = null; x.onreadingerror = null; }";
    fn js_abort(controller: u32) = "(c) => { G.get(c).abort(); }";
    fn js_serial(ev: u32, out: usize) =
        "(e, o) => { const s = G.get(e).serialNumber; if (typeof s === 'string') G.retStr(s, o); }";
    fn js_record_count(ev: u32) -> u32 =
        "(e) => { const m = G.get(e).message; return m && m.records ? m.records.length >>> 0 : 0; }";
    // Field `k` of record `i`: 0 recordType, 1 mediaType, 2 id, 3 encoding, 4 lang.
    fn js_record_str(ev: u32, i: u32, k: u32, out: usize) =
        "(e, i, k, o) => { const r = G.get(e).message.records[i >>> 0]; \
           const v = [r.recordType, r.mediaType, r.id, r.encoding, r.lang][k]; \
           if (typeof v === 'string') G.retStr(v, o); }";
    fn js_record_data_len(ev: u32, i: u32) -> u32 =
        "(e, i) => { const d = G.get(e).message.records[i >>> 0].data; return d ? d.byteLength >>> 0 : 0; }";
    fn js_record_data_copy(ev: u32, i: u32, p: usize) =
        "(e, i, p) => { const d = G.get(e).message.records[i >>> 0].data; \
           if (d) G.u8().set(new Uint8Array(d.buffer, d.byteOffset, d.byteLength), p >>> 0); }";
    fn js_bytes(p: usize, n: usize) -> u32 =
        "(p, n) => G.add(G.u8().slice(p >>> 0, (p >>> 0) + (n >>> 0)))";
    fn js_record_new(tp: usize, tl: usize) -> u32 =
        "(p, l) => G.add({ recordType: G.str(p, l) })";
    fn js_array_new() -> u32 = "() => G.add([])";
    #[catch]
    fn js_write(reader: u32, records: u32, controller: u32) -> u32 =
        "(r, recs, c) => G.add(G.get(r).write({ records: G.get(recs) }, { signal: G.get(c).signal }))";
    fn js_error_name(err: u32, out: usize) =
        "(e, o) => { const v = G.get(e); if (v && typeof v.name === 'string') G.retStr(v.name, o); }";
}

thread_local! {
    /// Web NFC allows one active scan per page in practice; mirror the
    /// crate-wide one-session rule.
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn availability() -> Availability {
    if unsafe { js_supported() } != 0 {
        Availability::Available
    } else {
        Availability::Unsupported
    }
}

/// What a tag handle needs from its session: the reader to write through and
/// whether the session is still open.
struct Shared {
    reader: JsValue,
    controller: JsValue,
    open: Cell<bool>,
}

pub(crate) struct Session {
    shared: Rc<Shared>,
    mailbox: Mailbox<(Vec<u8>, NdefMessage)>,
    /// The `scan()` promise, awaited by the first `next_tag`.
    scan: RefCell<Option<JsFuture>>,
    /// Keep the `onreading` / `onreadingerror` functions alive; dropping them
    /// after `js_unlisten` revokes them.
    listeners: RefCell<Option<(Closure, Closure)>>,
}

impl Session {
    pub(crate) fn begin(_options: &SessionOptions) -> Result<Session, NfcError> {
        if availability() != Availability::Available {
            return Err(NfcError::Unsupported(
                "Web NFC (NDEFReader) needs Chrome on Android in a secure context".into(),
            ));
        }
        if ACTIVE.with(|a| a.replace(true)) {
            return Err(NfcError::Busy);
        }
        let reader = unsafe { JsValue::from_raw(js_reader_new()) };
        let controller = unsafe { JsValue::from_raw(js_controller_new()) };
        let mailbox = Mailbox::new();

        let on_reading = {
            let mailbox = mailbox.clone();
            Closure::new(move |ev: JsValue| mailbox.push(read_event(&ev)))
        };
        // `readingerror`: a tag was seen but could not be read as NDEF. Web
        // NFC exposes nothing else about it, so the session keeps scanning.
        let on_error = Closure::new(|_ev: JsValue| {});
        unsafe { js_listen(reader.raw(), on_reading.as_js().raw(), on_error.as_js().raw()) };

        let scan = match unsafe { js_scan(reader.raw(), controller.raw()) } {
            Ok(h) => JsFuture::new(&unsafe { JsValue::from_raw(h) }),
            Err(e) => {
                unsafe { js_unlisten(reader.raw()) };
                ACTIVE.with(|a| a.set(false));
                return Err(map_error(&e));
            }
        };

        Ok(Session {
            shared: Rc::new(Shared { reader, controller, open: Cell::new(true) }),
            mailbox,
            scan: RefCell::new(Some(scan)),
            listeners: RefCell::new(Some((on_reading, on_error))),
        })
    }

    pub(crate) async fn next_tag(&self) -> Result<Tag, NfcError> {
        let scan = self.scan.borrow_mut().take();
        if let Some(scan) = scan {
            if let Err(e) = scan.await {
                let err = map_error(&e);
                self.mailbox.close(err.clone());
                return Err(err);
            }
        }
        let (id, message) = self.mailbox.recv().await?;
        let handle = TagHandle { shared: Rc::clone(&self.shared), message };
        Ok(Tag::new(id, vec![Technology::Ndef], handle))
    }

    pub(crate) fn set_prompt(&self, _text: &str) {}

    pub(crate) fn close(&self, _error_message: Option<&str>) {
        if !self.shared.open.replace(false) {
            return;
        }
        unsafe {
            js_unlisten(self.shared.reader.raw());
            js_abort(self.shared.controller.raw());
        }
        self.listeners.borrow_mut().take();
        self.mailbox.close(NfcError::Cancelled);
        ACTIVE.with(|a| a.set(false));
    }
}

pub(crate) struct TagHandle {
    shared: Rc<Shared>,
    message: NdefMessage,
}

impl TagHandle {
    pub(crate) async fn read_ndef(&self) -> Result<Option<NdefMessage>, NfcError> {
        if !self.shared.open.get() {
            return Err(NfcError::TagLost);
        }
        Ok(Some(self.message.clone()))
    }

    pub(crate) async fn write_ndef(&self, message: &NdefMessage) -> Result<(), NfcError> {
        if !self.shared.open.get() {
            return Err(NfcError::TagLost);
        }
        let records = records_to_js(&message.records, false)?;
        let promise = unsafe {
            js_write(self.shared.reader.raw(), records.raw(), self.shared.controller.raw())
        }
        .map_err(|e| map_error(&e))?;
        JsFuture::new(&unsafe { JsValue::from_raw(promise) })
            .await
            .map(|_| ())
            .map_err(|e| map_error(&e))
    }

    pub(crate) async fn transceive(&self, _command: &[u8]) -> Result<Vec<u8>, NfcError> {
        Err(NfcError::NotAvailable("raw tag commands on Web NFC"))
    }
}

/// Map a DOMException from `scan()` / `write()` to an [`NfcError`].
fn map_error(e: &JsError) -> NfcError {
    let name = string::receive(|o| unsafe { js_error_name(e.value().raw(), o) });
    match name.as_str() {
        "AbortError" => NfcError::Cancelled,
        "NetworkError" => NfcError::TagLost,
        "NotSupportedError" => NfcError::Unsupported(e.message()),
        "NotAllowedError" => NfcError::Backend(format!("NFC permission denied: {}", e.message())),
        "NotReadableError" => NfcError::Disabled,
        _ => NfcError::Backend(e.message()),
    }
}

// ---------------------------------------------------------------------------
// Reading: Web NFC record → raw NDEF record.
// ---------------------------------------------------------------------------

/// Parse a Web NFC serial number (`"04:a2:…"`). Empty when absent or not
/// hex.
fn parse_serial(s: &str) -> Vec<u8> {
    if s.is_empty() {
        return Vec::new();
    }
    s.split(':').map(|b| u8::from_str_radix(b, 16)).collect::<Result<_, _>>().unwrap_or_default()
}

fn read_event(ev: &JsValue) -> (Vec<u8>, NdefMessage) {
    let id = parse_serial(&string::receive(|o| unsafe { js_serial(ev.raw(), o) }));
    let n = unsafe { js_record_count(ev.raw()) };
    let records = (0..n)
        .map(|i| {
            let field = |k: u32| string::receive(|o| unsafe { js_record_str(ev.raw(), i, k, o) });
            let len = unsafe { js_record_data_len(ev.raw(), i) } as usize;
            let mut data = vec![0u8; len];
            if len > 0 {
                unsafe { js_record_data_copy(ev.raw(), i, data.as_mut_ptr() as usize) };
            }
            WebRecord {
                record_type: field(0),
                media_type: field(1),
                id: field(2),
                encoding: field(3),
                lang: field(4),
                data,
            }
            .into_ndef()
        })
        .collect();
    (id, NdefMessage::new(records))
}

/// One Web NFC record's fields, as read from a `reading` event.
struct WebRecord {
    record_type: String,
    media_type: String,
    id: String,
    encoding: String,
    lang: String,
    data: Vec<u8>,
}

impl WebRecord {
    /// The Web NFC spec's record-type mapping, inverted.
    fn into_ndef(self) -> NdefRecord {
        let id = self.id.into_bytes();
        let mut record = match self.record_type.as_str() {
            "empty" => NdefRecord::empty(),
            "text" => {
                let lang = &self.lang.as_bytes()[..self.lang.len().min(0x3F)];
                let utf16 = if self.encoding.starts_with("utf-16") { 0x80 } else { 0 };
                let mut payload = Vec::with_capacity(1 + lang.len() + self.data.len());
                payload.push(lang.len() as u8 | utf16);
                payload.extend_from_slice(lang);
                payload.extend_from_slice(&self.data);
                NdefRecord::new(Tnf::WellKnown, b"T".to_vec(), Vec::new(), payload)
            }
            "url" => NdefRecord::uri(&String::from_utf8_lossy(&self.data)),
            "absolute-url" => NdefRecord::new(Tnf::AbsoluteUri, self.data, Vec::new(), Vec::new()),
            "mime" => NdefRecord::new(Tnf::Media, self.media_type.into_bytes(), Vec::new(), self.data),
            "smart-poster" => NdefRecord::new(Tnf::WellKnown, b"Sp".to_vec(), Vec::new(), self.data),
            "unknown" => NdefRecord::new(Tnf::Unknown, Vec::new(), Vec::new(), self.data),
            t if t.starts_with(':') => {
                NdefRecord::new(Tnf::WellKnown, t.as_bytes()[1..].to_vec(), Vec::new(), self.data)
            }
            t if t.contains(':') => NdefRecord::new(Tnf::External, t.as_bytes().to_vec(), Vec::new(), self.data),
            _ => NdefRecord::new(Tnf::Unknown, Vec::new(), Vec::new(), self.data),
        };
        // An Empty record carries no id by definition.
        if record.tnf != Tnf::Empty {
            record.id = id;
        }
        record
    }
}

// ---------------------------------------------------------------------------
// Writing: raw NDEF record → Web NFC record init.
// ---------------------------------------------------------------------------

fn js_bytes_of(data: &[u8]) -> JsValue {
    unsafe { JsValue::from_raw(js_bytes(data.as_ptr() as usize, data.len())) }
}

fn set(obj: &JsValue, key: &str, value: &JsValue) -> Result<(), NfcError> {
    obj.set(key, value).map_err(|e| NfcError::Backend(e.message()))
}

/// Records to a JS array of `NDEFRecordInit`. `nested` is true inside a
/// smart poster, where other well-known types are written as local types
/// (`":act"`).
fn records_to_js(records: &[NdefRecord], nested: bool) -> Result<JsValue, NfcError> {
    let array = unsafe { JsValue::from_raw(js_array_new()) };
    for r in records {
        let init = record_to_js(r, nested)?;
        array.call_method("push", &[&init]).map_err(|e| NfcError::Backend(e.message()))?;
    }
    Ok(array)
}

fn record_to_js(r: &NdefRecord, nested: bool) -> Result<JsValue, NfcError> {
    let new = |t: &str| {
        let (p, l) = string::abi(t);
        unsafe { JsValue::from_raw(js_record_new(p, l)) }
    };
    let utf8 = |b: &[u8]| -> Result<String, NfcError> {
        String::from_utf8(b.to_vec()).map_err(|_| NfcError::NotAvailable("a non-UTF-8 record type on Web NFC"))
    };
    let init = match r.tnf {
        Tnf::Empty => return Ok(new("empty")),
        Tnf::WellKnown if r.record_type == b"T" => {
            let text = r.as_text().ok_or(NfcError::NotAvailable("a malformed Text record on Web NFC"))?;
            let init = new("text");
            set(&init, "lang", &JsValue::from_str(&text.language))?;
            set(&init, "encoding", &JsValue::from_str("utf-8"))?;
            set(&init, "data", &JsValue::from_str(&text.text))?;
            init
        }
        Tnf::WellKnown if r.record_type == b"U" => {
            let uri = r.as_uri().ok_or(NfcError::NotAvailable("a malformed URI record on Web NFC"))?;
            let init = new("url");
            set(&init, "data", &JsValue::from_str(&uri))?;
            init
        }
        Tnf::WellKnown if r.record_type == b"Sp" => {
            let nested_msg = NdefMessage::from_bytes(&r.payload)?;
            let init = new("smart-poster");
            set(&init, "data", &records_to_js(&nested_msg.records, true)?)?;
            init
        }
        Tnf::WellKnown if nested => {
            let init = new(&format!(":{}", utf8(&r.record_type)?));
            set(&init, "data", &js_bytes_of(&r.payload))?;
            init
        }
        Tnf::WellKnown => return Err(NfcError::NotAvailable("this well-known record type on Web NFC")),
        Tnf::Media => {
            let init = new("mime");
            set(&init, "mediaType", &JsValue::from_str(&utf8(&r.record_type)?))?;
            set(&init, "data", &js_bytes_of(&r.payload))?;
            init
        }
        Tnf::AbsoluteUri => {
            let init = new("absolute-url");
            set(&init, "data", &JsValue::from_str(&utf8(&r.record_type)?))?;
            init
        }
        Tnf::External => {
            let init = new(&utf8(&r.record_type)?);
            set(&init, "data", &js_bytes_of(&r.payload))?;
            init
        }
        Tnf::Unknown => {
            let init = new("unknown");
            set(&init, "data", &js_bytes_of(&r.payload))?;
            init
        }
        Tnf::Reserved => return Err(NfcError::NotAvailable("reserved-TNF records on Web NFC")),
    };
    if !r.id.is_empty() {
        set(&init, "id", &JsValue::from_str(&String::from_utf8_lossy(&r.id)))?;
    }
    Ok(init)
}
