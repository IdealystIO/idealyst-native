//! Attach-safe `focus()` for the iOS text fields (the
//! `TextInputOps::focus` / `TextAreaOps::focus` contract).
//!
//! `-[UIResponder becomeFirstResponder]` returns NO for a view with no
//! window, and a view realize just built has none: the subtree is inserted
//! after its leaves mount, and a portal's container joins the window on a
//! later run-loop turn (`portal.rs`). `focus()` right after mount used to be
//! a silent no-op.
//!
//! A focus on a windowless field is recorded in the shared [`PendingFocus`]
//! slot. The editable views are framework subclasses (`IdealystTextField`,
//! `IdealystTextView`) whose `didMoveToWindow` calls
//! [`view_moved_to_window`]; when the pending view gets a window, one
//! microtask makes it first responder.
//!
//! Why a microtask rather than inside `didMoveToWindow`: the move happens
//! inside `addSubview:`, i.e. inside the backend's `insert` (the backend is
//! borrowed), and `IdealystTextField::becomeFirstResponder` drives
//! `StateBits::FOCUSED` synchronously — the setter re-enters `apply_style`
//! and would abort with "RefCell already borrowed". The microtask (on Apple:
//! the main dispatch queue) runs on a clean turn, before the next paint.

use std::cell::{Cell, RefCell};

use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_ui_kit::UIView;
use runtime_shared::primitives::text_input::{FocusRequest, PendingFocus};

thread_local! {
    static PENDING: RefCell<PendingFocus<Retained<UIView>>> = const { RefCell::new(PendingFocus::new()) };
    static DRAIN_QUEUED: Cell<bool> = const { Cell::new(false) };
}

fn has_window(view: &UIView) -> bool {
    let window: *mut AnyObject = unsafe { msg_send![view, window] };
    !window.is_null()
}

fn become_first_responder(view: &UIView) {
    let _: bool = unsafe { msg_send![view, becomeFirstResponder] };
}

/// Focus `view` now if it is in a window, else when it gets one.
pub(crate) fn request_focus(view: &UIView) {
    let retained = unsafe { Retained::retain(view as *const UIView as *mut UIView) }
        .expect("request_focus: retain a live view");
    let attached = has_window(view);
    let req = PENDING.with(|p| p.borrow_mut().request(retained, attached));
    if let FocusRequest::Now(view) = req {
        become_first_responder(&view);
    }
}

/// A `blur()` on `view`: drop its pending (pre-window) focus.
pub(crate) fn cancel(view: &UIView) {
    PENDING.with(|p| p.borrow_mut().cancel(|v| std::ptr::eq(&**v, view)));
}

/// Called from the editable subclasses' `didMoveToWindow`.
pub(crate) fn view_moved_to_window(view: &UIView) {
    if !has_window(view) {
        return;
    }
    if PENDING.with(|p| p.borrow().is_pending_for(|v| std::ptr::eq(&**v, view))) {
        queue_drain();
    }
}

fn queue_drain() {
    if DRAIN_QUEUED.with(|q| q.replace(true)) {
        return;
    }
    runtime_shared::schedule_microtask(drain);
}

fn drain() {
    DRAIN_QUEUED.with(|q| q.set(false));
    let ready = PENDING.with(|p| p.borrow_mut().take_attached(|v| has_window(v)));
    if let Some(view) = ready {
        become_first_responder(&view);
    }
}
