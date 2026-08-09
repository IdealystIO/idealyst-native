//! Raw touch delivery for the web backend.
//!
//! Implements [`runtime_shared::Backend::install_touch_handler`] and
//! [`runtime_shared::Backend::claim_touch`] using the Pointer Events
//! API. One DOM element receives five listeners — `pointerdown`,
//! `pointermove`, `pointerup`, `pointercancel`, plus `contextmenu`,
//! which both suppresses the browser's native menu and stands in for
//! the `pointerdown` that some browsers withhold for a secondary
//! press (Chrome/macOS Ctrl-click) — and translates each into a
//! [`TouchEvent`] for the framework's handler.
//!
//! Pointer Events unify mouse, touch, and pen on the web; the
//! `pointerType` distinction is not surfaced through the framework
//! today (`force` is filled when the device reports it; otherwise
//! `None`).
//!
//! Native scroll / pinch on the subscribed element is suppressed via
//! `touch-action: none`. Once a handler returns `claim: true`, we
//! call `setPointerCapture` so subsequent events stay locked to this
//! element even if the finger / cursor leaves its bounds — that's
//! the web-side implementation of the claim protocol.

use crate::WebBackend;
use runtime_shared::{
    set_pointer_button, set_pointer_modifiers, PointerButton, PointerModifiers, TouchEvent,
    TouchHandler, TouchId, TouchPhase, TouchPoint,
};
use std::cell::{Cell, RefCell};
use runtime_shared::collections::{SmallIdMap, SmallIdSet};
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::{Element, MouseEvent, Node, PointerEvent};

