//! The web-glue bindings the WebSocket and EventSource arms share
//! (own-web-bindings phase 3): constructing the JS object, assigning and
//! clearing its `on<event>` handler slots, and reading an event's payload.
//!
//! The HTTP arm (`web.rs`) declares its own fetch module (phase 4).

use web_glue::{string, Closure, JsError, JsValue};

web_glue::import! {
    // `new WebSocket(url)` with `binaryType = "arraybuffer"`, so binary
    // frames arrive as ArrayBuffers (never Blobs, which would need an
    // async read). Throws (→ Err) for a malformed URL.
    #[catch]
    fn js_websocket_new(p: usize, l: usize) -> u32 =
        "(p, l) => { const w = new WebSocket(G.str(p, l)); w.binaryType = 'arraybuffer'; return G.add(w); }";
    // `new EventSource(url)`. Throws (→ Err) for a malformed URL.
    #[catch]
    fn js_event_source_new(p: usize, l: usize) -> u32 =
        "(p, l) => G.add(new EventSource(G.str(p, l)))";
    // `target[prop] = f`, or `= null` for handle 0.
    fn js_set_handler(t: u32, p: usize, l: usize, f: u32) =
        "(t, p, l, f) => { G.get(t)[G.str(p, l)] = f === 0 ? null : G.get(f); }";
    #[catch]
    fn js_close(t: u32) = "(t) => { G.get(t).close(); }";
    #[catch]
    fn js_send_text(t: u32, p: usize, l: usize) = "(t, p, l) => { G.get(t).send(G.str(p, l)); }";
    // The bytes are COPIED (`slice`) before `send`: the socket may queue
    // them past this call, and a view of wasm memory would not survive a
    // later memory growth.
    #[catch]
    fn js_send_bytes(t: u32, p: usize, l: usize) =
        "(t, p, l) => { G.get(t).send(G.u8().slice(p >>> 0, (p >>> 0) + (l >>> 0))); }";
    // `event.data` into `out` when it is a string (1), else 0.
    fn js_data_text(e: u32, out: usize) -> u32 =
        "(e, o) => { const d = G.get(e).data; if (typeof d !== 'string') return 0; G.retStr(d, o); return 1; }";
    // `event.data` as a handle when it is an ArrayBuffer, else 0.
    fn js_data_buffer(e: u32) -> u32 =
        "(e) => { const d = G.get(e).data; return d instanceof ArrayBuffer ? G.add(d) : 0; }";
    fn js_byte_length(b: u32) -> u32 = "(b) => G.get(b).byteLength";
    // Copy an ArrayBuffer into `len` bytes Rust already allocated at `p`.
    fn js_copy_bytes(b: u32, p: usize) = "(b, p) => { G.u8().set(new Uint8Array(G.get(b)), p >>> 0); }";
    fn js_close_code(e: u32) -> u32 = "(e) => G.get(e).code >>> 0";
    fn js_close_reason(e: u32, out: usize) =
        "(e, o) => { const r = G.get(e).reason; G.retStr(r == null ? '' : String(r), o); }";
}

fn adopt(r: Result<u32, JsError>) -> Result<JsValue, JsError> {
    // SAFETY: a fresh `G.add` slot the snippet minted for us.
    r.map(|h| unsafe { JsValue::from_raw(h) })
}

/// `new WebSocket(url)`, binary frames as ArrayBuffers.
pub(crate) fn websocket_new(url: &str) -> Result<JsValue, JsError> {
    let (p, l) = string::abi(url);
    adopt(unsafe { js_websocket_new(p, l) })
}

/// `new EventSource(url)`.
pub(crate) fn event_source_new(url: &str) -> Result<JsValue, JsError> {
    let (p, l) = string::abi(url);
    adopt(unsafe { js_event_source_new(p, l) })
}

/// `target.<prop> = f` (`None` clears the slot to `null`).
pub(crate) fn set_handler(target: &JsValue, prop: &str, f: Option<&Closure>) {
    let (p, l) = string::abi(prop);
    let f = f.map_or(0, |c| c.as_js().raw());
    unsafe { js_set_handler(target.raw(), p, l, f) }
}

/// `target.close()`, errors ignored (closing twice is fine).
pub(crate) fn close(target: &JsValue) {
    let _ = unsafe { js_close(target.raw()) };
}

pub(crate) fn send_text(target: &JsValue, s: &str) -> Result<(), JsError> {
    let (p, l) = string::abi(s);
    unsafe { js_send_text(target.raw(), p, l) }
}

pub(crate) fn send_bytes(target: &JsValue, b: &[u8]) -> Result<(), JsError> {
    unsafe { js_send_bytes(target.raw(), b.as_ptr() as usize, b.len()) }
}

/// A message event's `data`, if it is a string.
pub(crate) fn data_text(ev: &JsValue) -> Option<String> {
    let mut hit = 0;
    let s = string::receive(|o| hit = unsafe { js_data_text(ev.raw(), o) });
    (hit != 0).then_some(s)
}

/// A message event's `data`, if it is an ArrayBuffer, copied out.
pub(crate) fn data_bytes(ev: &JsValue) -> Option<Vec<u8>> {
    let buf = match unsafe { js_data_buffer(ev.raw()) } {
        0 => return None,
        // SAFETY: a fresh `G.add` slot the snippet minted for us.
        h => unsafe { JsValue::from_raw(h) },
    };
    let len = unsafe { js_byte_length(buf.raw()) } as usize;
    // Allocated BEFORE the copy crossing, so the snippet's memory view is
    // taken after the last thing that could grow memory.
    let mut out = vec![0u8; len];
    if len != 0 {
        unsafe { js_copy_bytes(buf.raw(), out.as_mut_ptr() as usize) }
    }
    Some(out)
}

/// A `CloseEvent`'s `code` and `reason`.
pub(crate) fn close_code_and_reason(ev: &JsValue) -> (u16, String) {
    let code = unsafe { js_close_code(ev.raw()) } as u16;
    let reason = string::receive(|o| unsafe { js_close_reason(ev.raw(), o) });
    (code, reason)
}
