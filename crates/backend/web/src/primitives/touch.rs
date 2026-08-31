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
//! `touch-action: none`; the native text selection a press would
//! otherwise anchor is suppressed via `selectstart` (see that listener
//! for why not `preventDefault` and not `user-select`).
//!
//! **`setPointerCapture` runs when a handler CONSUMES the `Began`, not
//! when it claims.** Consuming already means "this pointer is mine":
//! `runtime_shared::touch` documents that whichever handler consumes the
//! `Began` keeps every later event for that `TouchId`, and every native
//! backend delivers that for free — UIKit `touchesMoved:`, AppKit
//! `mouseDragged:` and Android's touch target all keep reporting to the
//! view that took the down, however far outside its bounds the finger
//! travels. DOM delivery does not: an uncaptured `pointermove` goes to
//! whatever is under the cursor, so an element hears only about motion
//! that stays inside its own rect.
//!
//! That silently broke every slop-gated recognizer (pan, drag, pinch),
//! which by construction cannot claim until it has measured N px of
//! travel — the travel it needs the events to see. Chrome coalesces
//! `pointermove` to one dispatch per frame, aimed at the element under
//! the cursor's FINAL position that frame, so a normal-speed flick's
//! first sample is already off a small handle and the recognizer hears
//! nothing after `Began`. It presented as "a drag only starts if you
//! press and move slowly", and imposed an unwritten floor of ~2× the
//! activation slop on the size of any drag handle.
//!
//! Capture is delivery, not preemption — it cancels no native scroller
//! by itself, and native scrolling from this element is already off via
//! `touch-action: none` — so it is not a claim and must not wait for
//! one. The `claim`-driven capture on a later `Moved` stays as a
//! fallback (it is idempotent, and re-tries a press-time capture the
//! browser refused).
//!
//! A release that lands off the element still has to finish the
//! gesture, so there is also a safety net on `window` — but exactly
//! ONE for the whole page, shared by every subscribed element via
//! [`WINDOW_NET`]. See that item for why it must not be per-element.

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

