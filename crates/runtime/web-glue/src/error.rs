//! Exceptions: a JS throw inside a `#[catch]` import becomes
//! `Err(JsError)`.
//!
//! The JS side (`G.catching`) wraps the snippet in try/catch. On a throw
//! it parks the thrown value in a two-word error slot that lives in Rust
//! memory — `[flag, handle]` — and returns 0. The generated wrapper checks
//! the flag after every catching call ([`take_pending`]). Signalling
//! through memory rather than a second import keeps the success path at
//! one boundary crossing: a load and a branch, no "did it throw?" call.
//!
//! A separate flag word (instead of "handle != 0") because `throw
//! undefined` is legal JS and handle 0 IS undefined.

use std::cell::Cell;
use std::fmt;

use crate::{ffi, string, JsValue};

thread_local! {
    static SLOT: Cell<[u32; 2]> = const { Cell::new([0, 0]) };
}

/// Address of the error slot. Exported; `G.attach` reads it once.
#[unsafe(no_mangle)]
pub extern "C" fn __glue_err_slot() -> usize {
    SLOT.with(|s| s.as_ptr() as usize)
}

/// The error a catching import parked, if it threw. Clears the slot.
#[doc(hidden)]
#[inline]
pub fn take_pending() -> Option<JsError> {
    SLOT.with(|s| {
        let [flag, handle] = s.get();
        if flag == 0 {
            return None;
        }
        s.set([0, 0]);
        // SAFETY: `G.catching` minted `handle` with `G.add` for us alone.
        Some(JsError { value: unsafe { JsValue::from_raw(handle) } })
    })
}

/// Park `handle` as the pending error — the mock host's `G.catching`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn park(handle: u32) {
    SLOT.with(|s| s.set([1, handle]));
}

/// A value JS threw.
pub struct JsError {
    value: JsValue,
}

impl JsError {
    /// The thrown value itself.
    pub fn value(&self) -> &JsValue {
        &self.value
    }

    pub fn into_value(self) -> JsValue {
        self.value
    }

    /// `"Name: message"` for an `Error`, `String(value)` otherwise.
    pub fn message(&self) -> String {
        string::receive(|out| unsafe { ffi::error_message(self.value.raw(), out) })
    }
}

impl From<JsValue> for JsError {
    fn from(value: JsValue) -> JsError {
        JsError { value }
    }
}

impl fmt::Debug for JsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JsError({})", self.message())
    }
}

/// A JS exception IS a JS value — as with wasm-bindgen, where `catch`
/// imports return `Result<_, JsValue>` — so `&err` passes wherever a
/// `&JsValue` is wanted (logging it, rethrowing it).
impl std::ops::Deref for JsError {
    type Target = JsValue;
    fn deref(&self) -> &JsValue {
        self.value()
    }
}

impl fmt::Display for JsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for JsError {}
