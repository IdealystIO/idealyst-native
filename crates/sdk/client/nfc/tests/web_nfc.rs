//! Browser tests for the Web NFC backend (`NDEFReader`).
//!
//! No headless browser has NFC hardware, so `window.NDEFReader` is replaced
//! per test with a JS stand-in: `scan()` resolves (or rejects with a named
//! DOMException), the instance publishes itself on `globalThis.__nfc` so the
//! test can fire a synthetic `reading` event, and `write()` records the
//! records it was handed. What's under test is the SDK's binding: the
//! record mapping both ways, the serial parse, the one-session guard, abort
//! on close, and error mapping.
//!
//! Run with `cargo test -p nfc --target wasm32-unknown-unknown` (the
//! workspace runner supplies web-glue's JS; `wasm-pack test` cannot).

#![cfg(target_arch = "wasm32")]

use nfc::{availability, Availability, NdefMessage, NdefRecord, NfcError, NfcSession, SessionOptions, Technology, Tnf};
use wasm_bindgen_test::*;
use web_glue::JsValue;

wasm_bindgen_test_configure!(run_in_browser);

fn eval(body: &str) -> JsValue {
    let f = JsValue::global()
        .get("Function")
        .unwrap()
        .construct(&[&JsValue::from_str(body)])
        .unwrap();
    f.call(&JsValue::undefined(), &[]).unwrap()
}

/// Installs a fake `NDEFReader` until dropped. `scan_error` names the
/// DOMException `scan()` rejects with, or `None` to resolve.
struct FakeReader;

impl FakeReader {
    fn install(scan_error: Option<&str>) -> FakeReader {
        let scan = match scan_error {
            Some(name) => format!("return Promise.reject(new DOMException('no', '{name}'));"),
            None => "return Promise.resolve();".to_string(),
        };
        eval(&format!(
            "globalThis.__aborted = false; globalThis.__written = null; \
             window.NDEFReader = class {{ \
               constructor() {{ globalThis.__nfc = this; }} \
               scan(o) {{ o.signal.addEventListener('abort', () => {{ globalThis.__aborted = true; }}); {scan} }} \
               write(m, o) {{ \
                 const conv = (r) => {{ const out = {{}}; for (const k of Object.keys(r)) {{ const v = r[k]; \
                   out[k] = v instanceof Uint8Array ? Array.from(v) : Array.isArray(v) ? v.map(conv) : v; }} return out; }}; \
                 globalThis.__written = JSON.stringify(m.records.map(conv)); return Promise.resolve(); }} \
             }};"
        ));
        FakeReader
    }
}

impl Drop for FakeReader {
    fn drop(&mut self) {
        eval("delete window.NDEFReader; delete globalThis.__nfc; delete globalThis.__written;");
    }
}

/// Fire a `reading` event carrying a text, a url and a mime record.
fn fire_reading() {
    eval(
        "const dv = (a) => new DataView(new Uint8Array(a).buffer); \
         const enc = (s) => dv(Array.from(new TextEncoder().encode(s))); \
         globalThis.__nfc.onreading({ serialNumber: '04:a2:ff', message: { records: [ \
           { recordType: 'text', encoding: 'utf-8', lang: 'en', data: enc('hello'), id: '#1' }, \
           { recordType: 'url', data: enc('https://www.example.com/a') }, \
           { recordType: 'mime', mediaType: 'application/json', data: enc('{}') }, \
           { recordType: 'example.com:badge', data: dv([1, 2]) }, \
           { recordType: 'empty', data: null }, \
         ] } });",
    );
}

#[wasm_bindgen_test]
fn unsupported_without_ndef_reader() {
    assert_eq!(availability(), Availability::Unsupported);
    assert!(matches!(NfcSession::begin(SessionOptions::default()), Err(NfcError::Unsupported(_))));
}

