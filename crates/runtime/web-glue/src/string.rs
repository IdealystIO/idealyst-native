//! Strings across the boundary.
//!
//! **Rust → JS** is a borrow: the snippet gets `(ptr, len)` of UTF-8 and
//! decodes it with `G.str(ptr, len)` (one `TextDecoder.decode` over a
//! subarray view — no copy on the Rust side).
//!
//! **JS → Rust** uses a Rust-allocated buffer: the snippet calls
//! `G.retStr(s, out)`, which asks the exported [`__glue_alloc`] for
//! `s.length` bytes, UTF-8-encodes `s` STRAIGHT INTO that buffer (ASCII by
//! `charCodeAt`, the rest — from the first non-ASCII unit on — with
//! `TextEncoder.encodeInto` after growing the buffer to the worst case
//! through [`__glue_realloc`], then shrinking it to what was written), and
//! writes `[ptr, len]` into the two-word out-slot the Rust wrapper passed.
//! Rust then adopts the buffer as a `String` with no further copy.
//!
//! Why this and not "length, then copy": a JS string's UTF-8 length is only
//! known after encoding it, so length-then-copy needs two crossings plus
//! either a second encode or a JS-side stash of the bytes between them.
//! The alloc re-entry is one crossing (JS → wasm) inside the import that
//! is already running, and wasm-bindgen uses the same shape
//! (`__wbindgen_malloc` / `__wbindgen_realloc`) for the same reason. An
//! ASCII string — nearly every string the framework reads back — costs
//! exactly that one crossing; encoding into a temporary
//! `TextEncoder.encode` array and copying it in was ~8× slower per string
//! (see `js/runtime.js`, `retStr`).
//!
//! The price is the invariant this module's E2E pins: `__glue_alloc` and
//! `__glue_realloc` may GROW MEMORY, which detaches every JS view of the
//! old buffer, so the JS side must take its `Uint8Array` view after each
//! of them, never before (see `js/runtime.js`, "MEMORY VIEWS ARE NEVER
//! HELD ACROSS A CALL INTO WASM"). [`debug_grow_on_next_alloc`] forces
//! that growth deterministically, on whichever of the two runs next.

use std::alloc::{alloc, realloc, Layout};
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
    static GROW_ON_NEXT_REALLOC: Cell<bool> = const { Cell::new(false) };
}

/// Make the next [`__glue_alloc`] grow linear memory by one page before
/// allocating — i.e. detach every JS view of the old buffer at the moment
/// `G.retStr` is exposed to it. Test hook for the memory-growth invariant;
/// costs one thread-local read per call.
pub fn debug_grow_on_next_alloc() {
    GROW_ON_NEXT_ALLOC.with(|g| g.set(true));
}

/// [`debug_grow_on_next_alloc`] for [`__glue_realloc`]: the growth lands
/// between `G.retStr`'s ASCII prefix and its `encodeInto` of a non-ASCII
/// tail, which must write through a fresh view.
pub fn debug_grow_on_next_realloc() {
    GROW_ON_NEXT_REALLOC.with(|g| g.set(true));
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
    grow_if_requested(&GROW_ON_NEXT_ALLOC);
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

fn grow_if_requested(flag: &'static std::thread::LocalKey<Cell<bool>>) {
    if flag.with(|g| g.replace(false)) {
        #[cfg(target_arch = "wasm32")]
        core::arch::wasm32::memory_grow(0, 1);
    }
}

/// Resize a buffer [`__glue_alloc`] returned (`G.retStr` grows it for a
/// non-ASCII tail, then shrinks it to the encoded length). Exported;
/// called only by `G.retStr`. Both sizes are non-zero.
///
/// # Safety
///
/// `ptr` must be a live `__glue_alloc(old)` / `__glue_realloc(_, _, old)`
/// result; it is invalid after this call. The JS runtime is the only
/// caller and upholds that.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __glue_realloc(ptr: *mut u8, old: usize, new: usize) -> *mut u8 {
    grow_if_requested(&GROW_ON_NEXT_REALLOC);
    assert!(old != 0 && new != 0, "web-glue: __glue_realloc of an empty buffer");
    let layout = Layout::from_size_align(old, 1).expect("web-glue: string too large");
    // SAFETY: `ptr`/`layout` are what the matching alloc returned/used (the
    // caller's contract); `new` is non-zero.
    let p = unsafe { realloc(ptr, layout, new) };
    if p.is_null() {
        std::alloc::handle_alloc_error(Layout::from_size_align(new, 1).expect("web-glue: string too large"));
    }
    p
}
