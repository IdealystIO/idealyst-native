//! Scroll chaining between nested `NSScrollView`s.
//!
//! AppKit gives a scroll gesture to the innermost `NSScrollView` under the
//! pointer, and that scroll view keeps all of it, whichever way the fingers
//! move. A vertical list inside a horizontal board therefore swallowed every
//! horizontal pan over the list, and the board never moved. The browser
//! instead chains: a scroller that cannot move along the gesture's axis
//! passes the gesture to its scrolling ancestor. This module gives AppKit
//! the browser's rule.
//!
//! When a gesture starts, the scroll view decides who owns it:
//!
//! - the gesture's axis is the dominant one of its first deltas
//!   (`|dx| > |dy|` is horizontal; otherwise vertical — a plain mouse wheel
//!   only carries `dy`, and AppKit turns shift+wheel into `dx`);
//! - the scroll view keeps the gesture when it can scroll that axis: it
//!   scrolls the axis at all ([`set_axes`]; not `hasVerticalScroller` /
//!   `hasHorizontalScroller`, which `scrollbar: Hidden` turns off on a
//!   scroller that still scrolls) AND its content is longer than its viewport on
//!   it. Otherwise it forwards the gesture up the responder chain, where the
//!   enclosing scroll view makes the same decision;
//! - the decision holds for the whole gesture: every later `changed` /
//!   `ended` event and the momentum events that follow go to the same owner,
//!   so a gesture never splits between two scrollers mid-flight;
//! - `overscroll-behavior: contain | none` keeps every gesture, as CSS does;
//! - a scroll view with no enclosing scroll view keeps the gesture (and its
//!   rubber-band), since there is nothing to chain to.
//!
//! The decision logic is pure ([`ScrollChain`]) so it is tested without
//! AppKit; [`ChainedScrollView`] and the virtualizer's
//! `VirtualizerScrollView` feed it from their `scrollWheel:` overrides via
//! [`scroll_wheel`].
//!
//! ## Responsive scrolling
//!
//! AppKit turns responsive scrolling (overdraw: pre-rendering content
//! outside the visible rect so scrolling stays at the display rate) OFF for
//! a scroll view when it, its clip view or its documentView overrides
//! `scrollWheel:` — unless the overriding class answers YES from
//! `+isCompatibleWithResponsiveScrolling` (`NSView.h`). Both chained scroll
//! view classes answer YES: the override only picks which scroll view's
//! stock `scrollWheel:` runs and never draws or changes how the scroll view
//! scrolls once it owns the gesture. With responsive scrolling AppKit may
//! track the rest of a gesture itself after the first event instead of
//! sending each event through `scrollWheel:`; that keeps the gesture with
//! the scroll view whose stock `scrollWheel:` began it, the same owner the
//! latch here records, so both delivery paths agree.

use std::cell::Cell;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_app_kit::{NSEvent, NSScrollView, NSView};
use objc2_foundation::{CGRect, MainThreadMarker};

/// `NSEventPhase` bits (`NSEvent.h`).
pub(crate) mod phase {
    pub const NONE: usize = 0;
    pub const BEGAN: usize = 1 << 0;
    #[cfg(test)]
    pub const CHANGED: usize = 1 << 2;
    pub const ENDED: usize = 1 << 3;
    pub const CANCELLED: usize = 1 << 4;
    pub const MAY_BEGIN: usize = 1 << 5;
}

/// A scroll axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Axis {
    X,
    Y,
}

/// Who handles a scroll event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Route {
    /// This scroll view scrolls (its stock `scrollWheel:`).
    Own,
    /// The enclosing scroll view gets it (via `nextResponder`).
    Forward,
}

/// The axis a gesture runs along, from its deltas. Ties go to Y: a mouse
/// wheel is vertical, and an exactly diagonal start has no better answer.
/// `None` when both deltas are zero — a `mayBegin` (fingers down, not yet
/// moving) carries no direction.
pub(crate) fn dominant_axis(dx: f64, dy: f64) -> Option<Axis> {
    let (ax, ay) = (dx.abs(), dy.abs());
    if ax == 0.0 && ay == 0.0 {
        None
    } else if ax > ay {
        Some(Axis::X)
    } else {
        Some(Axis::Y)
    }
}

