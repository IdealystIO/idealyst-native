//! A focused field's `on_key_down`, and Escape dismissing the topmost
//! overlay — the two key behaviors web and Windows give and AppKit has no
//! per-control hook for.
//!
//! One `NSEvent` local monitor for key-downs, installed the first time a
//! field registers a handler or a portal registers a dismisser. For each
//! key-down, in the DOM's order:
//!
//! 1. **Field `on_key_down`.** The event window's first responder is a
//!    field editor (an `NSTextView` with `isFieldEditor`) whose delegate is
//!    a registered `text_input`'s `NSTextField`, or a registered
//!    `text_area`'s own `NSTextView` (or the scroll view hosting it). The
//!    handler gets the key from the app-level source's own translation
//!    (`imp::keyboard::translate`) and the editor's selection
//!    (`selectedRange`, UTF-16 like every platform's). `PreventDefault`
//!    returns `nil`: the event is swallowed, so the field never types it.
//! 2. **Escape → topmost `on_dismiss`.** A not-swallowed Escape calls the
//!    `on_dismiss` of the most recently mounted portal that is shown in the
//!    event's window, and is swallowed (the field editor would otherwise
//!    run `cancelOperation:`, which pops up completions). Mirrors the
//!    Windows host's `escape_dismiss`.
//!
//! Registrations hold the view weakly and are pruned when it dies; a
//! portal's dismisser is also removed when the portal is released. Author
//! handlers run with no registry borrow held (a handler may mount or
//! release a portal, which re-enters the registry), and the monitor never
//! touches the backend.

use std::cell::RefCell;
use std::rc::Rc;

use block2::RcBlock;
use objc2::rc::{Retained, Weak};
use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use objc2_app_kit::NSView;
use objc2_foundation::NSObject;
use runtime_shared::primitives::key::{KeyDownHandler, KeyEvent, KeyOutcome};

use crate::imp::keyboard::{read_raw, translate, RawKind};
use crate::imp::MacosNode;

/// `NSEventMaskKeyDown`.
const MASK_KEY_DOWN: usize = 1 << 10;

#[derive(Default)]
struct Registry {
    fields: Vec<(Weak<NSView>, KeyDownHandler)>,
    /// Mount order: the last shown one is the topmost.
    portals: Vec<(usize, Weak<NSView>, Rc<dyn Fn()>)>,
    monitor: Option<Retained<AnyObject>>,
}

thread_local! {
    static REGISTRY: RefCell<Registry> = RefCell::new(Registry::default());
}

/// Route key-downs in `node` (a `text_input`'s field or a `text_area`) to
/// `handler` while the field has focus.
pub(crate) fn register_field(node: &MacosNode, handler: KeyDownHandler) {
    let weak = Weak::from(node.as_view());
    REGISTRY.with(|r| r.borrow_mut().fields.push((weak, handler)));
    ensure_monitor();
}

/// Escape in the portal's window calls `dismiss` while the portal is the
/// topmost shown one.
pub(crate) fn register_portal(node: &MacosNode, dismiss: Rc<dyn Fn()>) {
    let weak = Weak::from(node.as_view());
    REGISTRY.with(|r| r.borrow_mut().portals.push((node.view_key(), weak, dismiss)));
    ensure_monitor();
}

/// The portal was released: its Escape dismissal goes with it.
pub(crate) fn unregister_portal(node: &MacosNode) {
    let key = node.view_key();
    REGISTRY.with(|r| r.borrow_mut().portals.retain(|(k, _, _)| *k != key));
}

fn ensure_monitor() {
    if REGISTRY.with(|r| r.borrow().monitor.is_some()) {
        return;
    }
    let block = RcBlock::new(|event: *mut NSObject| -> *mut NSObject {
        let swallow = backend_apple_core::crash::abort_on_panic("field key monitor", || unsafe { on_key_down(event) });
        if swallow {
            std::ptr::null_mut()
        } else {
            event
        }
    });
    let monitor: *mut AnyObject = unsafe {
        msg_send![class!(NSEvent), addLocalMonitorForEventsMatchingMask: MASK_KEY_DOWN, handler: &*block]
    };
    let monitor = unsafe { Retained::retain(monitor) };
    REGISTRY.with(|r| r.borrow_mut().monitor = monitor);
}

/// The monitor body; `true` swallows the event.
unsafe fn on_key_down(event: *mut NSObject) -> bool {
    let Some(raw) = read_raw(event) else { return false };
    if raw.kind != RawKind::KeyDown {
        return false;
    }
    let Some(ev) = translate(&raw) else { return false };
    let mut window: *mut AnyObject = msg_send![event, window];
    if window.is_null() {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        window = msg_send![app, keyWindow];
    }
    if window.is_null() {
        return false;
    }

    // 1. The focused field's own handler.
    let responder: *mut AnyObject = msg_send![window, firstResponder];
    if let Some((handler, editor)) = field_handler(responder) {
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
        if handler(&key_event) == KeyOutcome::PreventDefault {
            return true;
        }
    }

    // 2. Escape closes the topmost overlay in this window.
    if ev.key == "Escape" {
        if let Some(dismiss) = topmost_dismiss(window) {
            dismiss();
            return true;
        }
    }
    false
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
