//! The `scroll_view` primitive — a `<div>` with `overflow: auto` on the
//! requested axis (or both, for a [`ScrollAxis::Both`] scroller). Sticky
//! descendants pin natively against it on every axis that scrolls.

use crate::WebBackend;
use runtime_shared::primitives::scroll_view::{EndReach, ScrollAxis, ScrollViewHandle, ScrollViewOps};
use std::any::Any;
use std::rc::Rc;
use web_glue::JsCast;
use web_glue::dom::Node;

pub(crate) fn create(
    b: &mut WebBackend,
    axis: ScrollAxis,
    on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
) -> Node {
    // HYDRATION: adopt the SSR scroll `<div>` (tag match) so its children
    // (the scrolled content) adopt in place. Without this the scroll_view
    // built a FRESH div and never advanced the cursor, so the SSR scroll node
    // was left for the next primitive (the content panel) to mis-adopt —
    // diverging the whole scrolled subtree and cascading to later siblings.
    // Mirrors `view::create`. Off hydration both arms create a fresh div.
    let div: web_glue::dom::Element = match b.hydrate_next("div") {
        Some(el) => el,
        None => {
            let el = b
                .doc
                .create_element("div")
                .expect("create_element div failed");
            // Its children stack as a flex column, as on native
            // (`css::VIEW_FLEX_RESET`); an author direction still wins.
            let _ = el.set_attribute(css::VIEW_MARKER_ATTR, "");
            b.hydrate_note_fresh(&el.clone().unchecked_into::<Node>());
            el
        }
    };
    // Apply the overflow style inline (not via the framework's style
    // system) so it's always present regardless of user-supplied
    // styling. The inline rules win over class rules for the overflow
    // properties; the class still governs flex direction etc.
    let _ = div.set_attribute("style", overflow_declaration(axis));
    // NOTE for `apply_scroll_view_bounces`: this writes the whole style
    // ATTRIBUTE, so anything that lands here later must go through
    // `style().set_property(...)` rather than a second `set_attribute`,
    // which would drop the overflow above.

    // Wire `on_scroll`. The callback receives CSS-pixel offsets
    // (`scrollLeft`/`scrollTop`) directly \u{2014} same units the
    // framework already uses for layout, so author code doesn't
    // need to translate.
    //
    // A RE-ENTRANT (`Fn`) listener, not `FnMut`: the author's callback
    // may write a signal whose subscribers mutate layout in ways that
    // synchronously re-fire `scroll`, and `FnMut`'s recursion guard would
    // reject that benign second call (`web_glue::Closure::new_fn`).
    //
    // The element owns the listener for its lifetime; the closure is
    // released when JS collects the element (a `.forget()` would leak one
    // closure per ScrollView for the life of the page).
    if let Some(cb) = on_scroll {
        let element_for_handler = div.clone();
        crate::glue_dom::listen_fn_for_element_lifetime(&div, "scroll", move |_| {
            if let Some(html) = element_for_handler.dyn_ref::<web_glue::dom::HtmlElement>() {
                let x = html.scroll_left() as f32;
                let y = html.scroll_top() as f32;
                cb(x, y);
            }
        });
    }

    if axis == ScrollAxis::Both {
        lock_wheel_direction(&div);
    }

    div.unchecked_into::<Node>()
}

/// How far one `deltaMode == 1` (line) wheel step scrolls, in CSS pixels
/// — the line height Chrome and Firefox use for it.
const WHEEL_LINE_PX: f64 = 16.0;