/// Install the four pointer listeners on `node`. Storage of the
/// resulting [`Closure`]s is shared with the existing event-listener
/// vec so the JS side keeps them alive for the element's lifetime.
pub(crate) fn install(b: &mut WebBackend, node: &Node, handler: TouchHandler) {
    // The framework only installs a touch handler on primitives that
    // map to real DOM elements; if the downcast fails we'd be
    // looking at a text node or a fragment, which shouldn't carry
    // an `on_touch` slot. Bail silently rather than panic — the
    // framework treats a missing impl as best-effort.
    let element: Element = match node.clone().dyn_into::<Element>() {
        Ok(e) => e,
        Err(_) => return,
    };

    // Suppress native scroll/pinch so the browser doesn't preempt
    // our events with pan-to-scroll or pinch-to-zoom. Touch-action:
    // none is the Pointer Events knob for "I want all the events
    // myself"; CSS-cascadable so existing stylesheet rules can
    // override per-element if needed.
    if let Ok(html) = element.clone().dyn_into::<web_sys::HtmlElement>() {
        let _ = html.style().set_property("touch-action", "none");
    }

    // `pointermove` fires for hover-only motion too — for mouse the
    // pointer can move over an element with no button down. We
    // only deliver `Moved` for pointers that are currently "down"
    // on this element, which we track here. Touch never hovers, so
    // this filter is effectively a no-op for finger input.
    //
    // We also track captured pointers (those for which a handler
    // returned `claim: true`) so future logic — e.g. an element
    // suppressing scroll while claimed — can read it.
    let active: Rc<RefCell<SmallIdSet<i32>>> = Rc::new(RefCell::new(SmallIdSet::new()));
    let captured: Rc<RefCell<SmallIdSet<i32>>> = Rc::new(RefCell::new(SmallIdSet::new()));

    // Element-local coordinates are `client - element_origin`. Reading that
    // origin (`getBoundingClientRect`) forces a synchronous layout flush, and
    // doing it on every `pointermove` — the drag hot path — is the dominant
    // cost of dragging on web (it stutters *any* `on_touch` gesture, not just
    // DnD). The origin only changes on real layout (scroll / resize / reflow),
    // NOT from the CSS transform a drag applies to the element itself — in fact
    // reading the live, transformed rect each move makes view-local position a
    // *moving* frame that fights the drag. So we sample the origin ONCE at
    // `pointerdown` and reuse it for the gesture's moves. Keyed by pointer id;
    // dropped on up/cancel.
    let origins: Rc<RefCell<SmallIdMap<i32, (f64, f64)>>> = Rc::new(RefCell::new(SmallIdMap::new()));

    // Whether `pointerdown` delivered a Secondary `Began` for the press whose
    // `contextmenu` is about to fire, and whether the handler consumed it.
    // Browsers disagree on how a secondary press surfaces (see the
    // `contextmenu` listener below), so the two listeners share this note:
    // `pointerdown` writes it, `contextmenu` takes it. `None` means the press
    // never reached app code through `pointerdown` and `contextmenu` must
    // deliver it itself. Written on EVERY pointerdown (cleared for
    // non-secondary buttons) so one anomalous unpaired secondary press can't
    // leave a stale entry that eats the next Ctrl-click.
    let secondary_delivered: Rc<Cell<Option<bool>>> = Rc::new(Cell::new(None));

    // pointerdown — Began.
    {
        let handler = handler.clone();
        let active = active.clone();
        let captured = captured.clone();
        let origins = origins.clone();
        let secondary_delivered = secondary_delivered.clone();
        let element_for_capture = element.clone();
        let closure = Closure::<dyn FnMut(PointerEvent)>::new(move |ev: PointerEvent| {
            // `button` is 0 for touch + pen contact + primary mouse; 2 is the
            // secondary press. On macOS, Ctrl-click IS the OS's secondary
            // press, but only Safari does that folding for us (`button == 2`
            // at pointerdown); Chrome and Firefox deliver `button == 0` with
            // `ctrlKey` — which, delivered as Primary, reaches app code as a
            // Ctrl-modified primary click (e.g. toggling a selection) right
            // before the context-menu press it actually is. So the backend
            // folds it here, and the press rides the normal secondary path:
            // `Began`-only, `secondary_delivered` recorded, `contextmenu`
            // suppress-only. The stray `button == 0` pointerup is already
            // ignored — a secondary press never enters `active`. Mouse only:
            // a Ctrl-held *touch* (iPad with hardware keyboard reports
            // platform "MacIntel") is not a click at all.
            let mac_ctrl_click = ev.button() == 0
                && ev.ctrl_key()
                && ev.pointer_type() == "mouse"
                && ctrl_click_is_secondary();
            let button = match ev.button() {
                0 if mac_ctrl_click => PointerButton::Secondary,
                0 => PointerButton::Primary,
                1 => PointerButton::Middle,
                2 => PointerButton::Secondary,
                other => PointerButton::Other(other.max(0) as u16),
            };
            set_pointer_button(button);
            set_pointer_modifiers(PointerModifiers {
                shift: ev.shift_key(),
                ctrl: ev.ctrl_key(),
                alt: ev.alt_key(),
                meta: ev.meta_key(),
            });
            // Sample the element origin once, here; reused for every move.
            let origin = element_origin(&ev);
            let local = local_from(&ev, origin);
            let touch_id = TouchId(ev.pointer_id() as u64);
            let te = TouchEvent {
                id: touch_id,
                phase: TouchPhase::Began,
                position: TouchPoint::new(local.0, local.1),
                window_position: TouchPoint::new(ev.client_x() as f32, ev.client_y() as f32),
                timestamp_ns: timestamp_ns(&ev),
                force: pressure_to_force(ev.pressure()),
            };
            // Batching is automatic: the `on_touch` handler is wrapped in a
            // reactive cycle at attach time (see `runtime_shared::cycle`), so every
            // signal write it makes (a camera pan writes pan_x, pan_y, + a repaint
            // tick) fans out ONCE after the handler returns. Without it each write
            // triggers the reactive repaint effect separately; web coalesces those
            // to one rAF render and keeps only the LAST — which can be a no-op
            // (composite-at-origin) frame, so a pan appears frozen until the next
            // bake. Centralized in core so every backend gets it uniformly.
            let response = (handler)(&te);
            // Leave the note for the `contextmenu` listener, which fires next
            // for a secondary press: the `Began` already reached app code, so
            // it must only suppress the native menu, not re-deliver. Cleared
            // (not skipped) for other buttons so it can never go stale.
            secondary_delivered.set(if button == PointerButton::Secondary {
                Some(response.consumed)
            } else {
                None
            });
            if response.consumed {
                // Honor the responder model: whichever ancestor consumes
                // the Began keeps every subsequent event for this TouchId.
                // Listeners are bubble-phase, so the deepest element fires
                // first; halting propagation here stops the same Began from
                // also reaching ancestor `on_touch` listeners. An unconsumed
                // Began (no stop) still bubbles up to retry one level higher.
                ev.stop_propagation();
                // A non-primary press delivers ONLY this `Began` — it never
                // enters the drag/capture path. The browser's context menu can
                // swallow the matching `pointerup`, which would strand a
                // claimed gesture (a dragged element following the cursor
                // forever), so a secondary press is registered as a complete
                // click here and nothing is left open. `PointerButton`'s docs
                // state this contract for authors.
                if button == PointerButton::Primary {
                    active.borrow_mut().insert(ev.pointer_id());
                    // Cache the origin for this gesture's moves (mirrors
                    // `active`, so it's cleaned up by the same up/cancel path).
                    origins.borrow_mut().insert(ev.pointer_id(), origin);
                    if response.claim {
                        capture_pointer(&element_for_capture, ev.pointer_id(), &captured);
                    }
                }
            }
        });
        let _ = element.add_event_listener_with_callback(
            "pointerdown",
            closure.as_ref().unchecked_ref(),
        );
        b._touch_closures
            .push(closure.into_js_value().unchecked_into());
    }

    // contextmenu — native-menu suppression AND, in some browsers, the only
    // delivery of a secondary press.
    //
    // A node that handles touch owns its secondary press, so the browser's own
    // menu must not open on top of whatever the app puts there; `preventDefault`
    // has to run on the DOM event, which never reaches app code, so it lives
    // here (bubble-phase and element-scoped — the rest of the page keeps its
    // native menu). But browsers disagree on how the press itself surfaces —
    // a macOS Ctrl-click has been observed arriving in all three shapes, and
    // the backend must be correct under each (FRAMEWORK-NOTES #95):
    //
    //   - `pointerdown` with `button == 2` (Safari's remap; also every
    //     browser's real right-button / two-finger press). Handled by the
    //     pointerdown listener directly.
    //   - `pointerdown` with `button == 0` + `ctrlKey` plus this
    //     `contextmenu` (Chrome and Firefox). The pointerdown listener folds
    //     that into Secondary on macOS — delivered as Primary it would reach
    //     app code as a Ctrl-modified primary click first, corrupting
    //     selection-style handlers right before the menu opens.
    //   - NO `pointerdown` at all, only this `contextmenu` (also observed
    //     from Chrome on macOS). With a suppress-only listener here, that
    //     shape reached app code as nothing at all — hence the synthesis
    //     below.
    //
    // So `pointerdown` records whether it already delivered the Secondary
    // `Began` (and whether the handler consumed it) and this listener takes
    // that note. Present → only the menu needs suppressing; propagation
    // mirrors the pointerdown outcome (consumed → stop), so an ancestor's own
    // `contextmenu` listener neither re-delivers a press its descendant
    // consumed nor misses one it ignored. Absent → the press never surfaced
    // as a pointer event; synthesize the Secondary `Began` and dispatch it
    // exactly as `pointerdown` would. A secondary press is contractually "a
    // complete click — `Began` only" (see the pointerdown listener), so the
    // synthetic event needs no active/capture/up bookkeeping.
    {
        let handler = handler.clone();
        let secondary_delivered = secondary_delivered.clone();
        let closure = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
            ev.prevent_default();
            if let Some(consumed) = secondary_delivered.take() {
                if consumed {
                    ev.stop_propagation();
                }
                return;
            }
            // Long-press on a touchscreen also raises `contextmenu` (Chrome
            // ships it as a PointerEvent with pointerType "touch"). That
            // press already reached app code as a *primary* gesture that may
            // still be in flight — injecting a Secondary `Began` mid-gesture
            // would corrupt it, so touch keeps suppress-only behavior.
            if let Some(pe) = ev.dyn_ref::<PointerEvent>() {
                if pe.pointer_type() == "touch" {
                    return;
                }
            }
            set_pointer_button(PointerButton::Secondary);
            set_pointer_modifiers(PointerModifiers {
                shift: ev.shift_key(),
                ctrl: ev.ctrl_key(),
                alt: ev.alt_key(),
                meta: ev.meta_key(),
            });
            let origin = element_origin(&ev);
            let local = local_from(&ev, origin);
            // Chrome delivers `contextmenu` as a PointerEvent (real pointer
            // id); Firefox as a plain MouseEvent. The id is never matched
            // against a later up/cancel — a secondary press is `Began`-only —
            // so falling back to 0 (Firefox's mouse pointerId) is safe.
            let pid = ev
                .dyn_ref::<PointerEvent>()
                .map(|pe| pe.pointer_id())
                .unwrap_or(0);
            let te = TouchEvent {
                id: TouchId(pid as u64),
                phase: TouchPhase::Began,
                position: TouchPoint::new(local.0, local.1),
                window_position: TouchPoint::new(ev.client_x() as f32, ev.client_y() as f32),
                timestamp_ns: timestamp_ns(&ev),
                force: None,
            };
            // Born batched via the core `on_touch` cycle wrapper.
            if (handler)(&te).consumed {
                ev.stop_propagation();
            }
        });
        let _ = element
            .add_event_listener_with_callback("contextmenu", closure.as_ref().unchecked_ref());
        b._touch_closures
            .push(closure.into_js_value().unchecked_into());
    }

    // pointermove — Moved for pointers in `active`; `Hovered` for unpressed
    // mouse/pen motion (touch never hovers — an inactive touch move is stray).
    //
    // Hover has no pointerdown to sample the element origin at, and reading
    // `getBoundingClientRect` per move is the layout-flush cost the origin
    // cache above exists to avoid — so hover keeps its own origin cache,
    // refreshed at most every 200ms (a presence cursor doesn't need per-pixel
    // rect freshness across scrolls).
    let hover_origin: Rc<RefCell<Option<(f64, (f64, f64))>>> = Rc::new(RefCell::new(None));
    {
        let handler = handler.clone();
        let active = active.clone();
        let captured = captured.clone();
        let origins = origins.clone();
        let element_for_capture = element.clone();
        let closure = Closure::<dyn FnMut(PointerEvent)>::new(move |ev: PointerEvent| {
            let pid = ev.pointer_id();
            if !active.borrow().contains(&pid) {
                let ptype = ev.pointer_type();
                if ptype != "mouse" && ptype != "pen" {
                    return;
                }
                let now = ev.time_stamp();
                let origin = {
                    let mut cache = hover_origin.borrow_mut();
                    match *cache {
                        Some((ts, o)) if now - ts < 200.0 => o,
                        _ => {
                            let o = element_origin(&ev);
                            *cache = Some((now, o));
                            o
                        }
                    }
                };
                let local = local_from(&ev, origin);
                let te = TouchEvent {
                    id: TouchId(pid as u64),
                    phase: TouchPhase::Hovered,
                    position: TouchPoint::new(local.0, local.1),
                    window_position: TouchPoint::new(ev.client_x() as f32, ev.client_y() as f32),
                    timestamp_ns: timestamp_ns(&ev),
                    force: None,
                };
                let _ = (handler)(&te);
                return;
            }
            set_pointer_modifiers(PointerModifiers {
                shift: ev.shift_key(),
                ctrl: ev.ctrl_key(),
                alt: ev.alt_key(),
                meta: ev.meta_key(),
            });
            // Hot path: use the origin cached at pointerdown — NO layout read.
            let origin = origins
                .borrow()
                .get(&pid)
                .copied()
                .unwrap_or_else(|| element_origin(&ev));
            let local = local_from(&ev, origin);
            let touch_id = TouchId(pid as u64);
            let te = TouchEvent {
                id: touch_id,
                phase: TouchPhase::Moved,
                position: TouchPoint::new(local.0, local.1),
                window_position: TouchPoint::new(ev.client_x() as f32, ev.client_y() as f32),
                timestamp_ns: timestamp_ns(&ev),
                force: pressure_to_force(ev.pressure()),
            };
            // Batching is automatic: the `on_touch` handler is wrapped in a
            // reactive cycle at attach time (see `runtime_shared::cycle`), so every
            // signal write it makes (a camera pan writes pan_x, pan_y, + a repaint
            // tick) fans out ONCE after the handler returns. Without it each write
            // triggers the reactive repaint effect separately; web coalesces those
            // to one rAF render and keeps only the LAST — which can be a no-op
            // (composite-at-origin) frame, so a pan appears frozen until the next
            // bake. Centralized in core so every backend gets it uniformly.
            let response = (handler)(&te);
            if response.consumed {
                ev.stop_propagation();
            }
            if response.claim && !captured.borrow().contains(&pid) {
                capture_pointer(&element_for_capture, pid, &captured);
            }
        });
        let _ = element.add_event_listener_with_callback(
            "pointermove",
            closure.as_ref().unchecked_ref(),
        );
        b._touch_closures
            .push(closure.into_js_value().unchecked_into());
    }

    // pointerup — Ended.
    {
        let handler = handler.clone();
        let active = active.clone();
        let captured = captured.clone();
        let origins = origins.clone();
        let closure = Closure::<dyn FnMut(PointerEvent)>::new(move |ev: PointerEvent| {
            let pid = ev.pointer_id();
            if !active.borrow_mut().remove(&pid) {
                return;
            }
            captured.borrow_mut().remove(&pid);
            let origin = origins
                .borrow_mut()
                .remove(&pid)
                .unwrap_or_else(|| element_origin(&ev));
            let local = local_from(&ev, origin);
            let touch_id = TouchId(pid as u64);
            let te = TouchEvent {
                id: touch_id,
                phase: TouchPhase::Ended,
                position: TouchPoint::new(local.0, local.1),
                window_position: TouchPoint::new(ev.client_x() as f32, ev.client_y() as f32),
                timestamp_ns: timestamp_ns(&ev),
                force: pressure_to_force(ev.pressure()),
            };
            // Born batched via the core `on_touch` cycle wrapper.
            if (handler)(&te).consumed {
                ev.stop_propagation();
            }
        });
        let _ = element.add_event_listener_with_callback(
            "pointerup",
            closure.as_ref().unchecked_ref(),
        );
        // Safety net: also listen on the window. If `setPointerCapture` didn't
        // hold, or the release landed off this element (over another element —
        // the now-`pointer-events:none` drag ghost lets the release fall to
        // whatever's beneath) or outside the window entirely, the element-level
        // `pointerup` never fires and the gesture would be stuck "active"
        // forever (the dragged element never gets its `Ended`). This catches the
        // release for THIS element's still-active pointers. When capture DID
        // hold, the element handler runs first, removes the pointer from
        // `active`, and `stop_propagation`s — so this fires as a no-op (the
        // `active` check returns early) and never double-delivers.
        if let Some(win) = web_sys::window() {
            let _ = win.add_event_listener_with_callback(
                "pointerup",
                closure.as_ref().unchecked_ref(),
            );
        }
        b._touch_closures
            .push(closure.into_js_value().unchecked_into());
    }

    // pointercancel — Cancelled.
    {
        let handler = handler.clone();
        let active = active.clone();
        let captured = captured.clone();
        let origins = origins.clone();
        let closure = Closure::<dyn FnMut(PointerEvent)>::new(move |ev: PointerEvent| {
            let pid = ev.pointer_id();
            if !active.borrow_mut().remove(&pid) {
                return;
            }
            captured.borrow_mut().remove(&pid);
            let origin = origins
                .borrow_mut()
                .remove(&pid)
                .unwrap_or_else(|| element_origin(&ev));
            let local = local_from(&ev, origin);
            let touch_id = TouchId(pid as u64);
            let te = TouchEvent {
                id: touch_id,
                phase: TouchPhase::Cancelled,
                position: TouchPoint::new(local.0, local.1),
                window_position: TouchPoint::new(ev.client_x() as f32, ev.client_y() as f32),
                timestamp_ns: timestamp_ns(&ev),
                force: pressure_to_force(ev.pressure()),
            };
            // Born batched via the core `on_touch` cycle wrapper.
            if (handler)(&te).consumed {
                ev.stop_propagation();
            }
        });
        let _ = element.add_event_listener_with_callback(
            "pointercancel",
            closure.as_ref().unchecked_ref(),
        );
        // Window safety net — same rationale as `pointerup` above: never miss a
        // cancel for an active pointer, even if it's delivered off this element.
        if let Some(win) = web_sys::window() {
            let _ = win.add_event_listener_with_callback(
                "pointercancel",
                closure.as_ref().unchecked_ref(),
            );
        }
        b._touch_closures
            .push(closure.into_js_value().unchecked_into());
    }
}

