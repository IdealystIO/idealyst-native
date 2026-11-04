//! The arithmetic of `on_end_reached` on Android. Un-gated so the regression
//! tests run on the host; the JNI half lives in
//! `imp/primitives/scroll_view.rs`.
//!
//! The arm / re-arm rule is `EndReach`'s, shared with every other backend.
//! What is Android's own is the unit: everything the toolkit reports is in
//! whole device pixels, and the comparison stays there. Converting the three
//! readings to dp first loses the end: `(content - viewport - offset)` over a
//! fractional density comes out a few millionths above zero at the very
//! bottom, which a threshold of `0` reads as "not there yet", so the callback
//! never fires on those densities.

use runtime_shared::primitives::scroll_view::EndReach;

/// One scroller's [`EndReach`], fed in device pixels.
pub(crate) struct EndWatch {
    reach: EndReach,
}

impl EndWatch {
    /// `threshold_dp` is the author's distance from the end.
    pub(crate) fn new(threshold_dp: f32, density: f32) -> Self {
        Self { reach: EndReach::new(threshold_dp * density) }
    }

    /// Whether this reading is an arrival.
    pub(crate) fn observe(&mut self, offset_px: i32, viewport_px: i32, content_px: i32) -> bool {
        self.reach.update(offset_px as f32, viewport_px as f32, content_px as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A device-pixel scroller's bottom has to read as the end on every
    /// density, including the fractional ones phones ship with. The dp form
    /// of this same reading misses it on several of these.
    #[test]
    fn regression_android_end_reached_at_a_fractional_density() {
        for density in [1.0f32, 1.5, 2.0, 2.625, 2.75, 3.0, 3.5] {
            for content in (1000..4000).step_by(97) {
                for viewport in [317, 640, 777] {
                    let offset = content - viewport;
                    let mut watch = EndWatch::new(0.0, density);
                    assert!(
                        watch.observe(offset, viewport, content),
                        "density {density}: {offset} + {viewport} of {content} is the end"
                    );
                }
            }
        }
    }

    #[test]
    fn the_threshold_is_in_dp() {
        // 100dp at 3x is 300px: 301px short of the end is outside, 299 inside.
        let mut watch = EndWatch::new(100.0, 3.0);
        assert!(!watch.observe(699, 1000, 2000));
        assert!(watch.observe(701, 1000, 2000));
    }

    #[test]
    fn it_fires_once_per_arrival_like_every_other_backend() {
        let mut watch = EndWatch::new(0.0, 2.0);
        assert!(!watch.observe(0, 500, 2000));
        assert!(watch.observe(1500, 500, 2000));
        assert!(!watch.observe(1500, 500, 2000), "still there");
        assert!(!watch.observe(1400, 500, 2000), "left");
        assert!(watch.observe(1500, 500, 2000), "and came back");
    }
}