/// What a scroll view can scroll right now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Reach {
    pub x: bool,
    pub y: bool,
}

impl Reach {
    /// Below this the content is treated as fitting: layout rounding can
    /// leave the document a fraction of a point past the viewport.
    const SLOP: f64 = 0.5;

    /// A scroll view that scrolls `x` / `y` at all, whose content is
    /// `content` long against a `viewport`, both `(width, height)`.
    pub fn new(
        scrolls_x: bool,
        scrolls_y: bool,
        content: (f64, f64),
        viewport: (f64, f64),
    ) -> Self {
        Reach {
            x: scrolls_x && content.0 > viewport.0 + Self::SLOP,
            y: scrolls_y && content.1 > viewport.1 + Self::SLOP,
        }
    }

    pub fn on(self, axis: Axis) -> bool {
        match axis {
            Axis::X => self.x,
            Axis::Y => self.y,
        }
    }
}

/// The fields of one `scrollWheel:` event the routing reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Wheel {
    /// `NSEvent.phase` ([`phase`] bits).
    pub phase: usize,
    /// `NSEvent.momentumPhase` ([`phase`] bits).
    pub momentum: usize,
    /// `scrollingDeltaX` / `scrollingDeltaY`.
    pub dx: f64,
    pub dy: f64,
}

impl Wheel {
    /// A mouse wheel (no phase at all): each tick stands alone.
    fn is_discrete(self) -> bool {
        self.phase == phase::NONE && self.momentum == phase::NONE
    }

    /// The first event of a trackpad gesture.
    fn starts_gesture(self) -> bool {
        self.phase & (phase::BEGAN | phase::MAY_BEGIN) != 0
    }

    /// The last event of a gesture: its momentum ran out, or a `mayBegin`
    /// was cancelled without the fingers moving. `ended` is not the end —
    /// momentum may follow it.
    fn ends_gesture(self) -> bool {
        self.momentum & (phase::ENDED | phase::CANCELLED) != 0 || self.phase & phase::CANCELLED != 0
    }
}

/// One scroll view's routing state: its `overscroll-behavior` and the owner
/// of the gesture in flight.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ScrollChain {
    /// `overscroll-behavior: contain | none` — never forward.
    pub contain: bool,
    /// The axes this scroll view scrolls.
    pub scrolls_x: bool,
    pub scrolls_y: bool,
    /// `scrollbar: Hidden`.
    pub bars_hidden: bool,
    /// The owner of the current gesture, decided at its start.
    latched: Option<Route>,
}

impl ScrollChain {
    /// Route one event. `reach` is read only when a decision is made (a
    /// gesture's start, or a mouse-wheel tick), so a gesture is not re-judged
    /// as it scrolls itself to an edge. `has_parent` says whether an
    /// enclosing scroll view exists to forward to.
    pub fn route(&mut self, ev: Wheel, has_parent: bool, reach: impl FnOnce() -> Reach) -> Route {
        if self.contain || !has_parent {
            self.latched = None;
            return Route::Own;
        }
        if ev.is_discrete() {
            self.latched = None;
            return decide(ev, reach());
        }
        if ev.starts_gesture() {
            return match dominant_axis(ev.dx, ev.dy) {
                Some(_) => {
                    let route = decide(ev, reach());
                    self.latched = Some(route);
                    route
                }
                // A `mayBegin` has no direction yet: let this scroll view see
                // it (it only stops any running momentum and shows the
                // scrollers) and decide on the `began` that follows.
                None => {
                    self.latched = None;
                    Route::Own
                }
            };
        }
        let route = self.latched.unwrap_or(Route::Own);
        if ev.ends_gesture() {
            self.latched = None;
        }
        route
    }