/// Make `el` "swallow" a press from any ancestor `on_touch` gesture
/// recognizer — the web equivalent of native's single-view touch delivery.
///
/// On native, tapping a `Button` / `Pressable` / `Link` delivers the touch
/// to that control only (AppKit sends `mouseDown` to the hit `NSButton` /
/// tappable `FlippedView`; the responder chain stops there), so a parent
/// view's `on_touch` tap never also fires. On web those controls activate
/// through a native `click` listener, which lives in a DIFFERENT event
/// channel from the framework's pointer-based `on_touch` responder chain —
/// so without this, pressing a button INSIDE an `on_touch` view (a clickable
/// table row, a tappable card) fires BOTH the button and the ancestor.
///
/// One bubble-phase `pointerdown` listener that `stop_propagation`s closes
/// the gap: the button is deeper than the ancestor, so its listener runs
/// first and halts the `pointerdown` before the ancestor's `on_touch`
/// listener sees it. That ancestor therefore never records this pointer in
/// its `active` set, so its `pointerup`/`pointermove`/`pointercancel`
/// listeners all early-return (see `install`) and no ancestor tap is
/// recognized. The control's OWN `click` is untouched — `stop_propagation`
/// halts bubbling, not the browser's click synthesis.
pub(crate) fn swallow_ancestor_touch(b: &mut WebBackend, el: &web_sys::Element) {
    let closure = Closure::<dyn FnMut(PointerEvent)>::new(move |ev: PointerEvent| {
        ev.stop_propagation();
    });
    let _ = el.add_event_listener_with_callback("pointerdown", closure.as_ref().unchecked_ref());
    b._touch_closures
        .push(closure.into_js_value().unchecked_into());

    // The swallow must extend to `contextmenu`: an ancestor `on_touch`
    // element's contextmenu listener SYNTHESIZES a Secondary `Began` when no
    // pointerdown preceded it (see `install`) — and stopping the control's
    // `pointerdown` above guarantees exactly that "no pointerdown" state at
    // the ancestor. Without this, right-clicking a Button inside an
    // `on_touch` row would deliver the row a secondary press its control
    // swallowed. `preventDefault` preserves the pre-#95 observable behavior:
    // the ancestor's listener used to suppress the native menu over the
    // control via the bubbled event, and stopping propagation here would
    // otherwise re-enable it.
    let closure = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
        ev.prevent_default();
        ev.stop_propagation();
    });
    let _ = el.add_event_listener_with_callback("contextmenu", closure.as_ref().unchecked_ref());
    b._touch_closures
        .push(closure.into_js_value().unchecked_into());
}

