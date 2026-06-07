//! Stroke color resolution: the adaptive `INK` slot and `ink`-stroke re-resolve.

use crate::{
    parse_rgba, resolve_color, stroke_color, CanvasBg, Stroke, INK, INK_ON_DARK, INK_ON_LIGHT,
};

// Regression for "the first palette color should contrast the backdrop": the
// `INK` slot resolves to a light ink on a dark canvas and a dark ink on a light
// one, so the default stroke is always visible. Non-ink entries pass through.
#[test]
fn ink_contrasts_explicit_canvas_colors() {
    assert_eq!(resolve_color(INK, CanvasBg::White, false), INK_ON_LIGHT);
    assert_eq!(resolve_color(INK, CanvasBg::Paper, false), INK_ON_LIGHT);
    assert_eq!(resolve_color(INK, CanvasBg::Slate, false), INK_ON_LIGHT);
    assert_eq!(resolve_color(INK, CanvasBg::Charcoal, false), INK_ON_DARK);
    assert_eq!(resolve_color(INK, CanvasBg::Black, false), INK_ON_DARK);
}

#[test]
fn ink_follows_auto_canvas_through_theme() {
    // Auto canvas tracks the theme: white in light → dark ink; near-black in
    // dark → light ink.
    assert_eq!(resolve_color(INK, CanvasBg::Auto, false), INK_ON_LIGHT);
    assert_eq!(resolve_color(INK, CanvasBg::Auto, true), INK_ON_DARK);
}

#[test]
fn non_ink_entries_pass_through_unchanged() {
    assert_eq!(resolve_color("#ef4444", CanvasBg::Black, true), "#ef4444");
    assert_eq!(resolve_color("#3b82f6", CanvasBg::White, false), "#3b82f6");
}

// Regression for "update the stroke color if it uses the contrast color": an
// `ink` stroke re-resolves against whatever the backdrop currently is, so it
// flips light↔dark when the canvas color/theme changes and never goes
// invisible. A fixed-hue stroke keeps its snapshot regardless.
#[test]
fn ink_stroke_tracks_backdrop_fixed_does_not() {
    let ink = Stroke {
        points: vec![],
        width: 2.0,
        rgba: parse_rgba(INK_ON_LIGHT),
        ink: true,
    };
    assert_eq!(stroke_color(&ink, CanvasBg::White, false), parse_rgba(INK_ON_LIGHT));
    assert_eq!(stroke_color(&ink, CanvasBg::Black, false), parse_rgba(INK_ON_DARK));
    assert_eq!(stroke_color(&ink, CanvasBg::Auto, true), parse_rgba(INK_ON_DARK));

    let red = Stroke { points: vec![], width: 2.0, rgba: (239, 68, 68, 255), ink: false };
    assert_eq!(stroke_color(&red, CanvasBg::White, false), (239, 68, 68, 255));
    assert_eq!(stroke_color(&red, CanvasBg::Black, true), (239, 68, 68, 255));
}
