//! Browser tests for the web WebSocket / EventSource arms' web-glue
//! bindings: frame decode (text and binary), send, close status, and the
//! stream's message path.
//!
//! No socket server runs inside the browser test, so `WebSocket` /
//! `EventSource` are replaced per test with JS stand-ins that echo sends
//! back and fire events the way the real objects do. The real objects are
//! exercised by `tests/web_closure_lifetime.rs` (refused connects).
//!
//! Run with `cargo test -p net --target wasm32-unknown-unknown` (the
//! workspace runner supplies web-glue's JS; `wasm-pack test` cannot).

#![cfg(target_arch = "wasm32")]

use net::{Error, EventSource, WebSocket, WsClose, WsMessage};
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

/// Replace `globalThis.<name>` with a JS class until the guard drops.
struct ClassOverride(&'static str);

impl ClassOverride {
    fn install(name: &'static str, class_src: &str) -> ClassOverride {
        eval(&format!("globalThis.__saved{name} = globalThis.{name}; globalThis.{name} = {class_src};"));
        ClassOverride(name)
    }
}

impl Drop for ClassOverride {
    fn drop(&mut self) {
        let n = self.0;
        eval(&format!("globalThis.{n} = globalThis.__saved{n}; delete globalThis.__last{n};"));
    }
}

/// An echo socket: opens on the next task, echoes every `send` back as a
/// message (a copy of the bytes for binary), and closes with 4001/"bye".
const ECHO_SOCKET: &str = "class { \
    constructor(url) { this.url = url; globalThis.__lastWebSocket = this; \
      setTimeout(() => this.onopen && this.onopen(new Event('open')), 0); } \
    send(d) { const data = typeof d === 'string' ? d : d.slice().buffer; \
      globalThis.__sentBinary = typeof d !== 'string' && d instanceof Uint8Array; \
      setTimeout(() => this.onmessage && this.onmessage({ data }), 0); } \
    close() { if (this.closed) return; this.closed = true; \
      setTimeout(() => this.onclose && this.onclose({ code: 4001, reason: 'bye' }), 0); } }";

#[wasm_bindgen_test]
async fn websocket_text_and_binary_round_trip_and_close_status() {
    let _ws = ClassOverride::install("WebSocket", ECHO_SOCKET);
    let mut ws = WebSocket::connect("ws://echo.invalid/").await.expect("opens");

    // The binding asks for ArrayBuffer frames.
    let bt = JsValue::global().get("__lastWebSocket").unwrap().get("binaryType").unwrap();
    assert_eq!(bt.as_string().as_deref(), Some("arraybuffer"));

    ws.send(WsMessage::Text("héllo".into())).unwrap();
    assert_eq!(ws.recv().await.map(Result::unwrap), Some(WsMessage::Text("héllo".into())));

    ws.sender().send(WsMessage::Binary(vec![0, 1, 254, 255])).unwrap();
    assert_eq!(ws.recv().await.map(Result::unwrap), Some(WsMessage::Binary(vec![0, 1, 254, 255])));
    assert_eq!(JsValue::global().get("__sentBinary").unwrap().as_bool(), Some(true));

    ws.close();
    assert!(ws.recv().await.is_none());
    assert_eq!(ws.close_status(), Some(WsClose { code: 4001, reason: "bye".into() }));
}

#[wasm_bindgen_test]
async fn a_malformed_websocket_url_is_a_network_error() {
    // The real constructor throws a SyntaxError for a non-ws(s)/http(s)
    // scheme; the binding catches it. (A bare relative string would NOT
    // throw — current browsers resolve it against the page URL.)
    match WebSocket::connect("ftp://example.invalid/").await {
        Err(Error::Network(msg)) => assert!(msg.contains("SyntaxError"), "{msg}"),
        Err(other) => panic!("expected Error::Network, got {other:?}"),
        Ok(_) => panic!("a malformed URL must not connect"),
    }
}

/// A stream that opens, then delivers two messages.
const TWO_MESSAGE_SOURCE: &str = "class { \
    constructor(url) { globalThis.__lastEventSource = this; \
      setTimeout(() => { this.onopen && this.onopen(new Event('open')); \
        this.onmessage && this.onmessage({ data: 'one' }); \
        this.onmessage && this.onmessage({ data: 'twö' }); }, 0); } \
    close() { this.closed = true; } }";

#[wasm_bindgen_test]
async fn event_source_delivers_messages_and_closes_on_drop() {
    let _es = ClassOverride::install("EventSource", TWO_MESSAGE_SOURCE);
    let mut es = EventSource::connect("http://sse.invalid/events").await.expect("opens");
    assert_eq!(es.recv().await.map(Result::unwrap).as_deref(), Some("one"));
    assert_eq!(es.recv().await.map(Result::unwrap).as_deref(), Some("twö"));

    drop(es);
    let js = JsValue::global().get("__lastEventSource").unwrap();
    assert_eq!(js.get("closed").unwrap().as_bool(), Some(true), "drop closes the stream");
    assert!(js.get("onmessage").unwrap().is_null(), "drop detaches onmessage");
}
