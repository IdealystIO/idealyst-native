//! The bundle half over wasm: the callback table as wasm exports, for
//! `stream-host`'s `Link`. Compiled only into a remote bundle
//! (`--cfg idealyst_stream_guest`).
//!
//! Bytes cross through two buffers in this module's memory. The host asks
//! [`idealyst_ui_alloc`] for room, writes a call's arguments there, then
//! calls [`idealyst_ui_invoke`], which COPIES them out before running the
//! callback — the callback may re-enter (a getter reads a signal → kernel
//! import → host → another invoke) and reuse the buffer. A reply is written
//! to the reply buffer when the callback has finished, and the host reads
//! it immediately on return, before it calls anything else, so the nested
//! calls' replies are never in the way.

use std::cell::RefCell;

use super::bundle::{invoke, release};
use super::Cb;

thread_local! {
    static ARGS: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static REPLY: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Room for `len` bytes of arguments; where to write them.
#[no_mangle]
pub extern "C" fn idealyst_ui_alloc(len: u32) -> *mut u8 {
    ARGS.with(|a| {
        let mut a = a.borrow_mut();
        a.clear();
        a.resize(len as usize, 0);
        a.as_mut_ptr()
    })
}

/// The arguments the host wrote, copied out.
pub fn take_args(len: u32) -> Vec<u8> {
    ARGS.with(|a| a.borrow()[..len as usize].to_vec())
}

/// Run callback `cb` on the `len` argument bytes the host wrote; the reply,
/// packed as `ptr << 32 | len`.
#[no_mangle]
pub extern "C" fn idealyst_ui_invoke(cb: Cb, len: u32) -> i64 {
    let args = take_args(len);
    reply(invoke(cb, &args))
}

#[no_mangle]
pub extern "C" fn idealyst_ui_release(cb: Cb) {
    release(cb)
}

/// Hand `bytes` to the host as an export's result, packed as
/// `ptr << 32 | len` (what a remote component's mount export returns).
pub fn reply(bytes: Vec<u8>) -> i64 {
    REPLY.with(|r| {
        let mut r = r.borrow_mut();
        *r = bytes;
        ((r.as_ptr() as u32 as i64) << 32) | r.len() as i64
    })
}

/// Live callback entries (`bundle::live_callbacks`) — for the host's leak
/// checks.
#[no_mangle]
pub extern "C" fn idealyst_ui_live_callbacks() -> u32 {
    super::bundle::live_callbacks() as u32
}

thread_local! {
    static LAST_PANIC: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Keep the message of a panic in this bundle, for the host to read after
/// the trap it ends in (`idealyst_ui_panic_message`). A wasm panic aborts
/// with a bare `unreachable`; without this the host can only say "trapped".
/// Called by the host once, at load.
#[no_mangle]
pub extern "C" fn idealyst_ui_init() {
    std::panic::set_hook(Box::new(|info| {
        let msg = match (info.payload().downcast_ref::<&str>(), info.payload().downcast_ref::<String>()) {
            (Some(s), _) => (*s).to_string(),
            (_, Some(s)) => s.clone(),
            _ => "panic".to_string(),
        };
        let at = info.location().map(|l| format!(" ({}:{})", l.file(), l.line())).unwrap_or_default();
        let _ = LAST_PANIC.try_with(|p| *p.borrow_mut() = Some(format!("{msg}{at}")));
    }));
}

/// The last panic's message, packed like a reply; length 0 for none.
#[no_mangle]
pub extern "C" fn idealyst_ui_panic_message() -> i64 {
    reply(LAST_PANIC.with(|p| p.borrow_mut().take()).unwrap_or_default().into_bytes())
}