/// Lock a two-axis scroller's wheel and trackpad gestures to the axis each
/// one starts along — the rule iOS (`directionalLockEnabled`) and Android
/// (`RustTwoAxisScrollView`) apply to a drag on the same scroller, and
/// AppKit's predominant-axis scrolling applies to a trackpad. Browsers
/// rail a touch drag themselves but not a wheel, so without this a
/// mostly-vertical trackpad swipe down a wide table drifted it sideways on
/// the web alone.
///
/// A locked event is taken over: the cross axis is dropped and the locked
/// axis scrolled here. At the end of the locked axis the event is left to
/// the browser, so the page (or an outer scroller) still scrolls on —
/// the way Android hands a drag its scroller cannot follow to an ancestor.
fn lock_wheel_direction(div: &web_glue::dom::Element) {
    let el = div.clone();
    let state = std::cell::RefCell::new(WheelLock::new());
    crate::glue_dom::listen_fn_for_element_lifetime(div, "wheel", move |ev| {
        let ev: web_glue::dom::WheelEvent = ev.unchecked_into();
        // A ctrl+wheel is a pinch (zoom), and a handler may already have
        // taken the event.
        if ev.ctrl_key() || ev.default_prevented() {
            return;
        }
        let (unit_x, unit_y) = match ev.delta_mode() {
            1 => (WHEEL_LINE_PX, WHEEL_LINE_PX),
            2 => (el.client_width() as f64, el.client_height() as f64),
            _ => (1.0, 1.0),
        };
        let (dx, dy) = (ev.delta_x() * unit_x, ev.delta_y() * unit_y);
        let mut state = state.borrow_mut();
        let lock = state.lock_for(ev.time_stamp(), dx, dy);
        let (delta, pos, max) = match lock {
            WheelAxisLock::Free => return,
            WheelAxisLock::X => (
                dx,
                el.scroll_left_f64(),
                (el.scroll_width() - el.client_width()) as f64,
            ),
            WheelAxisLock::Y => (
                dy,
                el.scroll_top_f64(),
                (el.scroll_height() - el.client_height()) as f64,
            ),
        };
        if !locked_axis_can_take(delta, pos, max) {
            return;
        }
        ev.prevent_default();
        let target = state.step(pos, delta, max);
        match lock {
            WheelAxisLock::X => el.set_scroll_left(target),
            _ => el.set_scroll_top(target),
        }
    });
}

/// Which axis a wheel gesture on a two-axis scroller is locked to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WheelAxisLock {
    /// Diagonal: both axes scroll, as the browser does natively.
    Free,
    X,
    Y,
}

/// A gesture ends after a pause this long between wheel events; the next
/// event starts a new one and picks its lock afresh. Trackpads emit
/// events every frame (~16 ms) through a swipe and its momentum.
pub(crate) const WHEEL_GESTURE_GAP_MS: f64 = 160.0;

/// How much more one axis must move than the other to lock to it — the
/// ratio Android's two-axis scroller uses for a drag.
pub(crate) const DIRECTIONAL_LOCK_RATIO: f64 = 2.0;

/// The per-scroller gesture state: the current lock, when the last event
/// arrived, and the sub-pixel remainder of the locked axis (`scrollTop` /
/// `scrollLeft` are written whole, trackpad deltas are fractional).
pub(crate) struct WheelLock {
    lock: WheelAxisLock,
    last_ms: f64,
    carry: f64,
}

impl WheelLock {
    pub(crate) fn new() -> Self {
        Self { lock: WheelAxisLock::Free, last_ms: f64::NEG_INFINITY, carry: 0.0 }
    }

    /// The lock for an event at `t_ms` moving (`dx`, `dy`). The first event
    /// of a gesture decides it; the rest of the gesture keeps it.
    pub(crate) fn lock_for(&mut self, t_ms: f64, dx: f64, dy: f64) -> WheelAxisLock {
        if t_ms - self.last_ms > WHEEL_GESTURE_GAP_MS {
            let (ax, ay) = (dx.abs(), dy.abs());
            self.lock = if ax > ay * DIRECTIONAL_LOCK_RATIO {
                WheelAxisLock::X
            } else if ay > ax * DIRECTIONAL_LOCK_RATIO {
                WheelAxisLock::Y
            } else {
                WheelAxisLock::Free
            };
            self.carry = 0.0;
        }
        self.last_ms = t_ms;
        self.lock
    }