/// Install the pointer listeners on `node`. The element owns the resulting
/// [`Closure`]s (see [`super::own_listener`]), so they are released with it.
pub(crate) fn install(node: &Node, handler: TouchHandler) {
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
    // We also track captured pointers — those routed to this element by
    // `setPointerCapture`, i.e. every gesture whose `Began` this handler
    // consumed — so a later `claim: true` doesn't re-capture needlessly.
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

    // Ending a gesture — `Ended` / `Cancelled` — is reachable by two routes:
    // this element's own `pointerup` / `pointercancel` listener, and the shared
    // window safety net (see [`WINDOW_NET`]) for a release that never reaches
    // the element. Both go through this one closure so the bookkeeping (leave
    // `active`, drop the cached origin, unregister from the net) happens
    // exactly once: whichever route runs second finds the pointer already out
    // of `active` and returns without re-delivering.
    let finish: Rc<GestureFinish> = {
        let handler = handler.clone();
        let active = active.clone();
        let captured = captured.clone();
        let origins = origins.clone();
        Rc::new(move |ev: &PointerEvent, phase: TouchPhase| {
            let pid = ev.pointer_id();
            if !active.borrow_mut().remove(&pid) {
                return;
            }
            captured.borrow_mut().remove(&pid);
            unregister_gesture(pid);
            let origin = origins
                .borrow_mut()
                .remove(&pid)
                .unwrap_or_else(|| element_origin(ev));
            let local = local_from(ev, origin);
            let te = TouchEvent {
                id: TouchId(pid as u64),
                phase,
                position: TouchPoint::new(local.0, local.1),
                window_position: TouchPoint::new(ev.client_x() as f32, ev.client_y() as f32),
                timestamp_ns: timestamp_ns(ev),
                force: pressure_to_force(ev.pressure()),
            };
            // Born batched via the core `on_touch` cycle wrapper.
            if (handler)(&te).consumed {
                ev.stop_propagation();
            }
        })
    };

    // pointerdown — Began.
    {
        let handler = handler.clone();
        let active = active.clone();
        let captured = captured.clone();
        let origins = origins.clone();
        let finish = finish.clone();
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
                    // Arm the shared window safety net for THIS pointer only,
                    // for as long as the gesture is live (mirrors `active` /
                    // `origins`, cleared by the same `finish`).
                    register_gesture(ev.pointer_id(), &finish);
                    // Capture on the CONSUME, not on a later claim — see
                    // the module docs. Without it a recognizer that has to
                    // measure travel before it can claim never sees the
                    // travel, because DOM delivery is bounded by this
                    // element's rect until something captures.
                    capture_pointer(&element_for_capture, ev.pointer_id(), &captured);
                }
            }
        });
        let _ = element.add_event_listener_with_callback(
            "pointerdown",
            closure.as_ref().unchecked_ref(),
        );
        super::own_listener(closure);
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
        super::own_listener(closure);
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
        super::own_listener(closure);
    }

    // selectstart — suppress the native text selection a gesture press would
    // otherwise anchor.
    //
    // A press this element consumed is a gesture, not a caret placement, and
    // on every native backend it selects nothing (AppKit labels are
    // `isSelectable: false` by default, UIKit/Android labels likewise) — so
    // the browser sweeping a highlight across whatever sits beside a drag
    // handle is a web-only divergence. It is also the *visible* half of a
    // gesture that failed to pick up: the drag silently doesn't happen and
    // the user gets a stray highlight, which reads as a styling bug.
    //
    // Cancelling `selectstart` is the narrow tool for this. The two
    // alternatives are both worse:
    //   - `preventDefault()` on the consumed `pointerdown` suppresses the
    //     compatibility `mousedown`, and the browser's focus move IS that
    //     event's default action — the framework relies on exactly this
    //     elsewhere (`mark_preserves_focus`). Cancelling it here would stop
    //     a press on a gesture surface from focusing anything inside it and
    //     from blurring whatever was focused before, so a text input inside
    //     a draggable card would go dead.
    //   - `user-select: none` written next to `touch-action: none` is an
    //     INLINE declaration, so it would outrank the stylesheet rule the
    //     style system emits for the `user_select` prop — an author could no
    //     longer opt a gesture surface back into selection at all.
    //
    // Gated on a live consumed press so ordinary selection through this
    // element (no gesture in flight) is untouched, and skipped when the
    // press landed somewhere that owns its own selection — an editable
    // field, or a subtree whose computed `user-select` is an explicit
    // opt-in.
    {
        let active = active.clone();
        let closure = Closure::<dyn FnMut(web_sys::Event)>::new(move |ev: web_sys::Event| {
            if active.borrow().is_empty() || target_owns_its_selection(&ev) {
                return;
            }
            ev.prevent_default();
        });
        let _ = element
            .add_event_listener_with_callback("selectstart", closure.as_ref().unchecked_ref());
        super::own_listener(closure);
    }

    // pointerup — Ended. pointercancel — Cancelled. Both are pure delegations
    // to `finish`; the window safety net calls the same closure for a release
    // that never reaches this element.
    for (event, phase) in [
        ("pointerup", TouchPhase::Ended),
        ("pointercancel", TouchPhase::Cancelled),
    ] {
        let finish = finish.clone();
        let closure = Closure::<dyn FnMut(PointerEvent)>::new(move |ev: PointerEvent| {
            finish(&ev, phase);
        });
        let _ = element.add_event_listener_with_callback(event, closure.as_ref().unchecked_ref());
        super::own_listener(closure);
    }
}

/// Finishes one live gesture: `(event, phase)` → deliver `Ended` / `Cancelled`
/// to the element that owns the pointer. Boxed so the element's own listeners
/// and the shared window net can call the identical code path.
type GestureFinish = dyn Fn(&PointerEvent, TouchPhase);