/// Implementation of [`runtime_shared::Backend::claim_touch`] —
/// external claim invoked when a handler returned `claim: true` via
/// any route other than the local `pointerdown` / `pointermove`
/// callback we wired above (today there's no such route on web, but
/// the trait method exists for symmetry with iOS / Android where the
/// framework dispatches and the backend claims).
///
/// In practice on web, claims happen inline in the listener closure
/// (where we have the live `PointerEvent` to pass to
/// `setPointerCapture`). This method is a no-op fallback.
#[allow(dead_code)]
pub(crate) fn claim(_b: &mut WebBackend, _node: &Node, _touch_id: TouchId) {
    // No-op on web; see doc comment.
}

thread_local! {
    /// Cached "is this browser running on macOS" — `None` until first sampled.
    /// Tests override it via [`force_ctrl_click_fold`] so both branches are
    /// exercised deterministically regardless of the test host's OS.
    static CTRL_CLICK_FOLDS: Cell<Option<bool>> = const { Cell::new(None) };
}

/// Whether a `button == 0` press with Ctrl held should be classified as
/// [`PointerButton::Secondary`] — true exactly on macOS, where Ctrl-click *is*
/// the OS's secondary press (`PointerButton`'s docs promise this folding).
/// Browsers disagree on doing the fold themselves (Safari does at
/// `pointerdown`; Chrome and Firefox deliver a primary-with-ctrlKey and leave
/// it to us), so the backend applies it uniformly. On Windows/Linux Ctrl-click
/// is genuinely a modified primary press (add-to-selection idioms), so the
/// fold must NOT apply there.
///
/// `navigator.platform` ("MacIntel", …) is sampled once and cached: it's
/// deprecated-but-universal, and the alternatives (UA parsing,
/// `userAgentData.platform`) are async or no more reliable.
fn ctrl_click_is_secondary() -> bool {
    CTRL_CLICK_FOLDS.with(|c| {
        if let Some(v) = c.get() {
            return v;
        }
        let v = web_sys::window()
            .map(|w| w.navigator().platform().unwrap_or_default().starts_with("Mac"))
            .unwrap_or(false);
        c.set(Some(v));
        v
    })
}