    /// The whole-pixel offset to write after scrolling `delta` from `pos`,
    /// clamped to `[0, max]`, keeping the fraction for the next event.
    pub(crate) fn step(&mut self, pos: f64, delta: f64, max: f64) -> i32 {
        let want = pos + delta + self.carry;
        let target = want.round().clamp(0.0, max.max(0.0));
        self.carry = want - target;
        if target <= 0.0 || target >= max {
            self.carry = 0.0;
        }
        target as i32
    }
}

/// Whether the locked axis can move `delta` from `pos` (range `[0, max]`).
/// At the end it is heading for, it cannot, and the event is left to the
/// page.
pub(crate) fn locked_axis_can_take(delta: f64, pos: f64, max: f64) -> bool {
    if max <= 0.0 {
        return false;
    }
    (delta < 0.0 && pos > 0.0) || (delta > 0.0 && pos < max) || delta == 0.0
}

/// Watch for the reader arriving at the end of the scroll axis.
///
/// Its own listener rather than a branch inside `on_scroll`'s: the DOM
/// takes as many `scroll` listeners as you give it, so the two features
/// stay independent and either can be present without the other. (The
/// iOS backend cannot do this — a UIScrollView has one delegate — which
/// is why that side has to share.)
///
/// `scrollHeight`/`clientHeight` are the two numbers the app cannot
/// reach for itself: an author has the offset from `on_scroll` and no
/// way to ask how much content is under it.
pub(crate) fn observe_end(
    node: &Node,
    horizontal: bool,
    threshold: f32,
    on_end: Rc<dyn Fn()>,
) {
    let el: web_glue::dom::Element = node.clone().unchecked_into();
    let el_for_handler = el.clone();
    let reach = std::cell::RefCell::new(EndReach::new(threshold));
    // Re-entrant and element-owned, for the same reasons as the
    // `on_scroll` listener above — see the note there.
    crate::glue_dom::listen_fn_for_element_lifetime(&el, "scroll", move |_| {
            if let Some(html) = el_for_handler.dyn_ref::<web_glue::dom::HtmlElement>() {
                let (offset, viewport, content) = if horizontal {
                    (
                        html.scroll_left() as f32,
                        html.client_width() as f32,
                        html.scroll_width() as f32,
                    )
                } else {
                    (
                        html.scroll_top() as f32,
                        html.client_height() as f32,
                        html.scroll_height() as f32,
                    )
                };
                if reach.borrow_mut().update(offset, viewport, content) {
                    on_end();
                }
            }
    });
}

/// The inline overflow a scroller of `axis` mounts with: `auto` on every
/// axis that scrolls, `hidden` on one that does not (so a one-axis
/// scroller clips rather than spilling its cross axis).
pub(crate) fn overflow_declaration(axis: ScrollAxis) -> &'static str {
    match axis {
        ScrollAxis::Vertical => "overflow-y: auto; overflow-x: hidden",
        ScrollAxis::Horizontal => "overflow-x: auto; overflow-y: hidden",
        ScrollAxis::Both => "overflow-x: auto; overflow-y: auto",
    }
}

pub(crate) fn make_handle(node: &Node) -> ScrollViewHandle {
    let el: web_glue::dom::HtmlElement = node
        .clone()
        .dyn_into()
        .expect("scroll_view node is not an HtmlElement");
    ScrollViewHandle::new(Rc::new(el), &WebScrollViewOps)
}

struct WebScrollViewOps;
impl ScrollViewOps for WebScrollViewOps {
    fn scroll_to(&self, node: &dyn Any, x: f32, y: f32) {
        if let Some(html) = node.downcast_ref::<web_glue::dom::HtmlElement>() {
            html.set_scroll_left(x as i32);
            html.set_scroll_top(y as i32);
        }
    }
}

