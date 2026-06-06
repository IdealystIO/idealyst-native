//! Border-routing decision shared by the iOS and macOS backends.
//!
//! Both UIKit and AppKit expose the same two ways to stroke a border,
//! with the same sharp split:
//!
//!   * `CALayer.borderWidth`/`borderColor` strokes ONE uniform border
//!     that follows the layer's `cornerRadius` exactly — the stroke
//!     curves around rounded corners with no seams.
//!   * Per-side `UIView`/`NSView` bars can express asymmetric
//!     widths/colors, but each bar is a straight rectangle. With a corner
//!     radius the parent's clip mask slices the ends off every bar,
//!     leaving notches/gaps at each corner — a straight bar can't trace a
//!     curve.
//!
//! So each backend routes uniform borders (the common card) through
//! CALayer and reserves per-side bars for the genuinely asymmetric case
//! CALayer can't represent. This module owns that decision so it's
//! unit-tested once on the host and the two backends converge byte for
//! byte (Rule #7) — see [`uniform_border`].
//!
//! NOT OS-gated — pure `runtime_core` logic, so it builds and tests on the
//! host while iOS + macOS share one source of truth.

use runtime_core::Color;

/// Decide whether a per-side border collapses to a single uniform
/// CALayer stroke. `widths` and `colors` are the four resolved sides
/// in `[top, right, bottom, left]` order; a `None` color falls back to
/// the first author-supplied color (matching the per-side bar path) so
/// `border_width` set without an explicit color still counts as
/// uniform.
///
/// Returns `Some((width, color))` when all four sides share the same
/// width and effective color — the caller strokes the layer, which
/// traces `cornerRadius` cleanly. Returns `None` for the asymmetric
/// case (e.g. a `border-bottom`-only spec), where the caller draws
/// straight per-side bars.
pub fn uniform_border(widths: [f32; 4], colors: &[Option<Color>; 4]) -> Option<(f32, Color)> {
    if !widths.iter().all(|w| (*w - widths[0]).abs() < f32::EPSILON) {
        return None;
    }
    let fallback = colors.iter().find_map(|c| c.clone());
    let eff: Vec<Option<Color>> =
        colors.iter().map(|c| c.clone().or_else(|| fallback.clone())).collect();
    let first = eff[0].clone()?;
    if eff.iter().all(|c| c.as_ref() == Some(&first)) {
        Some((widths[0], first))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(s: &str) -> Option<Color> {
        Some(Color(s.to_string()))
    }

    // The bug: a uniform border on a rounded card was drawn as four
    // straight bars, whose corners get sliced by the parent's
    // rounded-corner clip mask, leaving notches. The fix routes the
    // uniform case to a CALayer stroke (which follows cornerRadius).
    // This test pins the routing decision that makes that happen.
    #[test]
    fn regression_uniform_rounded_border_uses_calayer() {
        // All four sides identical width + color → collapse to CALayer.
        let widths = [1.0; 4];
        let colors = [col("#e5e5e5"), col("#e5e5e5"), col("#e5e5e5"), col("#e5e5e5")];
        assert_eq!(uniform_border(widths, &colors), Some((1.0, Color("#e5e5e5".into()))));
    }

    #[test]
    fn width_without_per_side_color_falls_back_and_collapses() {
        // Author set a single border color (top) + equal widths; the
        // fallback fills the other sides, so it's still uniform.
        let widths = [2.0; 4];
        let colors = [col("#000"), None, None, None];
        assert_eq!(uniform_border(widths, &colors), Some((2.0, Color("#000".into()))));
    }

    #[test]
    fn differing_widths_stay_per_side() {
        // A bottom-only border (the per-side feature this must not
        // regress) must NOT collapse — it needs a single bar.
        let widths = [0.0, 0.0, 1.0, 0.0];
        let colors = [None, None, col("#000"), None];
        assert_eq!(uniform_border(widths, &colors), None);
    }

    #[test]
    fn differing_colors_stay_per_side() {
        // Equal widths but two distinct colors → CALayer can't express
        // it, so keep the per-side bars.
        let widths = [1.0; 4];
        let colors = [col("#f00"), col("#0f0"), col("#f00"), col("#0f0")];
        assert_eq!(uniform_border(widths, &colors), None);
    }
}
