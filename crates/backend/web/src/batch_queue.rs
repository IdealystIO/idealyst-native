//! Shared batching primitive for "one FFI call per fan-out, regardless
//! of N" updates.
//!
//! Every batched surface in the web backend boils down to the same
//! shape: queue `(id, string_payload)` tuples in a per-turn buffer,
//! schedule a microtask flush, ship the batch in ONE FFI call to a
//! JS-side shim that addresses individual nodes by id. This module
//! factors out the bookkeeping so each batched surface (text content,
//! class attributes, future attribute updates) is just:
//!
//!   1. A `StringBatchQueue` instance with a JS function name.
//!   2. A short `queue_*` method on `WebBackend` that calls
//!      `self.<queue>.queue(id, payload)` and registers the node if
//!      this is its first appearance.
//!   3. A matching JS shim that accepts `(ids: Uint32Array, joined:
//!      string, lengths: Uint32Array)` and walks them.
//!
//! Both pre-existing batched paths (`text_batch.js`,
//! `class_batch.js`) follow this contract; this module is what they
//! share rather than duplicate.

use std::cell::Cell;
use std::rc::Rc;

use js_sys::{Function, Reflect, Uint32Array};
use wasm_bindgen::{JsCast, JsValue};

/// Queue of `(id, utf-16-string)` updates flushed via one FFI call.
///
/// The wire shape on the JS side is three args:
///   - `ids: Uint32Array`  — one node id per entry
///   - `joined: string`    — every payload concatenated, no separators
///                           (the JS shim slices by length)
///   - `lengths: Uint32Array` — utf-16 length per entry, matching
///                              JS `String.length` semantics so the
///                              shim can call `joined.substring(off,
///                              off + len)` directly
///
/// Each queue caches its `js_sys::Function` handle after the first
/// flush so subsequent flushes skip the `Reflect::get` lookup.
///
/// **Re-entrancy contract:** the flag is cleared at the start of the
/// microtask body (before the actual flush runs), so updates queued
/// *during* the flush re-schedule a fresh microtask rather than
/// being silently dropped.
pub(crate) struct StringBatchQueue {
    /// Parallel arrays + buffer. `mem::take` and `clear` on flush so
    /// allocations survive across flushes (steady-state alloc cost:
    /// zero).
    ids: Vec<u32>,
    lengths: Vec<u32>,
    buffer: String,
    /// `true` if a microtask flush is already pending this turn.
    /// `Rc` so we can hand a clone to the microtask closure without
    /// pinning `&self`.
    flush_scheduled: Rc<Cell<bool>>,
    /// Cached `window.<js_fn_name>` after first flush.
    flush_fn: Option<Function>,
    /// Name of the JS-side shim function, e.g. `"__idealystApplyClassesBatch"`.
    /// Looked up off `window` on first flush.
    js_fn_name: &'static str,
}

impl StringBatchQueue {
    pub fn new(js_fn_name: &'static str) -> Self {
        Self {
            // Pre-allocate so a SHARED fan-out at hierarchy scale
            // (20k+ subscribers) doesn't grow-realloc its way up
            // from cap 0. The buffers survive across flushes via
            // `mem::take` + `clear`.
            ids: Vec::with_capacity(256),
            lengths: Vec::with_capacity(256),
            buffer: String::with_capacity(8192),
            flush_scheduled: Rc::new(Cell::new(false)),
            flush_fn: None,
            js_fn_name,
        }
    }

    /// Append `(id, payload)` to the buffer. Computes the payload's
    /// utf-16 length once on the ASCII fast-path (most class names +
    /// short labels are ASCII).
    pub fn queue(&mut self, id: u32, payload: &str) {
        let utf16_len = utf16_len(payload);
        self.ids.push(id);
        self.lengths.push(utf16_len);
        self.buffer.push_str(payload);
    }

    /// Append using a writer closure so callers that build the
    /// payload via `format!`-style machinery can write directly into
    /// the queue's buffer instead of producing an intermediate
    /// `String` that gets immediately copied. Mirrors
    /// `text_batch`'s `append_pending_text` pattern.
    pub fn queue_with<F: FnOnce(&mut String)>(&mut self, id: u32, write: F) {
        let start = self.buffer.len();
        write(&mut self.buffer);
        let segment = &self.buffer[start..];
        let utf16_len = utf16_len(segment);
        self.ids.push(id);
        self.lengths.push(utf16_len);
    }

    /// Snapshot the flag for the caller (typically the surface's
    /// `schedule_flush`) to atomically test-and-set without
    /// reaching into private fields. Returns the *previous* value;
    /// if it was already `true`, no scheduling is needed.
    pub fn mark_scheduled(&self) -> bool {
        let was = self.flush_scheduled.get();
        if !was {
            self.flush_scheduled.set(true);
        }
        was
    }