/// Test hook: pin (or with `None`, un-pin) the macOS Ctrl-click fold so tests
/// can exercise both the mac and non-mac classification on any host.
#[cfg(test)]
pub(crate) fn force_ctrl_click_fold(v: Option<bool>) {
    CTRL_CLICK_FOLDS.with(|c| c.set(v));
}

/// The listener element's top-left in viewport (client) coordinates.
/// **This is the one `getBoundingClientRect` call** — a forced synchronous
/// layout flush — so it is made once per gesture (at `pointerdown`) and the
/// result is cached for the gesture's moves. Returns `(0, 0)` if the target
/// isn't an element, which makes [`local_from`] hand back raw client
/// coordinates — a same-frame approximation, better than nothing.
///
/// Takes `&MouseEvent` (not `&PointerEvent`) so the `contextmenu` path —
/// a plain MouseEvent in Firefox — can share it; `PointerEvent` call sites
/// deref-coerce. Same for [`local_from`] / [`timestamp_ns`].
fn element_origin(ev: &MouseEvent) -> (f64, f64) {
    let Some(target) = ev.current_target() else {
        return (0.0, 0.0);
    };
    let el: web_sys::Element = match target.dyn_into() {
        Ok(e) => e,
        Err(_) => return (0.0, 0.0),
    };
    let rect = el.get_bounding_client_rect();
    (rect.x(), rect.y())
}