    /// The owner of the gesture in flight, if one was decided.
    #[cfg(test)]
    fn latched(&self) -> Option<Route> {
        self.latched
    }
}

fn decide(ev: Wheel, reach: Reach) -> Route {
    match dominant_axis(ev.dx, ev.dy) {
        Some(axis) if !reach.on(axis) => Route::Forward,
        _ => Route::Own,
    }
}

/// What `scroll_view` can scroll now: the axes it scrolls, and its
/// documentView against its clip view.
fn reach_of(scroll_view: &NSScrollView, chain: ScrollChain) -> Reach {
    let doc: *mut NSView = unsafe { msg_send![scroll_view, documentView] };
    if doc.is_null() {
        return Reach::default();
    }
    let doc_frame: CGRect = unsafe { msg_send![doc, frame] };
    let clip: *mut NSView = unsafe { msg_send![scroll_view, contentView] };
    if clip.is_null() {
        return Reach::default();
    }
    let clip_bounds: CGRect = unsafe { msg_send![clip, bounds] };
    Reach::new(
        chain.scrolls_x,
        chain.scrolls_y,
        (doc_frame.size.width, doc_frame.size.height),
        (clip_bounds.size.width, clip_bounds.size.height),
    )
}

/// The `scrollWheel:` body of a chained scroll view. Returns `true` when the
/// event went to the enclosing scroll view; on `false` the caller runs its
/// stock `[super scrollWheel:]`.
///
/// The event is forwarded through `nextResponder`, the path every wheel
/// event a content view does not handle already takes to its scroll view,
/// so the enclosing scroll view (and any `on_wheel` view between the two)
/// receives it exactly as if the inner scroll view were not there.
pub(crate) fn scroll_wheel(
    scroll_view: &NSScrollView,
    chain: &Cell<ScrollChain>,
    event: &NSEvent,
) -> bool {
    let ev = Wheel {
        phase: unsafe { msg_send![event, phase] },
        momentum: unsafe { msg_send![event, momentumPhase] },
        dx: unsafe { msg_send![event, scrollingDeltaX] },
        dy: unsafe { msg_send![event, scrollingDeltaY] },
    };
    let parent: *mut NSScrollView = unsafe { msg_send![scroll_view, enclosingScrollView] };
    let mut state = chain.get();
    let route = state.route(ev, !parent.is_null(), || reach_of(scroll_view, chain.get()));
    chain.set(state);
    if route == Route::Own {
        return false;
    }
    let next: *mut AnyObject = unsafe { msg_send![scroll_view, nextResponder] };
    if next.is_null() {
        return false;
    }
    let _: () = unsafe { msg_send![next, scrollWheel: event] };
    true
}

/// Write `overscroll-behavior` into `view`'s chaining state when it is one of
/// the chained scroll view classes. `contain` is true for `Contain` / `None`.
pub(crate) fn set_contain(view: &NSView, contain: bool) {
    if let Some(chain) = chain_of(view) {
        let mut state = chain.get();
        state.contain = contain;
        chain.set(state);
    }
}

/// Set the axes a chained scroll view scrolls, and show their scrollers.
pub(crate) fn set_axes(view: &NSView, x: bool, y: bool) {
    if let Some(chain) = chain_of(view) {
        let state = ScrollChain { scrolls_x: x, scrolls_y: y, ..chain.get() };
        chain.set(state);
        show_bars(view, state);
    }
}

/// Hide or show a chained scroll view's scrollers (`scrollbar: Hidden`). It
/// scrolls the same either way: an `NSScrollView` scrolls a document longer
/// than it is with or without a scroller on that axis.
pub(crate) fn set_bars_hidden(view: &NSView, hidden: bool) {
    if let Some(chain) = chain_of(view) {
        let state = ScrollChain { bars_hidden: hidden, ..chain.get() };
        chain.set(state);
        show_bars(view, state);
    }
}

