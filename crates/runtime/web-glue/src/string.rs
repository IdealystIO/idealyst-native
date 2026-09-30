//! Strings across the boundary.
//!
//! **Rust → JS** is a borrow: the snippet gets `(ptr, len)` of UTF-8 and
//! decodes it with `G.str(ptr, len)` (one `TextDecoder.decode` over a
//! subarray view — no copy on the Rust side).
//!
//! **JS → Rust** uses a Rust-allocated buffer: the snippet calls
//! `G.retStr(s, out)`, which UTF-8-encodes `s`, calls the exported
//! [`__glue_alloc`] for exactly that many bytes, copies them in, and
//! writes `[ptr, len]` into the two-word out-slot the Rust wrapper passed.
//! Rust then adopts the buffer as a `String` with no further copy.
//!
//! Why this and not "length, then copy": a JS string's UTF-8 length is only
//! known after encoding it, so length-then-copy needs two crossings plus
//! either a second encode or a JS-side stash of the bytes between them.
//! The alloc re-entry is one crossing (JS → wasm) inside the import that
//! is already running, and wasm-bindgen uses the same shape
//! (`__wbindgen_malloc`) for the same reason.
//!
//! The price is the invariant this module's E2E pins: `__glue_alloc` may
//! GROW MEMORY, which detaches every JS view of the old buffer, so the JS
//! side must take its `Uint8Array` view after the alloc, never before (see
//! `js/runtime.js`, "MEMORY VIEWS ARE NEVER HELD ACROSS A CALL INTO WASM").
//! [`debug_grow_on_next_alloc`] forces that growth deterministically.

use std::alloc::{alloc, Layout};
use std::cell::Cell;

/// `(ptr, len)` of a `&str`, for a snippet that reads it with `G.str`.
#[inline]
pub fn abi(s: &str) -> (usize, usize) {
    (s.as_ptr() as usize, s.len())
}

/// Run `f` with the address of a fresh out-slot and adopt whatever string
/// the snippet wrote there with `G.retStr`. An untouched slot (the snippet
/// returned early, or threw) yields `""`.
pub fn receive(f: impl FnOnce(usize)) -> String {
    let mut slot = [0usize; 2];
    f(slot.as_mut_ptr() as usize);
    let [ptr, len] = slot;
    if len == 0 {
        return String::new();
    }
    // SAFETY: `ptr` came from `__glue_alloc(len)` — the global allocator,
    // `Layout::from_size_align(len, 1)` — so capacity == len and the
    // layout matches. `TextEncoder` only ever produces valid UTF-8 (lone
    // surrogates are encoded as U+FFFD).
    unsafe { String::from_raw_parts(ptr as *mut u8, len, len) }
}

thread_local! {
    static GROW_ON_NEXT_ALLOC: Cell<bool> = const { Cell::new(false) };
}

/// Make the next [`__glue_alloc`] grow linear memory by one page before
/// allocating — i.e. detach every JS view of the old buffer at the one
/// moment `G.retStr` is exposed to it. Test hook for the memory-growth
/// invariant; costs one thread-local read per alloc.
pub fn debug_grow_on_next_alloc() {
    GROW_ON_NEXT_ALLOC.with(|g| g.set(true));
}

/// JS → Rust buffer allocation. Exported; called by `G.retStr`.
///
/// Also the ANCHOR of web-glue's JS runtime record. Exported functions are
/// always linked, so a static nested here is always in the linked module —
/// see `js_module!` for why a free-standing `#[link_section]` static is
/// not.
#[unsafe(no_mangle)]
pub extern "C" fn __glue_alloc(len: usize) -> *mut u8 {
    #[cfg(target_arch = "wasm32")]
    {
        const NAME: &str = "runtime";
        const SRC: &str = include_str!("../js/runtime.js");
        const LEN: usize = crate::record::len(NAME, SRC);
        // No `#[used]` — see `js_module!`: it would duplicate the
        // runtime into the data section.
        #[unsafe(link_section = "__idealyst_glue")]
        #[allow(dead_code)]
        static RUNTIME: [u8; LEN] = crate::record::encode::<LEN>(crate::record::KIND_RUNTIME, NAME, SRC);
    }
    if GROW_ON_NEXT_ALLOC.with(|g| g.replace(false)) {
        #[cfg(target_arch = "wasm32")]
        core::arch::wasm32::memory_grow(0, 1);
    }
    if len == 0 {
        return std::ptr::NonNull::<u8>::dangling().as_ptr();
    }
    let layout = Layout::from_size_align(len, 1).expect("web-glue: string too large");
    // SAFETY: non-zero size.
    let p = unsafe { alloc(layout) };
    if p.is_null() {
        std::alloc::handle_alloc_error(layout);
    }
    p
}