/// Page-level state for the off-element release safety net.
struct WindowNet {
    /// Have the two shared listeners been attached to `window`? Set even when
    /// there is no `window` to attach to — the condition can't change, so
    /// retrying per gesture would only cost a lookup per press.
    installed: bool,
    /// How many listeners actually landed on `window`. Read by the regression
    /// test; the invariant is that it never exceeds 2 no matter how many
    /// elements subscribe.
    listener_count: usize,
    /// Pointers currently mid-gesture → the finisher of the element that owns
    /// them. At most one entry per pointer id: a `Began` only enters an
    /// element's `active` set when that element's handler CONSUMED it, and a
    /// consumed `Began` stops propagating, so two elements can never both own
    /// the same pointer. Entries are added at `pointerdown` and removed by
    /// `finish`, so the map is bounded by the number of fingers on the glass —
    /// not by the number of subscribed elements. A gesture abandoned mid-press
    /// (element destroyed under the finger) holds its one entry until the next
    /// release for that pointer id replaces or clears it; the framework's
    /// `ScopeAlive` guard around the handler makes that late call inert.
    gestures: SmallIdMap<i32, Rc<GestureFinish>>,
}

impl WindowNet {
    const fn new() -> Self {
        Self {
            installed: false,
            listener_count: 0,
            gestures: SmallIdMap::new(),
        }
    }
}

thread_local! {
    /// The ONE `pointerup` / `pointercancel` pair on `window` that finishes a
    /// gesture whose release never reached its element — because
    /// `setPointerCapture` didn't hold, because the release landed over
    /// something else (a `pointer-events: none` drag ghost lets it fall
    /// through to whatever is beneath), or because it happened outside the
    /// window entirely. Without it a dragged element never gets its `Ended`
    /// and follows the cursor forever.
    ///
    /// Shared, and registered per live pointer rather than per element,
    /// because `window` outlives every element on the page: listeners added
    /// there for an element's benefit are never removed when the element is
    /// detached (a DOM node's own listeners die with it; `window`'s do not).
    /// Installing a pair per subscribed element therefore grew `window`'s
    /// listener list without bound in any app that mounts `on_touch` elements
    /// dynamically — a virtualized grid re-slicing on scroll added two per
    /// cell per slice, and every subsequent `pointerup` anywhere on the page
    /// was dispatched into all of them, getting monotonically slower the
    /// longer the user scrolled.
    static WINDOW_NET: RefCell<WindowNet> = RefCell::new(WindowNet::new());
}

/// Arm the window safety net for `pid`, attaching the shared listeners on
/// first use.
fn register_gesture(pid: i32, finish: &Rc<GestureFinish>) {
    ensure_window_net();
    WINDOW_NET.with(|net| {
        net.borrow_mut().gestures.insert(pid, finish.clone());
    });
}

/// Disarm the net for `pid`. Called from `finish` — i.e. from BOTH completion
/// routes — so a gesture is unregistered exactly once, whichever route ends it.
fn unregister_gesture(pid: i32) {
    WINDOW_NET.with(|net| {
        net.borrow_mut().gestures.remove(&pid);
    });
}

/// Attach the shared `pointerup` / `pointercancel` listeners to `window`, once
/// per thread (i.e. once per page).
fn ensure_window_net() {
    let first = WINDOW_NET.with(|net| {
        let mut net = net.borrow_mut();
        let first = !net.installed;
        net.installed = true;
        first
    });
    if !first {
        return;
    }
    let Some(win) = web_sys::window() else {
        return;
    };
    for (event, phase) in [
        ("pointerup", TouchPhase::Ended),
        ("pointercancel", TouchPhase::Cancelled),
    ] {
        let closure = Closure::<dyn FnMut(PointerEvent)>::new(move |ev: PointerEvent| {
            // Clone the finisher OUT of the map before calling it: `finish`
            // re-enters `unregister_gesture`, which needs the same `RefCell`
            // mutably, so the read borrow must be released first.
            let finish = WINDOW_NET
                .with(|net| net.borrow().gestures.get(&ev.pointer_id()).cloned());
            if let Some(finish) = finish {
                finish(&ev, phase);
            }
        });
        if win
            .add_event_listener_with_callback(event, closure.as_ref().unchecked_ref())
            .is_ok()
        {
            WINDOW_NET.with(|net| net.borrow_mut().listener_count += 1);
        }
        // Deliberately permanent, and the one place in this module where that
        // is correct: two listeners for the lifetime of the page, sized by
        // nothing. `window` roots the JS function.
        closure.forget();
    }
}

