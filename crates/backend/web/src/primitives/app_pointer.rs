//! The APP-LEVEL pointer source (`AppEnvOps::set_pointer_sink`), pointer
//! lock (`set_pointer_lock`) and cursor hiding (`set_cursor_hidden`) — the
//! web half of `runtime_shared::pointer_input`. Lives outside every
//! primitive gate: like the app-level keyboard it is an app capability,
//! not part of a view.
//!
//! ## Listeners (all on `document`, CAPTURE phase)
//!
//! Capture on the document runs before any view's own handler, so a view
//! that stops propagation can't hide a press from the app, and a locked
//! pointer (whose events target the lock element) is covered the same way.
//!
//! - `mousedown` / `mouseup` → `Down` / `Up`. MOUSE events, not pointer
//!   events, for two reasons: a pointer stream reports a second button
//!   pressed while one is held (a chord) as `pointermove`, not
//!   `pointerdown`; and cancelling `pointerdown` does not cancel the
//!   focus change / text selection — those are `mousedown`'s default
//!   actions, so that is the event `PreventDefault` must cancel.
//! - `pointermove` → `Move` (`movementX/Y` for `dx`/`dy`; the pointer
//!   stream keeps firing while locked). `pointerdown` + `pointermove` also
//!   record the latest `pointerType`, because the compatibility
//!   `mousedown`/`mouseup` a browser synthesizes after a finger tap must
//!   be skipped — finger touches belong to the view-level touch APIs.
//! - `wheel` (`passive: false`, the only way `preventDefault` can stop the
//!   page scrolling) → `Wheel`, line/page units converted by
//!   `app_pointer_policy::wheel_px`.
//! - `contextmenu` → cancelled when the press that opens it was prevented
//!   (secondary, or ctrl+primary on macOS), and always while the pointer
//!   is locked to the app (a right-click in a locked game must not open a
//!   menu over it).
//! - `pointerlockchange` / `pointerlockerror` → `lock_changed`.
//! - window `blur` → `focus_lost` (the keyboard source's hook: the browser
//!   never delivers the `mouseup` for a button held across an alt-tab).
//!
//! ## Pointer lock
//!
//! `requestPointerLock()` on the app's mount element, synchronously inside
//! `set_pointer_lock(true)` — the dispatcher calls it from the author's
//! click / key handler, and browsers honor the request only during such a
//! user gesture. The outcome is asynchronous: `pointerlockchange` (granted
//! or lost — Esc, tab switch) or `pointerlockerror` (refused). No
//! `unadjustedMovement` option is requested: Chrome rejects it on
//! platforms without raw input instead of falling back, and the other
//! engines don't implement it, so `dx`/`dy` are the browser's (accelerated)
//! movement on every engine.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use runtime_shared::pointer_input::{AppPointerEvent, PointerOutcome, PointerPhase, PointerSink};
use web_glue::dom::{Document, Element, ListenerOptions, MouseEvent, PointerEvent, WheelEvent};
use web_glue::JsCast;

use crate::app_pointer_policy as policy;
use crate::WebBackend;

/// Attribute that hides the cursor over the element and its descendants
/// (see [`CURSOR_HIDDEN_CSS`]).
const CURSOR_HIDDEN_ATTR: &str = "data-idealyst-cursor-hidden";
/// `id` of the one `<style>` element carrying [`CURSOR_HIDDEN_CSS`].
const CURSOR_HIDDEN_STYLE_ID: &str = "__idealyst_cursor_hidden";
/// A rule, not an inline `cursor: none` on the root: `cursor` is inherited
/// only by elements that don't set their own, so a button's
/// `cursor: pointer` would show through. `!important` on every
/// descendant beats those.
const CURSOR_HIDDEN_CSS: &str =
    "[data-idealyst-cursor-hidden], [data-idealyst-cursor-hidden] * { cursor: none !important; }";

/// State the listeners share.
struct Shared {
    /// Swapped in place when the dispatcher replaces the sink, so the
    /// listeners (and a held lock) survive the replacement.
    sink: RefCell<PointerSink>,
    doc: Document,
    mount: Element,
    /// The latest `pointerType` was `"touch"` — skip the compatibility
    /// mouse events that follow a finger tap.
    last_was_touch: Cell<bool>,
    /// The last down was prevented and opens a context menu: cancel the
    /// `contextmenu` that follows it.
    suppress_menu: Cell<bool>,
    /// The app currently wants the lock. A grant that arrives after the
    /// app already released (request → release → grant) is undone instead
    /// of reported.
    want_lock: Cell<bool>,
}

