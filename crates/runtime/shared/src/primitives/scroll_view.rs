//! ScrollView primitive.
//!
//! Backed by a `<div style="overflow: auto">` on web, `UIScrollView`
//! on iOS, `NSScrollView` on macOS, `ScrollView` /
//! `HorizontalScrollView` / a two-axis scroller on Android. Which axes
//! scroll is a [`ScrollAxis`]: vertical by default, `.horizontal(true)`
//! for left-right, `.axis(ScrollAxis::Both)` for content that overflows
//! in both directions (a wide, tall table). On a two-axis scroller
//! `on_scroll` reports both offsets and `Position::Sticky` children pin
//! on both axes at once — a `top: 0` + `left: 0` corner cell stays put
//! whichever way the reader scrolls.

use std::any::Any;
use std::rc::Rc;

/// Which axes a `scroll_view` scrolls.
///
/// The layout treats every scrolling axis as unbounded for the content
/// (`overflow: scroll` on that axis in Taffy, and the content exempt
/// from the default `max-width: 100%` clamp when X scrolls); an axis
/// that does not scroll clips its content to the viewport.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "remote-serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ScrollAxis {
    /// Top-to-bottom scrolling. The default.
    #[default]
    Vertical,
    /// Left-to-right scrolling.
    Horizontal,
    /// Both at once: the content may overflow the viewport in either
    /// direction. A drag, swipe or wheel gesture that starts mostly along
    /// one axis (by 2:1 or more) stays locked to it, so scrolling down a
    /// wide table does not drift it sideways; a diagonal one pans freely.
    /// Every backend does this — UIKit's directional lock, Android's
    /// two-axis scroller, AppKit's predominant-axis scrolling, and on the
    /// web the browser's own touch railing plus a wheel lock.
    Both,
}

impl ScrollAxis {
    /// `Horizontal` for `true`, `Vertical` for `false` — the meaning of
    /// the `.horizontal(bool)` builder setter.
    pub fn from_horizontal(horizontal: bool) -> Self {
        if horizontal {
            Self::Horizontal
        } else {
            Self::Vertical
        }
    }

    /// Whether the X axis scrolls (`Horizontal` or `Both`).
    pub fn scrolls_x(self) -> bool {
        matches!(self, Self::Horizontal | Self::Both)
    }

    /// Whether the Y axis scrolls (`Vertical` or `Both`).
    pub fn scrolls_y(self) -> bool {
        matches!(self, Self::Vertical | Self::Both)
    }

    /// The axis `on_end_reached` watches: X for a horizontal scroller,
    /// Y otherwise. A two-axis scroller's "end" is its bottom — paging a
    /// wide table grows it downward, not sideways.
    pub fn end_axis_is_horizontal(self) -> bool {
        self == Self::Horizontal
    }
}

#[derive(Clone)]
pub struct ScrollViewHandle {
    node: Rc<dyn Any>,
    ops: &'static dyn ScrollViewOps,
}

impl ScrollViewHandle {
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn ScrollViewOps) -> Self {
        Self { node, ops }
    }

    /// Scroll to absolute coordinates within the view's content box.
    /// Each coordinate is meaningful on an axis that scrolls (both on a
    /// [`ScrollAxis::Both`] scroller); a fixed axis ignores its own.
    pub fn scroll_to(&self, x: f32, y: f32) {
        self.ops.scroll_to(&*self.node, x, y);
    }

    /// Convenience: scroll to (0, 0). Common enough to warrant its
    /// own method.
    pub fn scroll_to_top(&self) {
        self.ops.scroll_to(&*self.node, 0.0, 0.0);
    }
}

pub trait ScrollViewOps {
    fn scroll_to(&self, node: &dyn Any, x: f32, y: f32);
}



