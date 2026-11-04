//! Attach-safe `focus()` for the web text fields (the
//! `TextInputOps::focus` / `TextAreaOps::focus` contract).
//!
//! `HTMLElement.focus()` on an element that is not in the document does
//! nothing — no error, no deferred effect. Realize builds a subtree before
//! inserting it, and a modal's content mounts into a portal, so a `focus()`
//! from `autofocus` or an `on_handle` fill right after mount would land
//! on a disconnected node and vanish.
//!
//! A focus on a disconnected field is recorded here instead. The backend's
//! attach points (`insert`, `insert_many`, `insert_at`, the root mount)
//! call [`node_attached`], which — only while something is pending —
//! queues one microtask that focuses the pending field if it is now
//! connected, and otherwise keeps waiting. The focus itself is deferred to that
//! microtask rather than run inside the insert: an insert runs under
//! `&mut WebBackend`, and `focus()` fires the field's `focus` listener
//! synchronously, whose author `on_focus` may restyle (re-enter the
//! backend). The microtask runs after the current flush has returned.
//!
//! The slot logic (latest request wins, blur cancels, taken once on
//! attach) is the shared [`PendingFocus`] every backend drives.

use std::cell::{Cell, RefCell};

use runtime_shared::primitives::text_input::{FocusRequest, PendingFocus};
use web_glue::dom::HtmlElement;

thread_local! {
    static PENDING: RefCell<PendingFocus<HtmlElement>> = const { RefCell::new(PendingFocus::new()) };
    static DRAIN_QUEUED: Cell<bool> = const { Cell::new(false) };
}

/// Focus `el` now if it's in the document, else when it gets there.
pub(crate) fn request_focus(el: &HtmlElement) {
    let req = PENDING.with(|p| p.borrow_mut().request(el.clone(), el.is_connected()));
    match req {
        FocusRequest::Now(el) => moving_focus(|| {
            let _ = el.focus();
        }),
        // The common case — realize inserts the subtree later in the same
        // synchronous flush — is caught by this microtask without waiting
        // for an attach hook.
        FocusRequest::Deferred => queue_drain(),
    }
}

/// Drop the pending focus if it is `el`'s (a `blur()` before attach).
pub(crate) fn cancel(el: &HtmlElement) {
    PENDING.with(|p| p.borrow_mut().cancel(|q| q.is_same_node(Some(el.as_ref()))));
}

/// A backend attach point ran: if a focus is waiting, check it once the
/// current flush is done. One TLS read when nothing is pending.
pub(crate) fn node_attached() {
    if PENDING.with(|p| p.borrow().is_pending()) {
        queue_drain();
    }
}

fn queue_drain() {
    if DRAIN_QUEUED.with(|q| q.replace(true)) {
        return;
    }
    web_glue::queue_microtask(drain);
}

fn drain() {
    DRAIN_QUEUED.with(|q| q.set(false));
    let ready = PENDING.with(|p| p.borrow_mut().take_attached(|el| el.is_connected()));
    // Outside the borrow: `focus()` fires listeners synchronously, and an
    // author handler may request another focus.
    if let Some(el) = ready {
        moving_focus(|| {
            let _ = el.focus();
        });
    }
}

/// Run a programmatic focus move, then deliver the focus events the browser
/// withheld.
///
/// A page without OS window focus (`document.hasFocus()` false) moves
/// `activeElement` on `focus()` / `blur()` and fires no event. That is every
/// page a robot drives in the background, and headless Chrome outside
/// `idealyst test`'s focus emulation, so a field that commits on blur shows
/// the typed value and never saves. A page with window focus got its native
/// events and gets nothing extra.
///
/// Robot builds only: a person's browser delivers these itself when the
/// window comes back, and a synthetic pair would run the handlers twice.
#[cfg(feature = "robot")]
pub(crate) fn moving_focus(move_focus: impl FnOnce()) {
    let Some(doc) = web_glue::dom::window().and_then(|w| w.document()) else {
        return move_focus();
    };
    let before = doc.active_element();
    move_focus();
    if doc.has_focus() {
        return;
    }
    let after = doc.active_element();
    let same = match (&before, &after) {
        (Some(b), Some(a)) => b.is_same_node(Some(a.as_ref())),
        (None, None) => true,
        _ => false,
    };
    if same {
        return;
    }
    // `activeElement` falls back to `<body>` when nothing holds focus; the
    // body never received focus, so it gets no event.
    let body = doc.body();
    let real = |el: Option<web_glue::dom::Element>| {
        el.filter(|e| !body.as_ref().is_some_and(|b| b.is_same_node(Some(e.as_ref()))))
    };
    if let Some(left) = real(before) {
        dispatch_focus_event(&left, "blur", false);
        dispatch_focus_event(&left, "focusout", true);
    }
    if let Some(entered) = real(after) {
        dispatch_focus_event(&entered, "focus", false);
        dispatch_focus_event(&entered, "focusin", true);
    }
}

#[cfg(not(feature = "robot"))]
#[inline]
pub(crate) fn moving_focus(move_focus: impl FnOnce()) {
    move_focus();
}

#[cfg(feature = "robot")]
fn dispatch_focus_event(target: &web_glue::dom::Element, ty: &str, bubbles: bool) {
    let init = web_glue::dom::EventInit::new();
    init.set_bubbles(bubbles);
    if let Ok(ev) = web_glue::dom::Event::new_with_event_init_dict(ty, &init) {
        let _ = target.dispatch_event(&ev);
    }
}

/// Whether a field is waiting for attach (tests).
#[cfg(test)]
pub(crate) fn is_pending() -> bool {
    PENDING.with(|p| p.borrow().is_pending())
}

/// Clear the slot between tests (a previous test's field must not linger).
#[cfg(test)]
pub(crate) fn reset_for_test() {
    PENDING.with(|p| *p.borrow_mut() = PendingFocus::new());
}
