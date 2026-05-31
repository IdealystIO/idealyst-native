//! Shared brand assets — the Idealyst lightbulb logo as `IconData`,
//! used in the hero, the sidebar header, and anywhere else the mark
//! shows up. Lives at the crate root so pages and shell don't
//! cross-import each other for it.

use runtime_core::{FillRule, IconData};

/// Lightbulb logo, traced from `examples/website/assets/light.svg`,
/// rescaled from 235×235 to a 48×48 viewBox. ONE continuous path
/// (single `M`) drawn bottom-up:
///
/// 1. Bottom of the threads up through the zigzags to the junction
///    point at the bottom of the bulb.
/// 2. Swoop curve from the junction up to the right side of the
///    bulb base.
/// 3. Bulb body sweeping from right base around the top and down to
///    the left base.
/// 4. A short closing line from the left base back down to the
///    junction so both sides of the bulb terminate at the same
///    point — the bulb reads as one closed outline rather than an
///    open arc with threads dangling off only one side.
pub const LIGHT_LOGO: IconData = IconData {
    view_box: (48, 48),
    paths: &[
        "M20.119 41.566C21.957 42.179 26.553 41.668 26.553 40.443C26.553 39.217 20.119 39.728 \
         20.119 37.889C20.119 36.664 26.553 38.604 26.86 34.621C26.86 26.349 31.945 23.081 \
         32.477 17.872C33.15 11.268 30.332 6.74 24.204 6.332C18.077 6.536 15.319 10.621 \
         15.319 17.872C15.932 22.774 20.119 27.268 20.119 34.621L20.119 37.889",
    ],
    fill_rule: FillRule::NonZero,
    filled: false,
};