/// `overscroll-behavior` — the web's spelling of "may this scroller
/// travel past its content".
///
/// Not a perfect analogue of iOS `bounces` and it does not pretend to
/// be. `none` does two things: it stops the rubber-band on the mobile
/// engines that have one (iOS Safari), and it stops SCROLL CHAINING —
/// a gesture that reaches this scroller's end no longer continues into
/// the page behind it. For a bounded pane inside a page, which is the
/// case this exists for, both are wanted.
///
/// Written on the scroller's OWN axes only, as the two longhands rather
/// than the shorthand (a two-axis scroller owns both, so both carry the
/// verdict). `overscroll-behavior` sets both axes, and a
/// one-axis scroller is still a scroll container on its cross axis
/// (`create` writes `overflow-y: hidden` on a horizontal one), so a
/// `none` landing there swallowed every cross-axis gesture that
/// started over the pane: a vertical wheel over a wide table stopped
/// the page behind it dead. The cross axis has nothing of its own to
/// travel past — its one job is to chain — so it is always `auto`.
/// See [`overscroll_declarations`].
///
/// Written with `set_property` rather than `set_attribute`: the mount
/// above writes the style ATTRIBUTE wholesale for the overflow, and a
/// second attribute write would drop it.
pub(crate) fn apply_bounces(el: &web_glue::dom::Element, bounces: bool) {
    let Some(html) = el.dyn_ref::<web_glue::dom::HtmlElement>() else {
        return;
    };
    let style = html.style();
    let overflow_x = style.get_property_value("overflow-x").unwrap_or_default();
    let overflow_y = style.get_property_value("overflow-y").unwrap_or_default();
    for (property, value) in overscroll_declarations(scroll_axis(&overflow_x, &overflow_y), bounces) {
        let _ = style.set_property(property, value);
    }
}

/// The scroller's axis, read back from the inline overflow `create`
/// wrote ([`overflow_declaration`]). The mount is the only writer of
/// `overflow-x` / `overflow-y` on this node, so `auto` on both is a
/// two-axis scroller, `auto` on X alone a horizontal one, and anything
/// else (`hidden`, or nothing at all) the vertical default.
pub(crate) fn scroll_axis(inline_overflow_x: &str, inline_overflow_y: &str) -> ScrollAxis {
    match (inline_overflow_x.trim() == "auto", inline_overflow_y.trim() == "auto") {
        (true, true) => ScrollAxis::Both,
        (true, false) => ScrollAxis::Horizontal,
        _ => ScrollAxis::Vertical,
    }
}

