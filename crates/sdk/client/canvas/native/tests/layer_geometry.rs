//! Host tests for the Android renderer's texture-layer composite math. The
//! renderer only compiles for Android; the math is pure, so it's included here
//! by path and checked on every host (the same arrangement as `pixels.rs`).

#[path = "../src/layer_geometry.rs"]
mod layer_geometry;

use layer_geometry::{border_argb, source_to_dest};

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

/// Apply the mapping to a source point.
fn map(m: (f32, f32, f32, f32), p: (f32, f32)) -> (f32, f32) {
    (p.0 * m.0 + m.2, p.1 * m.1 + m.3)
}

/// Regression: Android drew texture layers with `drawBitmap(Rect, RectF)`,
/// whose source rect is integer, so a crop starting mid-pixel snapped to a
/// whole pixel and the picture shifted against the other renderers. The
/// mapping must take the fractional source rect's corners exactly onto the
/// destination's corners.
#[test]
fn regression_android_fractional_source_rect_is_not_snapped() {
    // Source x 0.5..2.0 (1.5 px wide, starts mid-pixel) into 0..30 × 0..20.
    let m = source_to_dest((0.5, 0.25, 1.5, 1.0), (0.0, 0.0, 30.0, 20.0)).unwrap();
    let (x0, y0) = map(m, (0.5, 0.25));
    let (x1, y1) = map(m, (2.0, 1.25));
    assert!(close(x0, 0.0) && close(y0, 0.0), "source origin → dst origin, got ({x0},{y0})");
    assert!(close(x1, 30.0) && close(y1, 20.0), "source far corner → dst far corner, got ({x1},{y1})");
    // Snapping the origin to source pixel 0 would put it at x = 0 with a scale
    // of 30/2 — the left 7.5 canvas units would show a half-pixel too much.
    assert!(close(m.0, 20.0) && close(m.2, -10.0), "scale 30/1.5, translate -0.5·20: {m:?}");
}

/// A destination offset is carried through (Contain letterbox, a rect not at
/// the canvas origin), and the axes scale independently (Fill).
#[test]
fn source_to_dest_handles_offset_and_independent_axes() {
    let m = source_to_dest((10.0, 0.0, 20.0, 40.0), (5.0, 7.0, 40.0, 20.0)).unwrap();
    assert_eq!(map(m, (10.0, 0.0)), (5.0, 7.0));
    assert_eq!(map(m, (30.0, 40.0)), (45.0, 27.0));
    assert_eq!((m.0, m.1), (2.0, 0.5));
}

#[test]
fn source_to_dest_rejects_an_empty_source() {
    assert_eq!(source_to_dest((0.0, 0.0, 0.0, 10.0), (0.0, 0.0, 10.0, 10.0)), None);
    assert_eq!(source_to_dest((0.0, 0.0, 10.0, -1.0), (0.0, 0.0, 10.0, 10.0)), None);
}

/// Regression: the Android border frame kept its full alpha while the picture
/// faded with the layer opacity; web and vello fade both.
#[test]
fn regression_android_border_fades_with_layer_opacity() {
    // Opaque white border at opacity 0.5 → alpha 128, color untouched.
    assert_eq!(border_argb(255, 255, 255, 255, 0.5) as u32, 0x80_FF_FF_FF);
    // The border's own alpha and the opacity multiply.
    assert_eq!(border_argb(0x11, 0x22, 0x33, 128, 0.5) as u32, 0x40_11_22_33);
    // Full opacity is the border color as is (high bit set → negative i32,
    // which is what `Paint.setColor(int)` expects for an opaque color).
    assert_eq!(border_argb(0x11, 0x22, 0x33, 255, 1.0) as u32, 0xFF_11_22_33);
    // Opacity is clamped.
    assert_eq!(border_argb(1, 2, 3, 255, 2.0) as u32, 0xFF_01_02_03);
    assert_eq!(border_argb(1, 2, 3, 255, -1.0) as u32, 0x00_01_02_03);
}