fn show_bars(view: &NSView, state: ScrollChain) {
    let (x, y) = (state.scrolls_x && !state.bars_hidden, state.scrolls_y && !state.bars_hidden);
    unsafe {
        // Only on a change: the setters re-tile the scroll view.
        let has_x: bool = msg_send![view, hasHorizontalScroller];
        if has_x != x {
            let _: () = msg_send![view, setHasHorizontalScroller: x];
        }
        let has_y: bool = msg_send![view, hasVerticalScroller];
        if has_y != y {
            let _: () = msg_send![view, setHasVerticalScroller: y];
        }
    }
}

/// `view`'s chaining state, when it is a chained scroll view.
pub(crate) fn chain_of(view: &NSView) -> Option<&Cell<ScrollChain>> {
    if is_kind(view, ChainedScrollView::class()) {
        let typed = unsafe { &*(view as *const NSView as *const ChainedScrollView) };
        return Some(&typed.ivars().chain);
    }
    let virt = crate::imp::virtualizer::VirtualizerScrollView::class();
    if is_kind(view, virt) {
        return crate::imp::virtualizer::VirtualizerScrollView::chain_of(view);
    }
    None
}

fn is_kind(view: &NSView, cls: &AnyClass) -> bool {
    unsafe { msg_send![view, isKindOfClass: cls] }
}

pub(crate) struct ChainedScrollViewIvars {
    chain: Cell<ScrollChain>,
}

declare_class!(
    /// The `NSScrollView` of a `scroll_view` (and of a virtual grid): a stock
    /// scroll view that chains gestures it cannot scroll to its enclosing
    /// scroll view (module docs).
    pub(crate) struct ChainedScrollView;

    unsafe impl ClassType for ChainedScrollView {
        type Super = NSScrollView;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystMacChainedScrollView";
    }

    impl DeclaredClass for ChainedScrollView {
        type Ivars = ChainedScrollViewIvars;
    }

    unsafe impl ChainedScrollView {
        #[method(scrollWheel:)]
        fn scroll_wheel(&self, event: &NSEvent) {
            if !scroll_wheel(self, &self.ivars().chain, event) {
                let _: () = unsafe { msg_send![super(self), scrollWheel: event] };
            }
        }

        // Opt back into responsive scrolling, which AppKit otherwise turns
        // off for a scroll view overriding `scrollWheel:` (module docs).
        #[method(isCompatibleWithResponsiveScrolling)]
        fn is_compatible_with_responsive_scrolling() -> bool {
            true
        }
    }
);