/// Which `overscroll-behavior-*` longhands `apply_bounces` writes, and
/// with what. Pure so the decision is testable from any host — the
/// crate's DOM tests only run under `wasm-pack test`.
///
/// The scroller's own axis carries the `bounces` verdict; the cross
/// axis is `auto` unconditionally, because a `none` there is the bug
/// this exists to prevent (see [`apply_bounces`]).
pub(crate) fn overscroll_declarations(
    axis: ScrollAxis,
    bounces: bool,
) -> [(&'static str, &'static str); 2] {
    let own = if bounces { "auto" } else { "none" };
    match axis {
        ScrollAxis::Horizontal => [("overscroll-behavior-x", own), ("overscroll-behavior-y", "auto")],
        ScrollAxis::Vertical => [("overscroll-behavior-y", own), ("overscroll-behavior-x", "auto")],
        ScrollAxis::Both => [("overscroll-behavior-x", own), ("overscroll-behavior-y", own)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The regression: a horizontal scroller that does not bounce must
    /// leave its vertical axis free to chain, or a wheel over a wide
    /// table pins the page.
    #[test]
    fn horizontal_no_bounce_contains_x_only() {
        let decls = overscroll_declarations(ScrollAxis::Horizontal, false);
        assert_eq!(decls, [("overscroll-behavior-x", "none"), ("overscroll-behavior-y", "auto")]);
        assert!(
            decls.iter().all(|(p, _)| *p != "overscroll-behavior"),
            "the shorthand sets both axes and is exactly the bug"
        );
    }

    #[test]
    fn vertical_no_bounce_contains_y_only() {
        assert_eq!(
            overscroll_declarations(ScrollAxis::Vertical, false),
            [("overscroll-behavior-y", "none"), ("overscroll-behavior-x", "auto")]
        );
    }

    /// A two-axis scroller owns both axes, so `bounces(false)` contains
    /// both.
    #[test]
    fn two_axis_no_bounce_contains_both() {
        assert_eq!(
            overscroll_declarations(ScrollAxis::Both, false),
            [("overscroll-behavior-x", "none"), ("overscroll-behavior-y", "none")]
        );
    }

    #[test]
    fn bounces_leaves_both_axes_auto() {
        for axis in [ScrollAxis::Horizontal, ScrollAxis::Vertical, ScrollAxis::Both] {
            assert!(overscroll_declarations(axis, true)
                .iter()
                .all(|(_, v)| *v == "auto"));
        }
    }

    /// A two-axis scroller's wheel/trackpad gesture locks to the axis it
    /// starts along (a diagonal start pans freely), carries sub-pixel
    /// remainders, and hands the wheel to the page at the locked axis' end.
    #[test]
    fn a_wheel_gesture_locks_to_the_axis_it_starts_along() {
        let mut lock = WheelLock::new();
        // Mostly down: Y, kept through the gesture even when it wobbles.
        assert_eq!(lock.lock_for(0.0, 2.0, 30.0), WheelAxisLock::Y);
        assert_eq!(lock.lock_for(16.0, 12.0, 10.0), WheelAxisLock::Y);
        // A pause ends it; the next gesture, mostly sideways, locks to X.
        assert_eq!(lock.lock_for(16.0 + WHEEL_GESTURE_GAP_MS + 1.0, 40.0, 3.0), WheelAxisLock::X);
        // A diagonal start pans freely.
        assert_eq!(lock.lock_for(1000.0, 20.0, 18.0), WheelAxisLock::Free);
    }

    #[test]
    fn the_locked_axis_keeps_its_fraction_and_stops_at_the_ends() {
        let mut lock = WheelLock::new();
        lock.lock_for(0.0, 0.0, 1.0);
        assert_eq!(lock.step(10.0, 0.4, 100.0), 10);
        assert_eq!(lock.step(10.0, 0.4, 100.0), 11, "two 0.4s add up to a pixel");
        assert_eq!(lock.step(95.0, 30.0, 100.0), 100, "clamped to the end");
        assert!(!locked_axis_can_take(5.0, 100.0, 100.0), "at the bottom, a scroll down goes to the page");
        assert!(locked_axis_can_take(-5.0, 100.0, 100.0), "…a scroll up stays");
        assert!(!locked_axis_can_take(5.0, 0.0, 0.0), "an axis with nothing to scroll never takes it");
    }

    /// `create` mounts `overflow: auto` on exactly the axes that scroll,
    /// `hidden` on the rest — a two-axis scroller is `auto` on both.
    #[test]
    fn each_axis_mounts_its_own_overflow() {
        assert_eq!(overflow_declaration(ScrollAxis::Vertical), "overflow-y: auto; overflow-x: hidden");
        assert_eq!(overflow_declaration(ScrollAxis::Horizontal), "overflow-x: auto; overflow-y: hidden");
        assert_eq!(overflow_declaration(ScrollAxis::Both), "overflow-x: auto; overflow-y: auto");
    }

    #[test]
    fn axis_is_read_from_the_inline_overflow() {
        assert_eq!(scroll_axis("auto", "hidden"), ScrollAxis::Horizontal);
        assert_eq!(scroll_axis(" auto ", ""), ScrollAxis::Horizontal);
        assert_eq!(scroll_axis("auto", "auto"), ScrollAxis::Both);
        assert_eq!(scroll_axis("hidden", "auto"), ScrollAxis::Vertical);
        assert_eq!(scroll_axis("", ""), ScrollAxis::Vertical);
        // Round trip: what `create` writes reads back as the same axis.
        for axis in [ScrollAxis::Vertical, ScrollAxis::Horizontal, ScrollAxis::Both] {
            let decl = overflow_declaration(axis);
            let get = |prop: &str| {
                decl.split(';')
                    .filter_map(|d| d.split_once(':'))
                    .find(|(p, _)| p.trim() == prop)
                    .map(|(_, v)| v.trim().to_string())
                    .unwrap_or_default()
            };
            assert_eq!(scroll_axis(&get("overflow-x"), &get("overflow-y")), axis);
        }
    }
}