/// Test hook: `(window listeners installed, gestures currently armed)`.
#[cfg(test)]
pub(crate) fn window_net_stats() -> (usize, usize) {
    WINDOW_NET.with(|net| {
        let net = net.borrow();
        (net.listener_count, net.gestures.len())
    })
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
pub(crate) fn swallow_ancestor_touch(el: &web_sys::Element) {
    let closure = Closure::<dyn FnMut(PointerEvent)>::new(move |ev: PointerEvent| {
        ev.stop_propagation();
    });
    let _ = el.add_event_listener_with_callback("pointerdown", closure.as_ref().unchecked_ref());
    super::own_listener(closure);

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
    super::own_listener(closure);
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

/// Whether the node a `selectstart` fired on owns its own text selection, and
/// so must keep it even while this element has a gesture press in flight:
/// an editable field (its caret / drag-select IS the selection), or a subtree
/// carrying an explicit `user-select` opt-in from the author's `user_select`
/// prop. Everything else — plain labels, the gesture surface itself — has no
/// business anchoring a highlight off a press the handler took.
///
/// `getComputedStyle` forces a style recalc, which is why this runs on
/// `selectstart` (once per press) and never on the `pointermove` hot path.
fn target_owns_its_selection(ev: &web_sys::Event) -> bool {
    let Some(target) = ev.target() else {
        return false;
    };
    let Ok(el) = target.dyn_into::<Element>() else {
        return false;
    };
    if let Some(html) = el.dyn_ref::<web_sys::HtmlElement>() {
        if html.is_content_editable() {
            return true;
        }
    }
    if matches!(el.tag_name().as_str(), "INPUT" | "TEXTAREA") {
        return true;
    }
    let Some(win) = web_sys::window() else {
        return false;
    };
    let Ok(Some(style)) = win.get_computed_style(&el) else {
        return false;
    };
    // Safari only landed the unprefixed property recently; the style system
    // emits both (see `css::rules_to_css`), so read both.
    ["user-select", "-webkit-user-select"].iter().any(|prop| {
        matches!(
            style.get_property_value(prop).unwrap_or_default().as_str(),
            "text" | "all"
        )
    })
}

/// Call `Element.setPointerCapture(pointer_id)` and record the
/// capture in `captured`. Suppresses the call on browsers that
/// haven't implemented it (we fall back to whatever
/// `add_event_listener` plus `touch-action: none` give us).
fn capture_pointer(element: &Element, pointer_id: i32, captured: &Rc<RefCell<SmallIdSet<i32>>>) {
    #[cfg(test)]
    record_capture_attempt(pointer_id);
    let _ = element.set_pointer_capture(pointer_id);
    captured.borrow_mut().insert(pointer_id);
}

#[cfg(test)]
thread_local! {
    /// Pointer ids `capture_pointer` was called for, newest last. The browser
    /// itself refuses `setPointerCapture` for a synthetic pointer id (it
    /// matches no *active* pointer, so it throws `NotFoundError` and
    /// `hasPointerCapture` stays false), so a wasm test can only observe that
    /// the backend ASKED — which is the decision under test.
    static CAPTURE_ATTEMPTS: RefCell<Vec<i32>> = const { RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn record_capture_attempt(pointer_id: i32) {
    CAPTURE_ATTEMPTS.with(|c| c.borrow_mut().push(pointer_id));
}

/// Test hook: drain the pointer ids captured since the last call.
#[cfg(test)]
pub(crate) fn take_capture_attempts() -> Vec<i32> {
    CAPTURE_ATTEMPTS.with(|c| std::mem::take(&mut *c.borrow_mut()))
}
