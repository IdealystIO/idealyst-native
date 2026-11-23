//! Windows: a power request for the display (`powercfg /requests` lists it).
//!
//! Not `SetThreadExecutionState`: that state belongs to the calling thread
//! and is one value per thread, so another library's call on the UI thread
//! would overwrite this one. A power request is the process's own object.

use std::cell::Cell;

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Power::{
    PowerClearRequest, PowerCreateRequest, PowerRequestDisplayRequired, PowerSetRequest,
};
use windows::Win32::System::Threading::{
    POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT, REASON_CONTEXT_0,
};

/// `POWER_REQUEST_CONTEXT_VERSION` (`winnt.h`).
const CONTEXT_VERSION: u32 = 0;

runtime_tls::thread_local! {
    static HELD: Cell<Option<HANDLE>> = const { Cell::new(None) };
}

pub(crate) fn set(on: bool) {
    let held = HELD.with(Cell::get);
    match (on, held) {
        (true, None) => {
            if let Some(request) = request_display() {
                HELD.with(|h| h.set(Some(request)));
            }
        }
        (false, Some(request)) => {
            unsafe {
                let _ = PowerClearRequest(request, PowerRequestDisplayRequired);
                let _ = CloseHandle(request);
            }
            HELD.with(|h| h.set(None));
        }
        _ => {}
    }
}

fn request_display() -> Option<HANDLE> {
    let mut reason: Vec<u16> =
        crate::keep_awake_labels().reason.encode_utf16().chain(std::iter::once(0)).collect();
    let context = REASON_CONTEXT {
        Version: CONTEXT_VERSION,
        Flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING,
        Reason: REASON_CONTEXT_0 { SimpleReasonString: PWSTR(reason.as_mut_ptr()) },
    };
    unsafe {
        let request = PowerCreateRequest(&context).ok()?;
        if PowerSetRequest(request, PowerRequestDisplayRequired).is_err() {
            let _ = CloseHandle(request);
            return None;
        }
        Some(request)
    }
}

pub(crate) fn is_supported() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The request is made and cleared against the real power manager.
    /// `powercfg /requests`, which lists it, needs an administrator.
    #[test]
    fn a_request_is_held_until_cleared() {
        set(true);
        let first = HELD.with(Cell::get);
        assert!(first.is_some(), "the power manager took the request");
        set(true);
        assert_eq!(HELD.with(Cell::get), first, "asking again keeps it");
        set(false);
        assert!(HELD.with(Cell::get).is_none());
        set(false);
    }
}