/// Whether a scroller has arrived at its end, and whether that arrival
/// is NEW.
///
/// Every backend that can answer "how far is the reader from the
/// bottom" needs the same two rules on top of the raw numbers, so they
/// live here rather than three times in three backends:
///
/// 1. **Edge-triggered.** A scroller at rest at its end delivers scroll
///    events for every rubber-band twitch. Firing on each one turns
///    "load the next page" into a request storm. It fires on the way
///    IN and stays quiet until the reader leaves.
/// 2. **Re-arms on leaving.** Once the reader is back above the
///    threshold the next arrival counts again — which is what makes it
///    work for a list that GREW because of the last one.
///
/// Pure, so the rules can be tested without a scroller.
#[derive(Debug, Clone)]
pub struct EndReach {
    /// How close to the end counts as arriving, in logical px.
    threshold: f32,
    /// False while the reader is inside the zone, so the callback does
    /// not repeat.
    armed: bool,
}

impl EndReach {
    pub fn new(threshold: f32) -> Self {
        Self {
            threshold: threshold.max(0.0),
            armed: true,
        }
    }

    /// Feed one scroll sample. `true` means "fire now".
    ///
    /// `content` shorter than `viewport` never fires: there is no end
    /// to arrive at, and a list that fits its viewport would otherwise
    /// ask for its next page the moment it mounted — the failure that
    /// looks like an infinite loop rather than a scroll bug.
    pub fn update(&mut self, offset: f32, viewport: f32, content: f32) -> bool {
        if content <= viewport {
            // Nothing to scroll, so nothing to arrive at — but stay
            // ARMED. Disarming here is a dead end: the reader can only
            // re-arm by leaving the zone, and when the content next
            // grows to within `threshold` of the viewport the zone
            // spans the whole travel, so there is nowhere to leave to.
            // A list that mounts empty and is handed a first page
            // shorter than its own prefetch threshold would then never
            // ask for a second one. `new` starts armed for the same
            // reason: a scroller with nothing to scroll is in the same
            // state as one that has not scrolled.
            self.armed = true;
            return false;
        }
        // Clamped: iOS rubber-banding reports offsets past the end and
        // below zero, and a negative remaining distance is still
        // "arrived".
        let remaining = (content - viewport - offset).max(0.0);
        let inside = remaining <= self.threshold;
        if inside && self.armed {
            self.armed = false;
            return true;
        }
        if !inside {
            self.armed = true;
        }
        false
    }
}

#[cfg(test)]
mod axis_tests {
    use super::ScrollAxis;

    #[test]
    fn each_axis_reports_what_it_scrolls() {
        assert!(ScrollAxis::Vertical.scrolls_y() && !ScrollAxis::Vertical.scrolls_x());
        assert!(ScrollAxis::Horizontal.scrolls_x() && !ScrollAxis::Horizontal.scrolls_y());
        assert!(ScrollAxis::Both.scrolls_x() && ScrollAxis::Both.scrolls_y());
        assert_eq!(ScrollAxis::default(), ScrollAxis::Vertical);
    }

    #[test]
    fn horizontal_bool_maps_to_the_single_axes() {
        assert_eq!(ScrollAxis::from_horizontal(true), ScrollAxis::Horizontal);
        assert_eq!(ScrollAxis::from_horizontal(false), ScrollAxis::Vertical);
    }

    /// A two-axis scroller pages downward: its `on_end_reached` watches
    /// Y, like a vertical one.
    #[test]
    fn a_two_axis_scroller_reaches_its_end_at_the_bottom() {
        assert!(!ScrollAxis::Both.end_axis_is_horizontal());
        assert!(!ScrollAxis::Vertical.end_axis_is_horizontal());
        assert!(ScrollAxis::Horizontal.end_axis_is_horizontal());
    }
}

#[cfg(test)]
mod end_reach_tests {
    use super::EndReach;

