//! Web entropy: `crypto.getRandomValues` through a web-glue binding.
//!
//! Not `getrandom`: its web backend goes through wasm-bindgen, and any
//! wasm-bindgen in an app's graph forces the hybrid build.

web_glue::import! {
    // Fills `l` bytes at `p`. The draw goes into a fresh `Uint8Array` and is
    // copied into wasm memory, rather than handing `getRandomValues` a view
    // of linear memory directly: browsers reject views of a
    // `SharedArrayBuffer` (threaded builds share their memory), and
    // re-reading `G.u8()` after the call keeps the copy valid even if
    // memory grew. `crypto` is a global in windows and workers alike.
    #[catch]
    fn js_get_random_values(p: usize, l: usize) =
        "(p, l) => { const b = new Uint8Array(l >>> 0); \
           globalThis.crypto.getRandomValues(b); G.u8().set(b, p >>> 0); }";
}

/// `getRandomValues` throws a `QuotaExceededError` above 65,536 bytes per
/// call.
const MAX_PER_CALL: usize = 65_536;

pub(crate) fn fill(buf: &mut [u8]) {
    for chunk in buf.chunks_mut(MAX_PER_CALL) {
        // SAFETY: the snippet writes exactly `chunk.len()` bytes at
        // `chunk`'s address, which is valid, exclusively borrowed memory.
        if let Err(e) = unsafe { js_get_random_values(chunk.as_mut_ptr() as usize, chunk.len()) } {
            panic!("random: crypto.getRandomValues failed: {}", e.message());
        }
    }
}