impl ChainedScrollView {
    pub(crate) fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(ChainedScrollViewIvars {
            chain: Cell::new(ScrollChain::default()),
        });
        unsafe { msg_send_id![super(this), init] }
    }

    /// A new chained scroll view as a plain `NSView`.
    pub(crate) fn new_view(mtm: MainThreadMarker) -> Retained<NSView> {
        Retained::into_super(Retained::into_super(Self::new(mtm)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOM_Y: Reach = Reach { x: false, y: true };
    const ROOM_X: Reach = Reach { x: true, y: false };
    const ROOM_XY: Reach = Reach { x: true, y: true };

    fn ev(phase: usize, momentum: usize, dx: f64, dy: f64) -> Wheel {
        Wheel {
            phase,
            momentum,
            dx,
            dy,
        }
    }
    fn began(dx: f64, dy: f64) -> Wheel {
        ev(phase::BEGAN, phase::NONE, dx, dy)
    }
    fn changed(dx: f64, dy: f64) -> Wheel {
        ev(phase::CHANGED, phase::NONE, dx, dy)
    }
    fn ended() -> Wheel {
        ev(phase::ENDED, phase::NONE, 0.0, 0.0)
    }
    fn momentum(m: usize, dx: f64, dy: f64) -> Wheel {
        ev(phase::NONE, m, dx, dy)
    }
    fn wheel(dx: f64, dy: f64) -> Wheel {
        ev(phase::NONE, phase::NONE, dx, dy)
    }

    #[test]
    fn dominant_axis_picks_the_larger_delta_and_ties_go_vertical() {
        assert_eq!(dominant_axis(-12.0, 3.0), Some(Axis::X));
        assert_eq!(dominant_axis(2.0, -9.0), Some(Axis::Y));
        assert_eq!(dominant_axis(5.0, 5.0), Some(Axis::Y));
        assert_eq!(dominant_axis(-5.0, 5.0), Some(Axis::Y));
        assert_eq!(dominant_axis(0.0, 0.0), None);
        assert_eq!(dominant_axis(0.0, -0.0), None);
        assert_eq!(dominant_axis(0.1, 0.0), Some(Axis::X));
    }

    #[test]
    fn reach_needs_a_scroller_and_content_past_the_viewport() {
        // A vertical list taller than its viewport, as wide as it.
        assert_eq!(
            Reach::new(false, true, (300.0, 2000.0), (300.0, 600.0)),
            ROOM_Y
        );
        // A horizontal board wider than its viewport.
        assert_eq!(
            Reach::new(true, false, (2400.0, 600.0), (800.0, 600.0)),
            ROOM_X
        );
        // Content that fits (rounding slop included) gives no room.
        assert_eq!(
            Reach::new(true, true, (800.4, 600.0), (800.0, 600.0)),
            Reach::default()
        );
        // Overflow on an axis without a scroller is not room: a vertical
        // scroller whose row is wider than it still cannot pan sideways.
        assert_eq!(
            Reach::new(false, true, (900.0, 2000.0), (300.0, 600.0)),
            ROOM_Y
        );
        assert_eq!(
            Reach::new(true, true, (900.0, 2000.0), (300.0, 600.0)),
            ROOM_XY
        );
    }

    /// REGRESSION (Timely Kanban board): a horizontal pan over a column's
    /// vertical list stayed with the list, so the board never scrolled. The
    /// list now forwards it, for the whole gesture and its momentum, and
    /// the next gesture is judged afresh.
    #[test]
    fn regression_horizontal_pan_over_vertical_list_goes_to_the_board() {
        let mut list = ScrollChain::default();
        let r = |c: &mut ScrollChain, e| c.route(e, true, || ROOM_Y);

        assert_eq!(
            r(&mut list, ev(phase::MAY_BEGIN, phase::NONE, 0.0, 0.0)),
            Route::Own
        );
        assert_eq!(r(&mut list, began(-8.0, 1.0)), Route::Forward);
        // Later events drift vertical; the gesture stays with the board.
        assert_eq!(r(&mut list, changed(-3.0, -6.0)), Route::Forward);
        assert_eq!(r(&mut list, changed(0.0, -9.0)), Route::Forward);
        assert_eq!(r(&mut list, ended()), Route::Forward);
        assert_eq!(
            r(&mut list, momentum(phase::BEGAN, -20.0, 0.0)),
            Route::Forward
        );
        assert_eq!(
            r(&mut list, momentum(phase::CHANGED, -10.0, -4.0)),
            Route::Forward
        );
        assert_eq!(
            r(&mut list, momentum(phase::ENDED, 0.0, 0.0)),
            Route::Forward
        );
        assert_eq!(list.latched(), None, "the gesture is over");

        // The next gesture, vertical, scrolls the list.
        assert_eq!(r(&mut list, began(1.0, -7.0)), Route::Own);
        assert_eq!(
            r(&mut list, changed(-9.0, 0.0)),
            Route::Own,
            "still the list's gesture"
        );
        assert_eq!(r(&mut list, ended()), Route::Own);
        assert_eq!(r(&mut list, momentum(phase::BEGAN, -9.0, 0.0)), Route::Own);
    }

    #[test]
    fn a_gesture_on_an_axis_with_room_stays_put() {
        let mut board = ScrollChain::default();
        assert_eq!(board.route(began(-8.0, 0.0), true, || ROOM_X), Route::Own);
        assert_eq!(board.route(changed(0.0, -8.0), true, || ROOM_X), Route::Own);
        let mut grid = ScrollChain::default();
        assert_eq!(grid.route(began(-1.0, -8.0), true, || ROOM_XY), Route::Own);
    }

    #[test]
    fn reach_is_read_only_when_deciding() {
        let mut c = ScrollChain::default();
        let reads = Cell::new(0);
        let reach = || {
            reads.set(reads.get() + 1);
            ROOM_Y
        };
        c.route(began(-5.0, 0.0), true, reach);
        c.route(changed(-5.0, 0.0), true, || {
            panic!("changed must not re-judge")
        });
        c.route(ended(), true, || panic!("ended must not re-judge"));
        c.route(momentum(phase::BEGAN, -5.0, 0.0), true, || {
            panic!("momentum must not re-judge")
        });
        assert_eq!(reads.get(), 1);
    }

    #[test]
    fn content_that_fits_forwards_both_axes() {
        // A vertical scroller whose content fits: nothing to scroll on
        // either axis, so a vertical gesture chains to an outer vertical
        // scroller, as on the web.
        let mut c = ScrollChain::default();
        assert_eq!(
            c.route(began(0.0, -6.0), true, Reach::default),
            Route::Forward
        );
    }

    #[test]
    fn a_may_begin_without_movement_never_latches() {
        let mut c = ScrollChain::default();
        assert_eq!(
            c.route(ev(phase::MAY_BEGIN, phase::NONE, 0.0, 0.0), true, || ROOM_Y),
            Route::Own
        );
        assert_eq!(c.latched(), None);
        // Cancelled (fingers lifted without moving): this scroll view saw the
        // `mayBegin`, so it sees the cancel.
        assert_eq!(
            c.route(ev(phase::CANCELLED, phase::NONE, 0.0, 0.0), true, || ROOM_Y),
            Route::Own
        );
        assert_eq!(c.latched(), None);
    }

    #[test]
    fn a_cancelled_gesture_releases_the_latch() {
        let mut c = ScrollChain::default();
        assert_eq!(c.route(began(-6.0, 0.0), true, || ROOM_Y), Route::Forward);
        assert_eq!(
            c.route(ev(phase::CANCELLED, phase::NONE, 0.0, 0.0), true, || ROOM_Y),
            Route::Forward
        );
        assert_eq!(c.latched(), None);
    }

    #[test]
    fn a_new_gesture_replaces_a_latch_whose_end_never_arrived() {
        // Under responsive scrolling AppKit may track a gesture's tail
        // itself, so the end can skip `scrollWheel:`; the next start must
        // still be judged on its own.
        let mut c = ScrollChain::default();
        assert_eq!(c.route(began(-6.0, 0.0), true, || ROOM_Y), Route::Forward);
        assert_eq!(c.route(began(0.0, -6.0), true, || ROOM_Y), Route::Own);
        assert_eq!(c.route(changed(-6.0, 0.0), true, || ROOM_Y), Route::Own);
    }

    #[test]
    fn momentum_without_a_latch_stays_put() {
        let mut c = ScrollChain::default();
        assert_eq!(
            c.route(momentum(phase::CHANGED, -9.0, 0.0), true, || ROOM_Y),
            Route::Own
        );
        assert_eq!(c.route(changed(-9.0, 0.0), true, || ROOM_Y), Route::Own);
    }

    #[test]
    fn mouse_wheel_ticks_are_judged_one_by_one() {
        let mut list = ScrollChain::default();
        // A plain wheel is vertical: the list scrolls.
        assert_eq!(list.route(wheel(0.0, -3.0), true, || ROOM_Y), Route::Own);
        // Shift+wheel arrives as dx: the board scrolls.
        assert_eq!(
            list.route(wheel(-3.0, 0.0), true, || ROOM_Y),
            Route::Forward
        );
        assert_eq!(list.latched(), None, "a tick latches nothing");
        assert_eq!(list.route(wheel(0.0, -3.0), true, || ROOM_Y), Route::Own);
        // A horizontal board gets a plain wheel it cannot use and passes it on.
        let mut board = ScrollChain::default();
        assert_eq!(
            board.route(wheel(0.0, -3.0), true, || ROOM_X),
            Route::Forward
        );
        // An empty tick has no direction to forward.
        assert_eq!(list.route(wheel(0.0, 0.0), true, || ROOM_Y), Route::Own);
    }

    #[test]
    fn overscroll_contain_and_none_never_chain() {
        let mut list = ScrollChain {
            contain: true,
            ..Default::default()
        };
        let reach = || panic!("a contained scroller does not judge");
        assert_eq!(list.route(began(-8.0, 0.0), true, reach), Route::Own);
        assert_eq!(list.route(changed(-8.0, 0.0), true, reach), Route::Own);
        assert_eq!(
            list.route(momentum(phase::BEGAN, -8.0, 0.0), true, reach),
            Route::Own
        );
        assert_eq!(list.route(wheel(-3.0, 0.0), true, reach), Route::Own);

        // Back to `auto`: chaining resumes at the next gesture.
        list.contain = false;
        assert_eq!(
            list.route(began(-8.0, 0.0), true, || ROOM_Y),
            Route::Forward
        );
    }

    #[test]
    fn contain_set_mid_gesture_keeps_the_rest_of_it() {
        let mut list = ScrollChain::default();
        assert_eq!(
            list.route(began(-8.0, 0.0), true, || ROOM_Y),
            Route::Forward
        );
        list.contain = true;
        assert_eq!(list.route(changed(-8.0, 0.0), true, || ROOM_Y), Route::Own);
        assert_eq!(list.latched(), None);
    }

    #[test]
    fn without_an_enclosing_scroll_view_the_gesture_stays() {
        let mut list = ScrollChain::default();
        assert_eq!(list.route(began(-8.0, 0.0), false, || ROOM_Y), Route::Own);
        assert_eq!(list.route(wheel(-3.0, 0.0), false, || ROOM_Y), Route::Own);
    }

    /// AppKit's rule as `NSView.h` states it: a scroll view subclass that
    /// overrides `scrollWheel:` loses responsive scrolling unless it opts
    /// back in. The class query reads method tables only (no view, no
    /// window), so it runs off the main thread.
    #[test]
    fn chained_scroll_views_keep_responsive_scrolling() {
        let stock: bool = unsafe {
            msg_send![
                objc2::class!(NSScrollView),
                isCompatibleWithResponsiveScrolling
            ]
        };
        assert!(stock, "the stock NSScrollView is compatible");
        let chained: bool = unsafe {
            msg_send![
                ChainedScrollView::class(),
                isCompatibleWithResponsiveScrolling
            ]
        };
        assert!(
            chained,
            "ChainedScrollView opts back into responsive scrolling"
        );
        let virt: bool = unsafe {
            msg_send![
                crate::imp::virtualizer::VirtualizerScrollView::class(),
                isCompatibleWithResponsiveScrolling
            ]
        };
        assert!(
            virt,
            "VirtualizerScrollView opts back into responsive scrolling"
        );
        let bare: bool = unsafe {
            msg_send![
                BareWheelScrollView::class(),
                isCompatibleWithResponsiveScrolling
            ]
        };
        assert!(
            !bare,
            "AppKit turns responsive scrolling off for a bare scrollWheel: override"
        );
    }

    declare_class!(
        /// Overrides `scrollWheel:` without the opt-in: the control for
        /// `chained_scroll_views_keep_responsive_scrolling`.
        struct BareWheelScrollView;

        unsafe impl ClassType for BareWheelScrollView {
            type Super = NSScrollView;
            type Mutability = mutability::MainThreadOnly;
            const NAME: &'static str = "IdealystMacScrollChainTestBareWheelScrollView";
        }

        impl DeclaredClass for BareWheelScrollView {}

        unsafe impl BareWheelScrollView {
            #[method(scrollWheel:)]
            fn scroll_wheel(&self, event: &NSEvent) {
                let _: () = unsafe { msg_send![super(self), scrollWheel: event] };
            }
        }
    );
}
