//! The CPU linear-gradient rasterizer the scene painter uses in place of the
//! GDI+ line brush (see `src/gradient_raster.rs` for why).
//!
//! ## What this pins
//!
//! Geometry and color of the raster on its own: which transforms take the
//! fast path, the ramp's straight-alpha interpolation, area coverage at
//! fractional edges, rounded-corner coverage, and clipping to the visible
//! device rect (which keeps an off-screen or huge box from allocating a huge
//! bitmap). Agreement with the brush path it replaces is pinned on the
//! Windows target, pixel against pixel, by `scene::gradient_render_tests`
//! (run under Wine off Windows).
//!
//! The crate body is `cfg(target_os = "windows")`; this compiles the pure
//! module directly (`#[path]`) so it runs on every host.

#[path = "../src/gradient_raster.rs"]
#[allow(dead_code)]
mod gradient_raster;

use gradient_raster::*;

const IDENTITY: DeviceMap = DeviceMap { sx: 1.0, sy: 1.0, dx: 0.0, dy: 0.0 };
const SCREEN: DeviceRect = (0, 0, 4096, 4096);
const BLACK: u32 = 0xFF00_0000;
const WHITE: u32 = 0xFFFF_FFFF;

fn px(r: &Raster, x: i32, y: i32) -> u32 {
    r.pixels[((y - r.y) * r.width + (x - r.x)) as usize]
}
fn alpha(p: u32) -> u32 {
    p >> 24
}
fn red(p: u32) -> u32 {
    (p >> 16) & 0xFF
}

/// Left → right black → white over a `w × h` box (the axis
/// `linear_points(90°, w, h)` produces).
fn horizontal(w: f32, h: f32, radii: [f32; 4], map: DeviceMap, visible: DeviceRect) -> Option<Raster> {
    linear((0.0, h / 2.0), (w, h / 2.0), &[BLACK, WHITE], &[0.0, 1.0], w, h, radii, map, visible)
}

#[test]
fn axis_aligned_transforms_take_the_fast_path_and_others_do_not() {
    assert_eq!(
        axis_aligned([2.0, 0.0, 0.0, 2.0, 10.0, 20.0]),
        Some(DeviceMap { sx: 2.0, sy: 2.0, dx: 10.0, dy: 20.0 })
    );
    // 30° rotation, a skew, and a mirror can't be a device-aligned raster.
    assert_eq!(axis_aligned([0.866, 0.5, -0.5, 0.866, 0.0, 0.0]), None);
    assert_eq!(axis_aligned([1.0, 0.0, 0.3, 1.0, 0.0, 0.0]), None);
    assert_eq!(axis_aligned([-1.0, 0.0, 0.0, 1.0, 0.0, 0.0]), None);
}

#[test]
fn ramp_is_sampled_at_pixel_centres_along_the_axis() {
    let r = horizontal(100.0, 4.0, [0.0; 4], IDENTITY, SCREEN).unwrap();
    assert_eq!((r.x, r.y, r.width, r.height), (0, 0, 100, 4));
    // Pixel x samples t = (x + 0.5) / 100.
    for x in [0, 25, 50, 99] {
        let expect = ((x as f32 + 0.5) / 100.0 * 255.0).round() as i32;
        let got = red(px(&r, x, 2)) as i32;
        assert!((got - expect).abs() <= 1, "x={x}: {got} vs {expect}");
        assert_eq!(alpha(px(&r, x, 2)), 255);
    }
    // Constant across the perpendicular axis.
    assert_eq!(px(&r, 40, 0), px(&r, 40, 3));
}

#[test]
fn ramp_interpolates_straight_alpha_like_the_gdiplus_preset_blend() {
    // Transparent black → opaque red. Straight interpolation blends the
    // color channels independently of alpha, so at t = 0.75 the straight
    // pixel is (a 191, r 191) → PARGB red 191·191/255 = 143. A premultiplied
    // ramp (CSS) would give PARGB red 191 there.
    let r = linear(
        (0.0, 0.5),
        (2.0, 0.5),
        &[0x0000_0000, 0xFFFF_0000],
        &[0.0, 1.0],
        2.0,
        1.0,
        [0.0; 4],
        DeviceMap { sx: 1.0, sy: 1.0, dx: 0.0, dy: 0.0 },
        SCREEN,
    )
    .unwrap();
    // Two pixels: t = 0.25 and 0.75.
    let p = px(&r, 1, 0);
    let (a, rr) = (alpha(p) as f32, red(p) as f32);
    assert!((a - 191.0).abs() <= 1.0, "alpha {a}");
    // straight red = 191, premultiplied by 191/255 → 143.
    assert!((rr - 143.0).abs() <= 1.0, "red {rr}");
}

#[test]
fn a_hard_stop_switches_color_without_a_blend() {
    let r = linear(
        (0.0, 0.5),
        (10.0, 0.5),
        &[BLACK, BLACK, WHITE, WHITE],
        &[0.0, 0.5, 0.5, 1.0],
        10.0,
        1.0,
        [0.0; 4],
        IDENTITY,
        SCREEN,
    )
    .unwrap();
    assert_eq!(red(px(&r, 4, 0)), 0);
    assert_eq!(red(px(&r, 5, 0)), 255);
}

