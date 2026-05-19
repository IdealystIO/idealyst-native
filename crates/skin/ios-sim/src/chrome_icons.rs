//! iOS-style chrome icons used by navigator headers.
//!
//! Each icon is a tiny SVG-path-string set rendered inline by
//! the wgpu renderer's `paint_icon`. We mint our own (instead
//! of pulling from Lucide) because UIKit chrome has a distinct
//! visual signature: thin strokes, sharp inside corners on
//! chevrons, no rounded caps. Lucide's set is closer to
//! Android Material in feel and would look out of place under
//! the iOS skin.
//!
//! Icons are designed in a 24×24 viewBox to match the
//! framework's `IconData::view_box` convention. Stroke widths
//! are inherent to the path — the wgpu renderer doesn't carry a
//! stroke-thickness setting, so each path encodes the stroke
//! shape directly via consecutive line segments.
//!
//! These are deliberately not exposed as a public API; the iOS
//! skin's `paint_navigator_header` is the only caller and we
//! want to be able to revise the icon shapes without breaking
//! downstream code.
//!
//! Adding a new icon: pick a 24×24 design (1.5pt stroke is the
//! iOS chrome standard), trace it as a path string using
//! `M`/`L`/`C` SVG commands, drop it in here as a `ChromeIcon`,
//! and reference from `paint_navigator_header`.

/// A pre-baked chrome glyph. Mirrors the shape of
/// `framework_core::primitives::icon::IconData` so the wgpu
/// `paint_icon` helper can consume it directly.
#[derive(Copy, Clone)]
pub struct ChromeIcon {
    pub paths: &'static [&'static str],
    pub view_box: (u16, u16),
}

/// The back chevron used by stack navigators. Thin, pointy —
/// matches `UINavigationBar`'s default. Sits inside a 24×24
/// box with the apex roughly centered vertically.
pub const BACK_CHEVRON: ChromeIcon = ChromeIcon {
    paths: &["M 15 4 L 7 12 L 15 20"],
    view_box: (24, 24),
};

/// The hamburger menu used by drawer navigators. Three short
/// horizontal strokes — iOS doesn't ship a stock hamburger
/// (SF Symbols has `line.3.horizontal`); this matches that
/// glyph's proportions.
pub const HAMBURGER: ChromeIcon = ChromeIcon {
    paths: &[
        "M 4 7 L 20 7",
        "M 4 12 L 20 12",
        "M 4 17 L 20 17",
    ],
    view_box: (24, 24),
};

/// Close / dismiss glyph (X). Used by header_left on
/// modally-presented screens.
pub const CLOSE: ChromeIcon = ChromeIcon {
    paths: &["M 6 6 L 18 18", "M 18 6 L 6 18"],
    view_box: (24, 24),
};

/// Look up an icon by its SF-Symbol-style name. The framework's
/// `HeaderButton.icon` carries a `String` that conventionally
/// uses Apple's naming (e.g. "chevron.left", "line.3.horizontal");
/// this maps a handful of those to our local shapes. Returns
/// `None` if no match — the skin's header paint falls back to a
/// labelled placeholder so the slot stays visible during dev.
pub fn lookup(name: &str) -> Option<ChromeIcon> {
    Some(match name {
        "chevron.left" | "arrow.left" | "back" => BACK_CHEVRON,
        "line.3.horizontal" | "menu" | "hamburger" => HAMBURGER,
        "xmark" | "close" | "x" => CLOSE,
        _ => return None,
    })
}