/// Element-local position = client position − element origin. Pure arithmetic,
/// no layout read.
fn local_from(ev: &MouseEvent, origin: (f64, f64)) -> (f32, f32) {
    (
        ev.client_x() as f32 - origin.0 as f32,
        ev.client_y() as f32 - origin.1 as f32,
    )
}

/// Convert the event's `timeStamp` (DOMHighResTimeStamp, ms with
/// fractional precision) to nanoseconds. Web exposes only ms-with-
/// fractions; the conversion preserves the fractional part by
/// multiplying before casting.
fn timestamp_ns(ev: &web_sys::Event) -> u64 {
    (ev.time_stamp() * 1_000_000.0) as u64
}

/// Map the Pointer Events `pressure` field (0.0..=1.0 if reported)
/// onto our `force` slot. The DOM reports `0.5` for buttons that
/// don't track pressure but are active; we treat that as "no
/// information" by returning `None`. Pen / 3D-touch devices report
/// finer-grained values which pass through.
fn pressure_to_force(pressure: f32) -> Option<f32> {
    // The Pointer Events spec says non-pressure-sensitive sources
    // emit either 0.0 (no button) or 0.5 (button down). Both
    // values are sentinels rather than real measurements.
    if pressure == 0.0 || (pressure - 0.5).abs() < f32::EPSILON {
        None
    } else {
        Some(pressure)
    }
}

/// Call `Element.setPointerCapture(pointer_id)` and record the
/// capture in `captured`. Suppresses the call on browsers that
/// haven't implemented it (we fall back to whatever
/// `add_event_listener` plus `touch-action: none` give us).
fn capture_pointer(element: &Element, pointer_id: i32, captured: &Rc<RefCell<SmallIdSet<i32>>>) {
    let _ = element.set_pointer_capture(pointer_id);
    captured.borrow_mut().insert(pointer_id);
}