    /// It fires on the way IN and then holds its tongue. A scroller
    /// resting at the bottom delivers an event per twitch, and a page
    /// fetch per twitch is a request storm.
    #[test]
    fn it_fires_once_on_arrival() {
        let mut e = EndReach::new(0.0);
        assert!(!e.update(0.0, 500.0, 2000.0));
        assert!(!e.update(1000.0, 500.0, 2000.0));
        assert!(e.update(1500.0, 500.0, 2000.0), "arrived");
        assert!(!e.update(1500.0, 500.0, 2000.0), "still there");
        assert!(!e.update(1502.0, 500.0, 2000.0), "rubber-banding past it");
    }

    /// Leaving re-arms it, which is what serves a list that grew
    /// BECAUSE of the last arrival: new content, new end, new arrival.
    #[test]
    fn leaving_re_arms_it() {
        let mut e = EndReach::new(0.0);
        assert!(e.update(1500.0, 500.0, 2000.0));
        assert!(!e.update(1400.0, 500.0, 2000.0), "left the zone");
        assert!(e.update(1500.0, 500.0, 2000.0), "and came back");
        // The list grew: the end moved, so the reader is no longer at
        // it, and reaching the NEW end fires again.
        assert!(!e.update(1500.0, 500.0, 4000.0));
        assert!(e.update(3500.0, 500.0, 4000.0));
    }

    /// The threshold is a distance from the end, not a position.
    #[test]
    fn the_threshold_measures_from_the_end() {
        let mut e = EndReach::new(200.0);
        assert!(!e.update(1200.0, 500.0, 2000.0), "300 to go");
        assert!(e.update(1350.0, 500.0, 2000.0), "150 to go");
    }

    /// Regression: a paging list that mounts empty and is then handed a
    /// first page shorter than its own prefetch threshold must still ask
    /// for the second one.
    ///
    /// If the empty mount DISARMS (`content <= viewport`), the only way
    /// back to armed is leaving the zone. When the threshold is
    /// wider than the travel — 500pt of prefetch over 300pt of
    /// scrollable content, which is what "a screenful" means on a short
    /// page — there is nowhere to leave to, so it stays disarmed forever
    /// and the list stops at page 1. `end_reached_threshold`'s
    /// own docs recommend the configuration that triggered it.
    #[test]
    fn regression_a_short_first_page_after_an_empty_mount_still_fires() {
        let mut e = EndReach::new(500.0);
        assert!(!e.update(0.0, 500.0, 0.0), "mounts empty: nothing to scroll");
        assert!(
            e.update(0.0, 500.0, 800.0),
            "page 1 lands entirely inside the prefetch zone — ask for page 2"
        );
        // Still edge-triggered: sitting there does not re-ask.
        assert!(!e.update(0.0, 500.0, 800.0));
        assert!(!e.update(300.0, 500.0, 800.0), "scrolled to the very end");
        // Page 2 lands and the end moves out of reach again, so the next
        // approach is a fresh arrival.
        assert!(!e.update(300.0, 500.0, 3000.0));
        assert!(e.update(2100.0, 500.0, 3000.0));
    }

    /// A list emptied back to nothing (a filter clearing it) returns to
    /// the state a fresh scroller is in, so refilling it asks again.
    #[test]
    fn emptying_a_list_re_arms_it() {
        let mut e = EndReach::new(0.0);
        assert!(e.update(1500.0, 500.0, 2000.0), "arrived once");
        assert!(!e.update(0.0, 500.0, 0.0), "filtered down to nothing");
        assert!(e.update(1500.0, 500.0, 2000.0), "refilled and back at the end");
    }

    /// Content that fits its viewport has no end to arrive at. Firing
    /// here is the bug that reads as an infinite loop: the list asks
    /// for its next page the instant it mounts, and again for every
    /// page it is given.
    #[test]
    fn content_that_fits_never_arrives() {
        let mut e = EndReach::new(0.0);
        assert!(!e.update(0.0, 500.0, 500.0));
        assert!(!e.update(0.0, 500.0, 100.0));
        // And it stays quiet once the content grows past the viewport
        // until the reader actually travels.
        assert!(!e.update(0.0, 500.0, 2000.0));
        assert!(e.update(1500.0, 500.0, 2000.0));
    }
}