#[test]
fn fractional_box_edges_get_area_coverage() {
    // Box at device x ∈ [0.5, 10.5): first and last pixels half covered.
    let map = DeviceMap { sx: 1.0, sy: 1.0, dx: 0.5, dy: 0.0 };
    let r = linear((0.0, 0.5), (10.0, 0.5), &[WHITE, WHITE], &[0.0, 1.0], 10.0, 2.0, [0.0; 4], map, SCREEN)
        .unwrap();
    assert_eq!((r.x, r.width), (0, 11));
    assert_eq!(alpha(px(&r, 0, 0)), 128);
    assert_eq!(alpha(px(&r, 5, 0)), 255);
    assert_eq!(alpha(px(&r, 10, 0)), 128);
}

#[test]
fn device_scale_maps_the_logical_box_onto_more_pixels() {
    // The welcome vignette's shape: 287 × 768 logical at 2× DPI.
    let map = DeviceMap { sx: 2.0, sy: 2.0, dx: 0.0, dy: 0.0 };
    let r = horizontal(287.0, 768.0, [0.0; 4], map, SCREEN).unwrap();
    assert_eq!((r.width, r.height), (574, 1536));
    // Device pixel 287 is the logical midpoint.
    let mid = red(px(&r, 287, 100)) as i32;
    assert!((mid - 128).abs() <= 1, "{mid}");
}

#[test]
fn rounded_corners_are_cut_with_antialiased_coverage() {
    let (w, h, rad) = (40.0, 40.0, 10.0);
    let r = linear((0.0, 20.0), (40.0, 20.0), &[WHITE, WHITE], &[0.0, 1.0], w, h, [rad; 4], IDENTITY, SCREEN)
        .unwrap();
    // Outside the arc: empty. Centre and edge midpoints: solid.
    for (x, y) in [(0, 0), (39, 0), (39, 39), (0, 39)] {
        assert_eq!(alpha(px(&r, x, y)), 0, "corner ({x}, {y})");
    }
    for (x, y) in [(20, 20), (20, 0), (0, 20), (39, 20), (20, 39)] {
        assert_eq!(alpha(px(&r, x, y)), 255, "({x}, {y})");
    }
    // Total coverage ≈ the rounded rect's area, w·h − (4 − π)·r².
    let covered: f32 = r.pixels.iter().map(|p| alpha(*p) as f32 / 255.0).sum();
    let area = w * h - (4.0 - std::f32::consts::PI) * rad * rad;
    assert!((covered - area).abs() / area < 0.005, "{covered} vs {area}");
}

#[test]
fn radii_clamp_to_half_the_shorter_side() {
    // A 999px radius on a 40 × 20 box is a capsule: radius 10.
    let r = linear((0.0, 10.0), (40.0, 10.0), &[WHITE, WHITE], &[0.0, 1.0], 40.0, 20.0, [999.0; 4], IDENTITY, SCREEN)
        .unwrap();
    let covered: f32 = r.pixels.iter().map(|p| alpha(*p) as f32 / 255.0).sum();
    let area = 20.0 * 20.0 + std::f32::consts::PI * 100.0;
    assert!((covered - area).abs() / area < 0.005, "{covered} vs {area}");
}

#[test]
fn only_the_visible_device_rect_is_rasterized() {
    // Half the box hangs off the left of a 100 × 100 surface: only the
    // visible half is allocated, and fully off-screen draws nothing.
    let map = DeviceMap { sx: 1.0, sy: 1.0, dx: -50.0, dy: 0.0 };
    let r = horizontal(100.0, 10.0, [0.0; 4], map, (0, 0, 100, 100)).unwrap();
    assert_eq!((r.x, r.width), (0, 50));
    // Clipping doesn't shift the ramp: device 0 is logical 50.
    let mid = red(px(&r, 0, 5)) as i32;
    assert!((mid - 129).abs() <= 1, "{mid}");
    let off = DeviceMap { sx: 1.0, sy: 1.0, dx: 500.0, dy: 0.0 };
    assert!(horizontal(100.0, 10.0, [0.0; 4], off, (0, 0, 100, 100)).is_none());
}

#[test]
fn vertical_ramp_fills_rows_and_keeps_fractional_edges_on_both_axes() {
    // Top → bottom (the axis-along-y fill path), box at (0.5, 0.25).
    let map = DeviceMap { sx: 1.0, sy: 1.0, dx: 0.5, dy: 0.25 };
    let r = linear((5.0, 0.0), (5.0, 100.0), &[BLACK, WHITE], &[0.0, 1.0], 10.0, 100.0, [0.0; 4], map, SCREEN)
        .unwrap();
    // A row is one color across its fully covered pixels.
    assert_eq!(px(&r, 3, 50), px(&r, 8, 50));
    // Pixel row 50 samples logical y = 50.5 − 0.25 → t ≈ 0.5025.
    let mid = red(px(&r, 5, 50)) as i32;
    assert!((mid - 128).abs() <= 1, "{mid}");
    // Half-covered columns at both sides, ¾-covered first row.
    assert_eq!(alpha(px(&r, 0, 50)), 128);
    assert_eq!(alpha(px(&r, 10, 50)), 128);
    assert_eq!(alpha(px(&r, 5, 0)), 191);
    // The corner pixel combines both: ½ × ¾.
    assert_eq!(alpha(px(&r, 0, 0)), 96);
}