#[wasm_bindgen_test]
async fn reading_event_maps_to_raw_records() {
    let _fake = FakeReader::install(None);
    assert_eq!(availability(), Availability::Available);
    let session = NfcSession::begin(SessionOptions::default()).unwrap();
    fire_reading();
    let tag = session.next_tag().await.unwrap();
    assert_eq!(tag.id(), &[0x04, 0xa2, 0xff]);
    assert_eq!(tag.technologies(), &[Technology::Ndef]);

    let msg = tag.read_ndef().await.unwrap().unwrap();
    let mut text = NdefRecord::text("en", "hello");
    text.id = b"#1".to_vec();
    assert_eq!(
        msg.records,
        vec![
            text,
            NdefRecord::uri("https://www.example.com/a"),
            NdefRecord::mime("application/json", b"{}".to_vec()),
            NdefRecord::new(Tnf::External, b"example.com:badge".to_vec(), vec![], vec![1, 2]),
            NdefRecord::empty(),
        ]
    );
    assert!(matches!(tag.transceive(&[0x00]).await, Err(NfcError::NotAvailable(_))));
    drop(session);
}

#[wasm_bindgen_test]
async fn write_ndef_passes_web_nfc_records() {
    let _fake = FakeReader::install(None);
    let session = NfcSession::begin(SessionOptions::default()).unwrap();
    fire_reading();
    let tag = session.next_tag().await.unwrap();
    let poster = NdefMessage::new(vec![
        NdefRecord::uri("https://example.com"),
        NdefRecord::new(Tnf::WellKnown, b"act".to_vec(), vec![], vec![0]),
    ]);
    let message = NdefMessage::new(vec![
        NdefRecord::text("en", "hi"),
        NdefRecord::uri("tel:+1555"),
        NdefRecord::mime("a/b", vec![7, 8]),
        NdefRecord::external("example.com:x", vec![9]),
        NdefRecord::new(Tnf::WellKnown, b"Sp".to_vec(), vec![], poster.to_bytes().unwrap()),
        NdefRecord::empty(),
    ]);
    tag.write_ndef(&message).await.unwrap();
    let written = eval("return globalThis.__written;").as_string().unwrap();
    assert_eq!(
        written,
        concat!(
            r#"[{"recordType":"text","lang":"en","encoding":"utf-8","data":"hi"},"#,
            r#"{"recordType":"url","data":"tel:+1555"},"#,
            r#"{"recordType":"mime","mediaType":"a/b","data":[7,8]},"#,
            r#"{"recordType":"example.com:x","data":[9]},"#,
            r#"{"recordType":"smart-poster","data":[{"recordType":"url","data":"https://example.com"},{"recordType":":act","data":[0]}]},"#,
            r#"{"recordType":"empty"}]"#,
        )
    );

    // A well-known type Web NFC cannot express at top level is refused.
    let sig = NdefMessage::new(vec![NdefRecord::new(Tnf::WellKnown, b"Sig".to_vec(), vec![], vec![])]);
    assert!(matches!(tag.write_ndef(&sig).await, Err(NfcError::NotAvailable(_))));
}

#[wasm_bindgen_test]
async fn second_session_is_busy_and_close_aborts() {
    let _fake = FakeReader::install(None);
    let session = NfcSession::begin(SessionOptions::default()).unwrap();
    assert!(matches!(NfcSession::begin(SessionOptions::default()), Err(NfcError::Busy)));
    fire_reading();
    let tag = session.next_tag().await.unwrap();
    session.close();
    assert_eq!(eval("return globalThis.__aborted;").as_bool(), Some(true));
    assert!(matches!(tag.read_ndef().await, Err(NfcError::TagLost)));
    // Closed sessions free the slot.
    let again = NfcSession::begin(SessionOptions::default()).unwrap();
    drop(again);
}

#[wasm_bindgen_test]
async fn rejected_scan_maps_dom_exception() {
    let _fake = FakeReader::install(Some("NotAllowedError"));
    let session = NfcSession::begin(SessionOptions::default()).unwrap();
    let err = session.next_tag().await.unwrap_err();
    assert!(matches!(&err, NfcError::Backend(m) if m.contains("permission denied")), "{err:?}");
    // The session is over: later calls return the same error.
    assert_eq!(session.next_tag().await.unwrap_err(), err);
}
