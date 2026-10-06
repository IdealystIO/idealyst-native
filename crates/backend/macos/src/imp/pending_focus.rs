//! Attach-safe `focus()` for the macOS text fields (the
//! `TextInputOps::focus` / `TextAreaOps::focus` contract).
//!
//! `-[NSWindow makeFirstResponder:]` needs a window, and a view realize just
//! built has none: the subtree is inserted after its leaves mount, and a
//! modal's content lands in a portal later still. `focus()` on a windowless
//! field used to be a silent no-op (CrewForge's palette worked around it with a
//! 100 ms timer).
//!
//! A focus on a windowless field is recorded in the shared
//! [`PendingFocus`] slot. The editable views are framework subclasses
//! (`IdealystTextField`, `IdealystSecureTextField`, `IdealystTextView`) whose
//! `viewDidMoveToWindow` calls [`view_moved_to_window`]; when the pending view
//! gets a window, one microtask makes it first responder.
//!
//! Why a microtask rather than inside `viewDidMoveToWindow`: the move happens
//! inside `addSubview:`, i.e. inside the backend's `insert` (the backend is
//! borrowed), and `becomeFirstResponder` on the text view drives
//! `StateBits::FOCUSED` synchronously — the setter re-enters `apply_style` and
//! would abort with "RefCell already borrowed" (the trap documented on
//! `cell_fire_focus` / `fire_focus_state` in `view.rs`). The microtask (on
//! Apple: the main dispatch queue) runs on a clean turn, before the next
//! paint.

use std::cell::{Cell, RefCell};

use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::NSView;
use runtime_shared::primitives::text_input::{FocusRequest, PendingFocus};

thread_local! {
    static PENDING: RefCell<PendingFocus<Retained<NSView>>> = const { RefCell::new(PendingFocus::new()) };
    static DRAIN_QUEUED: Cell<bool> = const { Cell::new(false) };
}

fn window_of(view: &NSView) -> *mut AnyObject {
    unsafe { msg_send![view, window] }
}

fn same(a: &NSView, b: &NSView) -> bool {
    std::ptr::eq(a, b)
}

fn make_first_responder_now(view: &NSView) {
    let window = window_of(view);
    if !window.is_null() {
        let _: bool = unsafe { msg_send![window, makeFirstResponder: view] };
    }
}

/// Focus `target` now if it is in a window, else when it gets one.
pub(crate) fn request_focus(target: &NSView) {
    let retained = unsafe { Retained::retain(target as *const NSView as *mut NSView) }
        .expect("request_focus: retain a live view");
    let attached = !window_of(target).is_null();
    let req = PENDING.with(|p| p.borrow_mut().request(retained, attached));
    if let FocusRequest::Now(view) = req {
        make_first_responder_now(&view);
    }
}

/// A `blur()` on `target`: drop its pending (pre-window) focus.
pub(crate) fn cancel(target: &NSView) {
    PENDING.with(|p| p.borrow_mut().cancel(|v| same(v, target)));
}

/// Called from the editable subclasses' `viewDidMoveToWindow`.
pub(crate) fn view_moved_to_window(view: &NSView) {
    if window_of(view).is_null() {
        return;
    }
    let mine = PENDING.with(|p| p.borrow().is_pending_for(|v| same(v, view)));
    if mine {
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
    let ready = PENDING.with(|p| p.borrow_mut().take_attached(|v| !window_of(v).is_null()));
    if let Some(view) = ready {
        make_first_responder_now(&view);
    }
}

/// Whether a focus is waiting for a window (tests).
#[allow(dead_code)]
pub(crate) fn is_pending() -> bool {
    PENDING.with(|p| p.borrow().is_pending())
}
