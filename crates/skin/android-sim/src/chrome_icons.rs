//! Material-style chrome icons used by navigator headers.
//!
//! Mirror shape of `ios-sim/chrome_icons.rs` but with Material
//! Symbols proportions: heavier strokes, sharper corners on the
//! hamburger, an arrow back (not a chevron) for the up button.
//! Authored in 24×24 viewBox to match the framework's
//! `IconData::view_box` convention.
//!
//! Same rationale as iOS: minted in-crate rather than pulled
//! from Lucide so the chrome can revise its glyphs without
//! coupling to a third-party icon pack. Adding new icons works
//! the same way — drop in a 24×24 path, reference from
//! `paint_navigator_header`.

#[derive(Copy, Clone)]
pub struct ChromeIcon {
    pub paths: &'static [&'static str],
    pub view_box: (u16, u16),
}

/// Material's back arrow — a horizontal shaft with an
/// arrowhead, not a chevron. Maps to the framework's
/// "chevron.left" name because the framework is platform-
/// agnostic: the skin decides what "go back" looks like.
pub const BACK_ARROW: ChromeIcon = ChromeIcon {
    paths: &[
        "M 4 12 L 20 12",
        "M 4 12 L 11 5",
        "M 4 12 L 11 19",
    ],
    view_box: (24, 24),
};

/// Hamburger menu — three horizontal bars. Slightly tighter
/// spacing than iOS to match Material Symbols' density.
pub const HAMBURGER: ChromeIcon = ChromeIcon {
    paths: &[
        "M 3 6 L 21 6",
        "M 3 12 L 21 12",
        "M 3 18 L 21 18",
    ],
    view_box: (24, 24),
};

/// Close (X). Used by header slots on screens presented modally
/// from the bottom — Material's "dismiss this sheet" affordance.
pub const CLOSE: ChromeIcon = ChromeIcon {
    paths: &["M 5 5 L 19 19", "M 19 5 L 5 19"],
    view_box: (24, 24),
};

/// Map an icon name to a local Material glyph. The framework
/// uses SF-Symbol-style names by convention; we accept those
/// plus the Material equivalents.
pub fn lookup(name: &str) -> Option<ChromeIcon> {
    Some(match name {
        "chevron.left" | "arrow.left" | "back" | "arrow_back" => BACK_ARROW,
        "line.3.horizontal" | "menu" | "hamburger" => HAMBURGER,
        "xmark" | "close" | "x" => CLOSE,
        _ => return None,
    })
}