impl Shared {
    fn sink(&self) -> PointerSink {
        self.sink.borrow().clone()
    }

    fn locked_to_us(&self) -> bool {
        locked_to(&self.doc, &self.mount)
    }

    fn event_from(&self, me: &MouseEvent, phase: PointerPhase) -> AppPointerEvent {
        // App-root relative. While locked the browser freezes clientX/Y at
        // the lock point, which is exactly the contract's "position stays".
        let rect = self.mount.get_bounding_client_rect();
        let mut e = AppPointerEvent::new(
            phase,
            (me.client_x() as f64 - rect.left()) as f32,
            (me.client_y() as f64 - rect.top()) as f32,
        );
        e.shift = me.shift_key();
        e.ctrl = me.ctrl_key();
        e.alt = me.alt_key();
        e.meta = me.meta_key();
        e
    }
}

fn locked_to(doc: &Document, mount: &Element) -> bool {
    doc.pointer_lock_element().is_some_and(|el| AsRef::<web_glue::JsValue>::as_ref(&el) == mount.as_ref())
}

/// The installed source. Dropping it (sink `None`, a replaced backend, app
/// shutdown) detaches every listener, shows the cursor again and releases
/// a lock the app holds.
pub(crate) struct AppPointerSource {
    shared: Rc<Shared>,
    _listeners: Vec<web_glue::dom::Listener>,
}

impl Drop for AppPointerSource {
    fn drop(&mut self) {
        apply_cursor_hidden(&self.shared.doc, &self.shared.mount, false);
        if self.shared.locked_to_us() {
            self.shared.doc.exit_pointer_lock();
        }
        // `_listeners` drop after this body; each detaches as it drops.
    }
}

/// Install (`Some`) or remove (`None`) the app-level pointer source.
/// Replacing an installed source only swaps the sink.
pub(crate) fn install_pointer_sink(b: &mut WebBackend, sink: Option<PointerSink>) {
    let Some(sink) = sink else {
        b.app_pointer = None;
        return;
    };
    if let Some(src) = &b.app_pointer {
        *src.shared.sink.borrow_mut() = sink;
        return;
    }
    let shared = Rc::new(Shared {
        sink: RefCell::new(sink),
        doc: b.doc.clone(),
        mount: b.mount.clone(),
        last_was_touch: Cell::new(false),
        suppress_menu: Cell::new(false),
        want_lock: Cell::new(false),
    });
    let mut listeners = Vec::new();
    let capture = ListenerOptions::CAPTURE;
    // `listen_fn` (re-entrant): an author handler may synchronously cause
    // another of these events (`el.click()` from a mousedown handler).
    let doc = &b.doc;

    let s = shared.clone();
    listeners.push(crate::glue_dom::listen_fn(doc, "pointerdown", capture, move |e| {
        let pe: PointerEvent = e.unchecked_into();
        s.last_was_touch.set(policy::is_touch(&pe.pointer_type()));
    }));

    let s = shared.clone();
    listeners.push(crate::glue_dom::listen_fn(doc, "pointermove", capture, move |e| {
        let pe: PointerEvent = e.unchecked_into();
        let touch = policy::is_touch(&pe.pointer_type());
        s.last_was_touch.set(touch);
        if touch {
            return;
        }
        let me: &MouseEvent = &pe;
        let mut ev = s.event_from(me, PointerPhase::Move);
        ev.dx = me.movement_x() as f32;
        ev.dy = me.movement_y() as f32;
        // A move has no default worth cancelling; the outcome is ignored.
        let _ = s.sink().event(&ev);
    }));

    for (ty, phase) in [("mousedown", PointerPhase::Down), ("mouseup", PointerPhase::Up)] {
        let s = shared.clone();
        listeners.push(crate::glue_dom::listen_fn(doc, ty, capture, move |e| {
            if s.last_was_touch.get() {
                return;
            }
            let me: MouseEvent = e.unchecked_into();
            let Some(button) = policy::mouse_button(me.button()) else {
                return;
            };
            let mut ev = s.event_from(&me, phase);
            ev.button = Some(button);
            let prevented = s.sink().event(&ev) == PointerOutcome::PreventDefault;
            if prevented {
                // Down: no focus change / text selection / middle-click
                // autoscroll. Up: Chrome navigates on a back/forward
                // button's mouseup, and only cancelling it stops that.
                me.prevent_default();
            }
            if phase == PointerPhase::Down {
                s.suppress_menu.set(prevented && policy::down_opens_context_menu(button, me.ctrl_key()));
            }
        }));
    }

    let s = shared.clone();
    listeners.push(crate::glue_dom::listen_fn(doc, "contextmenu", capture, move |e| {
        if s.suppress_menu.replace(false) || s.locked_to_us() {
            e.prevent_default();
        }
    }));

    let s = shared.clone();
    let wheel_opts = ListenerOptions { capture: true, passive: Some(false), once: false };
    listeners.push(crate::glue_dom::listen_fn(doc, "wheel", wheel_opts, move |e| {
        let we: WheelEvent = e.unchecked_into();
        let page = web_glue::dom::window()
            .and_then(|w| w.inner_height().ok())
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let mut ev = s.event_from(&we, PointerPhase::Wheel);
        ev.wheel_dx = policy::wheel_px(we.delta_x(), we.delta_mode(), page);
        ev.wheel_dy = policy::wheel_px(we.delta_y(), we.delta_mode(), page);
        if s.sink().event(&ev) == PointerOutcome::PreventDefault {
            we.prevent_default();
        }
    }));

    let s = shared.clone();
    listeners.push(crate::glue_dom::listen_fn(doc, "pointerlockchange", ListenerOptions::default(), move |_| {
        let ours = s.locked_to_us();
        if ours && !s.want_lock.get() {
            // Granted after the app already released: undo, report nothing
            // (the `pointerlockchange` for the exit reports `false`).
            s.doc.exit_pointer_lock();
            return;
        }
        s.sink().lock_changed(ours);
    }));

    let s = shared.clone();
    listeners.push(crate::glue_dom::listen_fn(doc, "pointerlockerror", ListenerOptions::default(), move |_| {
        s.want_lock.set(false);
        s.sink().lock_changed(false);
    }));

    if let Some(win) = web_glue::dom::window() {
        let s = shared.clone();
        listeners.push(crate::glue_dom::listen_fn(&win, "blur", ListenerOptions::default(), move |_| {
            s.suppress_menu.set(false);
            s.sink().focus_lost();
        }));
    }

    b.app_pointer = Some(AppPointerSource { shared, _listeners: listeners });
}