    /// Clone the scheduling flag so the microtask closure can reset
    /// it (and a re-entrant queue call can re-schedule). Cheap —
    /// `Rc<Cell<bool>>` clone is one refcount bump.
    pub fn flush_flag(&self) -> Rc<Cell<bool>> {
        self.flush_scheduled.clone()
    }

    /// Ship every pending entry to the JS-side shim in one FFI call.
    /// Restores the empty `Vec`/`String` containers so their backing
    /// allocation survives across flushes.
    ///
    /// The flush is idempotent on empty queues — callers don't need
    /// to check `is_empty()` before invoking.
    pub fn flush(&mut self) {
        if self.ids.is_empty() {
            return;
        }
        if self.flush_fn.is_none() {
            let window = web_sys::window().expect("no window");
            let f_val = Reflect::get(&window, &JsValue::from_str(self.js_fn_name))
                .unwrap_or_else(|_| panic!("Reflect::get for {} failed", self.js_fn_name));
            self.flush_fn = Some(
                f_val
                    .dyn_into::<Function>()
                    .unwrap_or_else(|_| panic!("{} is not a Function — shim not injected", self.js_fn_name)),
            );
        }
        // Take out so the FFI call doesn't hold `&mut self` against
        // the buffers; restore as empty so capacity survives.
        let ids = std::mem::take(&mut self.ids);
        let lengths = std::mem::take(&mut self.lengths);
        let buffer = std::mem::take(&mut self.buffer);

        let ids_arr = Uint32Array::from(&ids[..]);
        let lengths_arr = Uint32Array::from(&lengths[..]);
        let buffer_js = JsValue::from_str(&buffer);

        let _ = self
            .flush_fn
            .as_ref()
            .expect("set above")
            .call3(&JsValue::NULL, &ids_arr, &buffer_js, &lengths_arr)
            .unwrap_or_else(|_| panic!("{} call failed", self.js_fn_name));

        self.ids = ids;
        self.ids.clear();
        self.lengths = lengths;
        self.lengths.clear();
        self.buffer = buffer;
        self.buffer.clear();
    }
}

/// Compute the utf-16 code-unit length of `s`. ASCII fast-path is
/// O(1) via `is_ascii()` + `len()`; non-ASCII walks `chars()` once.
/// Matches JS `String.length` semantics so the JS shim's
/// `substring(off, off + len)` slicing is correct.
fn utf16_len(s: &str) -> u32 {
    if s.is_ascii() {
        s.len() as u32
    } else {
        s.chars().map(|c| c.len_utf16() as u32).sum()
    }
}

/// Companion to [`StringBatchQueue`] for the release-batch shape:
/// a single `Uint32Array` of ids shipped to a JS shim that walks
/// them and clears each from its registry. Used by every batched
/// surface's release path (text, class, future attribute kinds).
///
/// Same wire shape as `__idealystReleaseTextBatch` already uses —
/// this factors out the lookup+cache+ship plumbing so each surface
/// just calls `IdBatch::flush_to(&mut ..., "__idealystReleaseXxxBatch")`.
pub(crate) struct IdBatch {
    ids: Vec<u32>,
    flush_fn: Option<Function>,
    js_fn_name: &'static str,
}

impl IdBatch {
    pub fn new(js_fn_name: &'static str) -> Self {
        Self {
            ids: Vec::with_capacity(64),
            flush_fn: None,
            js_fn_name,
        }
    }

    pub fn push(&mut self, id: u32) {
        self.ids.push(id);
    }

    /// Ship the entire id buffer in one FFI call, then clear. No-op
    /// if the buffer is empty. Caches the `js_sys::Function` handle
    /// after first flush.
    pub fn flush(&mut self) {
        if self.ids.is_empty() {
            return;
        }
        if self.flush_fn.is_none() {
            let window = web_sys::window().expect("no window");
            let f_val = Reflect::get(&window, &JsValue::from_str(self.js_fn_name))
                .unwrap_or_else(|_| panic!("Reflect::get for {} failed", self.js_fn_name));
            self.flush_fn = Some(
                f_val
                    .dyn_into::<Function>()
                    .unwrap_or_else(|_| panic!("{} is not a Function — shim not injected", self.js_fn_name)),
            );
        }
        let ids = std::mem::take(&mut self.ids);
        let buf = Uint32Array::from(&ids[..]);
        let _ = self
            .flush_fn
            .as_ref()
            .expect("set above")
            .call1(&JsValue::NULL, &buf)
            .unwrap_or_else(|_| panic!("{} call failed", self.js_fn_name));
        self.ids = ids;
        self.ids.clear();
    }
}
