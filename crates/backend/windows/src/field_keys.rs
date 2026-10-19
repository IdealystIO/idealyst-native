//! `on_key_down` for `text_input` / `text_area`.
//!
//! A field is a child `EDIT` window: its key messages go to the edit's own
//! window procedure, never the host's. Every key message for every window
//! of the thread passes through the host's message pump, though (the seam
//! `app_keys` uses), so the host offers each one here first, before the
//! app-level keyboard — the DOM's order, where the focused input's
//! `keydown` runs before window listeners.
//!
//! `KeyOutcome::PreventDefault` makes the pump skip translating and
//! dispatching the message: the edit never sees the key, so nothing is
//! typed (and Enter in a text area inserts no line break).

use std::cell::RefCell;

use runtime_shared::primitives::key::{KeyEvent, KeyOutcome};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{SendMessageW, MSG, WM_KEYDOWN, WM_SYSKEYDOWN};

use crate::WindowsBackend;

/// Offer a retrieved message to the focused field's `on_key_down`. `true`
/// = the handler prevented the default: skip `TranslateMessage` and
/// `DispatchMessageW`. A busy backend (re-entrant pump) declines.
pub fn pre_dispatch(cell: &RefCell<WindowsBackend>, msg: &MSG) -> bool {
    if msg.message != WM_KEYDOWN && msg.message != WM_SYSKEYDOWN {
        return false;
    }
    let handler = match cell.try_borrow() {
        Ok(b) => b.field_keys.get(&(msg.hwnd.0 as isize)).cloned(),
        Err(_) => return false,
    };
    let Some(handler) = handler else { return false };
    let key = unsafe { crate::app_keys::translate(true, msg.wParam.0 as u32, msg.lParam.0) };
    let (start, end) = selection(msg.hwnd);
    let ev = KeyEvent {
        key: key.key,
        shift: key.shift,
        ctrl: key.ctrl,
        alt: key.alt,
        meta: key.meta,
        selection_start: start,
        selection_end: end,
    };
    matches!(handler(&ev), KeyOutcome::PreventDefault)
}

/// The edit's selection `(start, end)` in UTF-16 units (`EM_GETSEL`).
fn selection(hwnd: windows::Win32::Foundation::HWND) -> (usize, usize) {
    let mut start = 0u32;
    let mut end = 0u32;
    unsafe {
        SendMessageW(
            hwnd,
            crate::EM_GETSEL,
            Some(WPARAM(&mut start as *mut u32 as usize)),
            Some(LPARAM(&mut end as *mut u32 as isize)),
        );
    }
    (start as usize, end as usize)
}
