//! Shared brand assets — the Idealyst lightbulb logo as `IconData`,
//! used in the hero, the sidebar header, and anywhere else the mark
//! shows up. Lives at the crate root so pages and shell don't
//! cross-import each other for it.

use runtime_core::{FillRule, IconData};

/// Lightbulb logo, traced from `examples/website/assets/light.svg`,
/// rescaled from 235×235 to a 48×48 viewBox. Authored as TWO
/// subpaths inside one `<path>` so the draw-in animation reads
/// bottom-up cleanly:
///
/// 1. First subpath: threads + the swoop curve that connects them to
///    the bulb's open base, drawn from the bottom of the threads
///    upward and ending at the right side of the bulb base.
/// 2. Second subpath: the bulb body, drawn from the right base as a
///    single continuous arc up around the top and down to the left
///    base.
///
/// SVG dasharray traverses both subpaths in document order (the `M`
/// command consumes zero length), so `draw_in` reveals subpath 1
/// fully, then subpath 2 fully — never showing a "broken bulb" mid-
/// frame the way a single reversed path did.
pub const LIGHT_LOGO: IconData = IconData {
    view_box: (48, 48),
    paths: &[
        "M20.119 41.566C21.957 42.179 26.553 41.668 26.553 40.443C26.553 39.217 20.119 39.728 \
         20.119 37.889C20.119 36.664 26.553 38.604 26.86 34.621\
         M26.86 34.621C26.86 26.349 31.945 23.081 32.477 17.872C33.15 11.268 30.332 6.74 \
         24.204 6.332C18.077 6.536 15.319 10.621 15.319 17.872C15.932 22.774 20.119 27.268 \
         20.119 34.621",
    ],
    fill_rule: FillRule::NonZero,
};