/// Request (`true`) or release the pointer lock. See the module docs for
/// why the request must happen synchronously here.
pub(crate) fn set_pointer_lock(b: &mut WebBackend, locked: bool) {
    let Some(src) = &b.app_pointer else {
        // No source: nobody to report to. Still never leave a lock behind.
        if !locked && locked_to(&b.doc, &b.mount) {
            b.doc.exit_pointer_lock();
        }
        return;
    };
    let shared = &src.shared;
    shared.want_lock.set(locked);
    if !locked {
        if shared.locked_to_us() {
            shared.doc.exit_pointer_lock();
        }
        return;
    }
    if shared.locked_to_us() {
        return;
    }
    if shared.mount.request_pointer_lock().is_err() {
        // A synchronous throw (no `requestPointerLock` at all, e.g. iOS
        // Safari): report the refusal. Deferred, because the dispatcher
        // holds this backend's borrow while it calls us and the sink must
        // never run under it.
        shared.want_lock.set(false);
        let sink = shared.sink();
        runtime_shared::scheduling::schedule_microtask(move || sink.lock_changed(false));
    }
}

/// Hide (`true`) or show the cursor over the page. Applied to the mount
/// AND `<html>`: overlays and portals render outside the mount, and a
/// cursor that reappears over a pause menu would defeat the point.
pub(crate) fn set_cursor_hidden(b: &mut WebBackend, hidden: bool) {
    apply_cursor_hidden(&b.doc, &b.mount, hidden);
}

fn apply_cursor_hidden(doc: &Document, mount: &Element, hidden: bool) {
    let root = doc.document_element();
    let targets = [Some(mount.clone()), root];
    if hidden {
        ensure_cursor_hidden_style(doc);
        for el in targets.iter().flatten() {
            let _ = el.set_attribute(CURSOR_HIDDEN_ATTR, "");
        }
    } else {
        for el in targets.iter().flatten() {
            let _ = el.remove_attribute(CURSOR_HIDDEN_ATTR);
        }
    }
}

fn ensure_cursor_hidden_style(doc: &Document) {
    if doc.get_element_by_id(CURSOR_HIDDEN_STYLE_ID).is_some() {
        return;
    }
    let Ok(style) = doc.create_element("style") else { return };
    style.set_id(CURSOR_HIDDEN_STYLE_ID);
    style.set_text_content(Some(CURSOR_HIDDEN_CSS));
    if let Some(head) = doc.head() {
        let _ = head.append_child(&style);
    }
}
