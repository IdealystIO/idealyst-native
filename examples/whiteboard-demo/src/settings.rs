//! Settings model — the aspect-ratio presets, canvas-background palette, and the
//! geometry that fits the canvas "stage" (an aspect-locked box) centered inside
//! the safe area. Pure data + math; the reactive signals live on
//! [`crate::BoardState`] and the UI in [`crate::screens`].

use runtime_core::{safe_area_insets, viewport_size};

/// Standard aspect-ratio presets, `(label, width, height)`. Portrait `9:16` is
/// the default (a phone-shaped board).
pub(crate) const ASPECTS: &[(&str, u32, u32)] = &[
    ("9:16", 9, 16),
    ("3:4", 3, 4),
    ("1:1", 1, 1),
    ("4:3", 4, 3),
    ("16:9", 16, 9),
];

/// Default aspect (portrait 9:16).
pub(crate) const DEFAULT_ASPECT: (u32, u32) = (9, 16);

/// Bounds for a custom aspect's width/height components — kept small so the
/// stage stays a sane shape (no 1:50 slivers).
pub(crate) const ASPECT_MIN: u32 = 1;
pub(crate) const ASPECT_MAX: u32 = 21;

/// Gap between the stage and the safe-area edges.
pub(crate) const STAGE_MARGIN: f32 = 10.0;

/// The chosen aspect's label if it matches a preset, else `"Custom"`.
pub(crate) fn aspect_label(w: u32, h: u32) -> &'static str {
    ASPECTS
        .iter()
        .find(|(_, aw, ah)| *aw == w && *ah == h)
        .map(|(l, _, _)| *l)
        .unwrap_or("Custom")
}

/// The canvas drawing-surface background. `Auto` follows the app theme (light →
/// white, dark → near-black); the rest are explicit.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CanvasBg {
    Auto,
    White,
    Paper,
    Slate,
    Charcoal,
    Black,
}

/// Selectable canvas backgrounds, `(label, value)` — `Auto` first (the default).
pub(crate) const CANVAS_BGS: &[(&str, CanvasBg)] = &[
    ("Auto", CanvasBg::Auto),
    ("White", CanvasBg::White),
    ("Paper", CanvasBg::Paper),
    ("Slate", CanvasBg::Slate),
    ("Charcoal", CanvasBg::Charcoal),
    ("Black", CanvasBg::Black),
];

impl CanvasBg {
    /// Resolve to an opaque RGB. `Auto` consults `dark` (the app theme).
    pub(crate) fn rgb(self, dark: bool) -> (u8, u8, u8) {
        match self {
            CanvasBg::Auto => {
                if dark {
                    (24, 24, 27)
                } else {
                    (255, 255, 255)
                }
            }
            CanvasBg::White => (255, 255, 255),
            CanvasBg::Paper => (250, 247, 240),
            CanvasBg::Slate => (226, 232, 240),
            CanvasBg::Charcoal => (39, 39, 42),
            CanvasBg::Black => (17, 17, 17),
        }
    }

    /// A small swatch CSS color for the picker (the `Auto` chip shows a neutral
    /// gradient-ish mid-gray since its real color depends on the theme).
    pub(crate) fn swatch_css(self) -> &'static str {
        match self {
            CanvasBg::Auto => "#9ca3af",
            CanvasBg::White => "#ffffff",
            CanvasBg::Paper => "#faf7f0",
            CanvasBg::Slate => "#e2e8f0",
            CanvasBg::Charcoal => "#27272a",
            CanvasBg::Black => "#111111",
        }
    }

}

/// The canvas "stage" rectangle: the largest box of aspect `aw:ah` that fits
/// inside the safe area (minus [`STAGE_MARGIN`]), centered. Returns `(x, y, w,
/// h)` in viewport points. Reads `viewport_size()` + `safe_area_insets()`
/// reactively, so call it inside a reactive style/closure and it follows
/// rotation, the keyboard, and aspect changes.
pub(crate) fn stage_geom(aw: u32, ah: u32) -> (f32, f32, f32, f32) {
    let vp = viewport_size().get();
    let ins = safe_area_insets().get();
    let m = STAGE_MARGIN;
    let avail_w = (vp.width - ins.left - ins.right - 2.0 * m).max(1.0);
    let avail_h = (vp.height - ins.top - ins.bottom - 2.0 * m).max(1.0);
    let ar = aw.max(1) as f32 / ah.max(1) as f32;
    // Fit: if the available area is wider than the target ratio, height binds;
    // otherwise width binds.
    let (w, h) = if avail_w / avail_h > ar {
        (avail_h * ar, avail_h)
    } else {
        (avail_w, avail_w / ar)
    };
    let x = ins.left + m + (avail_w - w) * 0.5;
    let y = ins.top + m + (avail_h - h) * 0.5;
    (x, y, w, h)
}
