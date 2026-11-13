//! Pure decisions made before a call reaches Android: the screenshot rate
//! gate, gesture argument checks and the search limits. Un-gated so they are tested on any host.

use std::time::Duration;

use crate::{Error, Result};

/// Android refuses an accessibility screenshot requested sooner than this
/// after the previous one (`AccessibilityService`'s hidden
/// `ACCESSIBILITY_TAKE_SCREENSHOT_REQUEST_INTERVAL_TIMES_MS`), failing it
/// with `ERROR_TAKE_SCREENSHOT_INTERVAL_TIME_SHORT` and no hint of when to
/// retry. The SDK gates on the same interval so it can say when.
pub(crate) const SCREENSHOT_INTERVAL_MS: u32 = 333;

/// Android's limit on one gesture's length
/// (`GestureDescription.getMaxGestureDuration`).
pub(crate) const MAX_GESTURE_MS: u32 = 60_000;

/// How long a [`tap`](crate::tap) holds the point down: well under
/// `ViewConfiguration.getTapTimeout` (100 ms), so no view reads it as the
/// start of a long press.
pub(crate) const TAP_MS: u32 = 50;

/// How often [`wait_for`](crate::wait_for) searches the screen again. A
/// search walks every window's tree, a call into each app per element, so
/// this keeps a long wait from loading the device while still answering a
/// dialog within a quarter of a second.
pub(crate) const WAIT_POLL_MS: u32 = 250;

/// How many found elements the platform half keeps for
/// [`click`](crate::click). A busy screen has a few hundred elements, so an
/// empty query's results still fit; past this the oldest are dropped.
pub(crate) const NODE_HANDLES: usize = 512;

/// `Some(ms)` to wait before a screenshot may be requested, given the time
/// since the last accepted request (`None`: there was none).
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn screenshot_retry_after(since_last: Option<Duration>) -> Option<u32> {
    let elapsed = since_last?.as_millis();
    let interval = u128::from(SCREENSHOT_INTERVAL_MS);
    (elapsed < interval).then(|| (interval - elapsed) as u32)
}

/// A gesture point: finite and not negative. Android rejects a path with
/// negative bounds; the upper bound (the screen's size) is checked by the
/// platform half, which knows it.
pub(crate) fn check_point(x: f32, y: f32) -> Result<()> {
    if x.is_finite() && y.is_finite() && x >= 0.0 && y >= 0.0 {
        Ok(())
    } else {
        Err(Error::InvalidArgument(format!(
            "({x}, {y}) is not a point on the screen"
        )))
    }
}

/// A gesture duration: at least 1 ms, at most [`MAX_GESTURE_MS`].
pub(crate) fn check_duration(ms: u32) -> Result<()> {
    if (1..=MAX_GESTURE_MS).contains(&ms) {
        Ok(())
    } else {
        Err(Error::InvalidArgument(format!(
            "a gesture lasts 1 to {MAX_GESTURE_MS} ms, not {ms}"
        )))
    }
}

/// JPEG quality, 0–100 as `Bitmap.compress` takes it.
pub(crate) fn check_quality(quality: u8) -> Result<()> {
    if quality <= 100 {
        Ok(())
    } else {
        Err(Error::InvalidArgument(format!("JPEG quality is 0 to 100, not {quality}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_screenshot_is_never_too_soon() {
        assert_eq!(screenshot_retry_after(None), None);
    }

    #[test]
    fn a_screenshot_inside_the_interval_says_how_long_to_wait() {
        assert_eq!(screenshot_retry_after(Some(Duration::from_millis(0))), Some(333));
        assert_eq!(screenshot_retry_after(Some(Duration::from_millis(100))), Some(233));
        assert_eq!(screenshot_retry_after(Some(Duration::from_millis(332))), Some(1));
    }

    #[test]
    fn a_screenshot_at_or_past_the_interval_goes_ahead() {
        assert_eq!(screenshot_retry_after(Some(Duration::from_millis(333))), None);
        assert_eq!(screenshot_retry_after(Some(Duration::from_secs(5))), None);
    }

    #[test]
    fn gesture_points_must_be_finite_and_not_negative() {
        assert!(check_point(0.0, 0.0).is_ok());
        assert!(check_point(2559.0, 1599.5).is_ok());
        for (x, y) in [(-1.0, 0.0), (0.0, -0.5), (f32::NAN, 1.0), (1.0, f32::INFINITY)] {
            assert!(matches!(check_point(x, y), Err(Error::InvalidArgument(_))), "({x}, {y})");
        }
    }

    #[test]
    fn gesture_durations_are_bounded_by_android() {
        assert!(check_duration(0).is_err());
        assert!(check_duration(1).is_ok());
        assert!(check_duration(TAP_MS).is_ok());
        assert!(check_duration(MAX_GESTURE_MS).is_ok());
        assert!(check_duration(MAX_GESTURE_MS + 1).is_err());
    }

    #[test]
    fn jpeg_quality_is_a_percentage() {
        assert!(check_quality(0).is_ok());
        assert!(check_quality(100).is_ok());
        assert!(check_quality(101).is_err());
    }
}
