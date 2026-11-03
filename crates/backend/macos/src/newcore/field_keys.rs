//! A focused field's `on_key_down`, and Escape dismissing the topmost
//! overlay — the two key behaviors web and Windows give and AppKit has no
//! per-control hook for.
//!
//! Both run from the backend's one key monitor (`imp::keyboard`), which is
//! installed the first time a field registers a handler or a portal
//! registers a dismisser. For each key-down, in the DOM's order:
//!
//! 1. **Field `on_key_down`** ([`field_prevents`]). The event window's first
//!    responder is a field editor (an `NSTextView` with `isFieldEditor`)
//!    whose delegate is a registered `text_input`'s `NSTextField`, or a
//!    registered `text_area`'s own `NSTextView` (or the scroll view hosting
//!    it). The handler gets the key from the app-level source's own
//!    translation (`imp::keyboard::translate`) and the editor's selection
//!    (`selectedRange`, UTF-16 like every platform's). `PreventDefault`
//!    swallows the event, so the field never types it — the app-level
//!    `on_key` listeners still hear it, as a `document` listener does on web.
//! 2. **Escape → topmost `on_dismiss`** ([`escape_dismisses`]). An Escape the
//!    field didn't prevent calls the `on_dismiss` of the most recently
//!    mounted portal that is shown in the event's window, and is swallowed
//!    (the field editor would otherwise run `cancelOperation:`, which pops
//!    up completions). Mirrors the Windows host's `escape_dismiss`.
//!
//! Registrations hold the view weakly and are pruned when it dies; a
//! portal's dismisser is also removed when the portal is released. Author
//! handlers run with no registry borrow held (a handler may mount or
//! release a portal, which re-enters the registry), and nothing here
//! touches the backend.

use std::cell::RefCell;
use std::rc::Rc;

use objc2::rc::{Retained, Weak};
use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use objc2_app_kit::NSView;
use objc2_foundation::NSObject;
use runtime_shared::primitives::key::{AppKeyEvent, KeyDownHandler, KeyEvent, KeyOutcome};

use crate::imp::keyboard::ensure_key_monitor;
use crate::imp::MacosNode;

#[derive(Default)]
struct Registry {
    fields: Vec<(Weak<NSView>, KeyDownHandler)>,
    /// Mount order: the last shown one is the topmost.
    portals: Vec<(usize, Weak<NSView>, Rc<dyn Fn()>)>,
}

thread_local! {
    static REGISTRY: RefCell<Registry> = RefCell::new(Registry::default());
}

/// Route key-downs in `node` (a `text_input`'s field or a `text_area`) to
/// `handler` while the field has focus.
pub(crate) fn register_field(node: &MacosNode, handler: KeyDownHandler) {
    let weak = Weak::from(node.as_view());
    REGISTRY.with(|r| r.borrow_mut().fields.push((weak, handler)));
    ensure_key_monitor();
}

/// Escape in the portal's window calls `dismiss` while the portal is the
/// topmost shown one.
pub(crate) fn register_portal(node: &MacosNode, dismiss: Rc<dyn Fn()>) {
    let weak = Weak::from(node.as_view());
    REGISTRY.with(|r| r.borrow_mut().portals.push((node.view_key(), weak, dismiss)));
    ensure_key_monitor();
}

/// The portal was released: its Escape dismissal goes with it.
pub(crate) fn unregister_portal(node: &MacosNode) {
    let key = node.view_key();
    REGISTRY.with(|r| r.borrow_mut().portals.retain(|(k, _, _)| *k != key));
}

/// The window a key event is for: its own, else the key window. Null when
/// there is neither.
pub(crate) unsafe fn event_window(event: *mut NSObject) -> *mut AnyObject {
    let window: *mut AnyObject = msg_send![event, window];
    if !window.is_null() {
        return window;
    }
    let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
    msg_send![app, keyWindow]
}

/// Run the `on_key_down` of the field focused in `window` for the key-down
/// `ev`; `true` when it prevented the key (the event must be swallowed).
pub(crate) unsafe fn field_prevents(window: *mut AnyObject, ev: &AppKeyEvent) -> bool {
    if window.is_null() {
        return false;
    }
    let responder: *mut AnyObject = msg_send![window, firstResponder];
    let Some((handler, editor)) = field_handler(responder) else { return false };
    let range: objc2_foundation::NSRange = msg_send![editor, selectedRange];
    let key_event = KeyEvent {
        key: ev.key.clone(),
        shift: ev.shift,
        ctrl: ev.ctrl,
        alt: ev.alt,
        meta: ev.meta,
        selection_start: range.location,
        selection_end: range.location + range.length,
    };
    handler(&key_event) == KeyOutcome::PreventDefault
}

/// For an Escape key-down, call the dismisser of the topmost overlay shown
/// in `window`; `true` when one ran (the event must be swallowed).
pub(crate) unsafe fn escape_dismisses(window: *mut AnyObject, ev: &AppKeyEvent) -> bool {
    if window.is_null() || ev.key != "Escape" {
        return false;
    }
    let Some(dismiss) = topmost_dismiss(window) else { return false };
    dismiss();
    true
}

/// The registered handler for the field `responder` is editing, with the
/// text view whose selection is the caret. Prunes dead registrations.
unsafe fn field_handler(responder: *mut AnyObject) -> Option<(KeyDownHandler, *mut AnyObject)> {
    if responder.is_null() {
        return None;
    }
    let is_text_view: bool = msg_send![responder, isKindOfClass: class!(NSTextView)];
    if !is_text_view {
        return None;
    }
    let is_field_editor: bool = msg_send![responder, isFieldEditor];
    let candidates: [*mut AnyObject; 2] = if is_field_editor {
        let delegate: *mut AnyObject = msg_send![responder, delegate];
        [delegate, std::ptr::null_mut()]
    } else {
        let scroll: *mut AnyObject = msg_send![responder, enclosingScrollView];
        [responder, scroll]
    };
    let handler = REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        let mut found = None;
        r.fields.retain(|(weak, handler)| match weak.load() {
            None => false,
            Some(view) => {
                let ptr = &*view as *const NSView as *mut AnyObject;
                if found.is_none() && candidates.iter().any(|c| !c.is_null() && *c == ptr) {
                    found = Some(handler.clone());
                }
                true
            }
        });
        found
    })?;
    Some((handler, responder))
}

/// The dismisser of the most recently mounted portal shown in `window`.
unsafe fn topmost_dismiss(window: *mut AnyObject) -> Option<Rc<dyn Fn()>> {
    let live: Vec<(Retained<NSView>, Rc<dyn Fn()>)> = REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        r.portals.retain(|(_, weak, _)| weak.load().is_some());
        r.portals.iter().filter_map(|(_, weak, d)| Some((weak.load()?, d.clone()))).collect()
    });
    live.into_iter().rev().find_map(|(view, dismiss)| {
        let in_window: *mut AnyObject = msg_send![&*view, window];
        let hidden: bool = msg_send![&*view, isHiddenOrHasHiddenAncestor];
        (in_window == window && !hidden).then_some(dismiss)
    })
}
