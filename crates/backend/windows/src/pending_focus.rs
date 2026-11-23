//! Attach-safe `focus()` for the Win32 fields (the `TextInputOps::focus` /
//! `TextAreaOps::focus` contract).
//!
//! An `EDIT` is a child of the host window from the moment it is created, so
//! `SetFocus` on it always takes, and `EN_SETFOCUS` (the author's `on_focus`)
//! is delivered inside the call. `autofocus` focuses a field while it is
//! still mounting, BEFORE its `Ref` is filled, so focusing there ran
//! `on_focus` against an empty ref. Every other backend's node has no window
//! at that point and reports focus after the mount; so does this one: a field
//! nothing has inserted yet is recorded in the shared [`PendingFocus`] slot
//! and focused one microtask after it is inserted. The microtask, because the
//! insert runs with the backend borrowed and the host drops an `EN_SETFOCUS`
//! that arrives then.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;

use runtime_shared::primitives::text_input::{FocusRequest, PendingFocus};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;

thread_local! {
    /// Fields created and not inserted yet, by HWND.
    static MOUNTING: RefCell<HashSet<isize>> = RefCell::new(HashSet::new());
    static PENDING: RefCell<PendingFocus<isize>> = const { RefCell::new(PendingFocus::new()) };
    static DRAIN_QUEUED: Cell<bool> = const { Cell::new(false) };
}

fn key(hwnd: HWND) -> isize {
    hwnd.0 as isize
}

fn inserted(field: isize) -> bool {
    MOUNTING.with(|m| !m.borrow().contains(&field))
}

fn focus_now(field: isize) {
    unsafe {
        let _ = SetFocus(Some(HWND(field as *mut std::ffi::c_void)));
    }
}

/// A field was created; it is mounting until [`field_inserted`].
pub(crate) fn field_created(hwnd: HWND) {
    MOUNTING.with(|m| m.borrow_mut().insert(key(hwnd)));
}

/// A field was inserted under a parent (or is the root).
pub(crate) fn field_inserted(hwnd: HWND) {
    let field = key(hwnd);
    if !MOUNTING.with(|m| m.borrow_mut().remove(&field)) {
        return;
    }
    if PENDING.with(|p| p.borrow().is_pending_for(|q| *q == field)) && !DRAIN_QUEUED.with(|q| q.replace(true)) {
        runtime_shared::schedule_microtask(drain);
    }
}

/// A field's window is being destroyed. Windows reuses HWND values, so
/// nothing may be kept under this one.
pub(crate) fn field_destroyed(hwnd: HWND) {
    MOUNTING.with(|m| m.borrow_mut().remove(&key(hwnd)));
    cancel(hwnd);
}

/// Focus `hwnd` now if it has been inserted, else once it is.
pub(crate) fn request_focus(hwnd: HWND) {
    let field = key(hwnd);
    let req = PENDING.with(|p| p.borrow_mut().request(field, inserted(field)));
    if let FocusRequest::Now(field) = req {
        focus_now(field);
    }
}

/// A `blur()` on `hwnd`: drop its pending focus.
pub(crate) fn cancel(hwnd: HWND) {
    PENDING.with(|p| p.borrow_mut().cancel(|q| *q == key(hwnd)));
}

fn drain() {
    DRAIN_QUEUED.with(|q| q.set(false));
    let ready = PENDING.with(|p| p.borrow_mut().take_attached(|f| inserted(*f)));
    if let Some(field) = ready {
        focus_now(field);
    }
}
