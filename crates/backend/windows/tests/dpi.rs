//! The Win32 backend's logical ↔ device pixel rules (`src/dpi.rs`).
//!
//! ## The bug this pins
//!
//! The backend had no DPI handling at all: the process was DPI-unaware,
//! layout ran against the device client rect, and SVGs rasterized at
//! scale 1 — on a 150%/200% monitor Windows bitmap-stretched the whole
//! window (blurry text and images), or, once aware, the UI shrank to half
//! size. These are the conversions the per-monitor-v2 support rests on.
//!
//! The crate body is `cfg(target_os = "windows")`; this test compiles the
//! pure policy module directly (`#[path]`) so it runs on every host. The
//! live window/GDI half is `dpi_tests` in `src/lib.rs` (Windows / Wine).

#[path = "../src/dpi.rs"]
mod dpi;

use backend_image_source::SvgImage;
use dpi::*;

#[test]
fn factor_follows_dpi_and_zero_means_96() {
    assert_eq!(DpiScale::from_dpi(96).factor(), 1.0);
    assert_eq!(DpiScale::from_dpi(120).factor(), 1.25);
    assert_eq!(DpiScale::from_dpi(144).factor(), 1.5);
    assert_eq!(DpiScale::from_dpi(192).factor(), 2.0);
    // `GetDpiForWindow` answers 0 for an invalid window.
    assert_eq!(DpiScale::from_dpi(0), DpiScale::default());
    assert_eq!(DpiScale::default().dpi(), BASE_DPI);
}

#[test]
fn regression_layout_units_scale_with_dpi() {
    let s = DpiScale::from_dpi(192);
    // The 800×600 device client is a 400×300 logical viewport…
    assert_eq!(s.client_to_logical(800, 600), (400.0, 300.0));
    // …a 100×50 logical view covers 200×100 device px…
    assert_eq!(s.rect_to_device((0.0, 0.0, 100.0, 50.0)), (0, 0, 200, 100));
    assert_eq!(s.rect_to_device((10.0, 50.0, 100.0, 50.0)), (20, 100, 200, 100));
    // …a click at device (150, 75) is logical (75, 37.5)…
    assert_eq!(s.point_to_logical(150.0, 75.0), (75.0, 37.5));
    // …and a 12 px font is a 24 px device font for native controls.
    assert_eq!(s.font_px_to_device(12), 24);
    // At 96 DPI everything is the identity.
    let one = DpiScale::from_dpi(96);
    assert_eq!(one.client_to_logical(800, 600), (800.0, 600.0));
    assert_eq!(one.rect_to_device((3.0, 4.0, 5.0, 6.0)), (3, 4, 5, 6));
    assert_eq!(one.font_px_to_device(12), 12);
}

#[test]
fn round_trip_logical_device() {
    for dpi in [96, 120, 144, 168, 192, 288] {
        let s = DpiScale::from_dpi(dpi);
        for v in [0.0_f32, 1.0, 13.5, 100.0, 777.25] {
            assert!((s.to_logical(s.to_device(v)) - v).abs() < 1e-4, "{dpi} {v}");
        }
    }
}

/// At 1.5× adjacent logical rects must share their device edge — rounding
/// origin and size independently opens a 1 px seam (or overlap) between
/// stacked native controls.
#[test]
fn fractional_scale_snaps_edges_not_sizes() {
    let s = DpiScale::from_dpi(144);
    let a = s.rect_to_device((0.0, 0.0, 33.0, 21.0));
    let b = s.rect_to_device((0.0, 21.0, 33.0, 21.0));
    let c = s.rect_to_device((0.0, 42.0, 33.0, 21.0));
    assert_eq!(a.1 + a.3, b.1);
    assert_eq!(b.1 + b.3, c.1);
    // 21 × 1.5 = 31.5: the rows alternate 32/31 device px, totaling 95.
    assert_eq!(c.1 + c.3, 95);
    assert_eq!(a.2, 50, "33 × 1.5 = 49.5 → 50");
}

#[test]
fn physical_surface_size_and_initial_window() {
    let s = DpiScale::from_dpi(144);
    assert_eq!(s.physical_size(400.0, 300.0), (600, 450));
    assert_eq!(s.logical_size_to_device(1024.0, 768.0), (1536, 1152));
    assert_eq!(DpiScale::from_dpi(192).font_px_to_device(0), 1, "never a zero lfHeight");
}

/// `SPI_GETNONCLIENTMETRICS` in an aware process reports the message font
/// pre-scaled for the system DPI; the backend wants it at 96.
#[test]
fn system_metric_normalizes_to_logical() {
    assert_eq!(metric_to_logical(24, 192), 12);
    assert_eq!(metric_to_logical(18, 144), 12);
    assert_eq!(metric_to_logical(12, 96), 12);
    assert_eq!(metric_to_logical(12, 0), 12, "0 = unknown = 96");
}

#[test]
fn wm_dpichanged_decoding() {
    // wParam: HIWORD = Y dpi, LOWORD = X dpi.
    assert_eq!(dpichanged_dpi((144 << 16) | 144), 144);
    assert_eq!(dpichanged_dpi((192 << 16) | 192), 192);
    // lParam RECT (l, t, r, b) → SetWindowPos (x, y, w, h).
    assert_eq!(suggested_window_pos((100, 50, 1700, 1250)), (100, 50, 1600, 1200));
    // Degenerate rects never go negative.
    assert_eq!(suggested_window_pos((10, 10, 5, 5)), (10, 10, 0, 0));
}

/// The SVG raster density the backend asks for at each scale — fed
/// straight into the shared `raster_scale` rule. Drawn at its intrinsic
/// size, a 32×16 SVG on a 2× monitor rasterizes at 64×32; drawn 4× larger,
/// at 8× intrinsic.
#[test]
fn regression_svg_rasterizes_at_monitor_scale() {
    let svg = SvgImage::parse(
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="16"><rect width="32" height="16"/></svg>"#,
    )
    .expect("svg");
    let two = DpiScale::from_dpi(192);
    let at_size = svg.raster_scale((32.0, 16.0), two.svg_screen_scale());
    assert_eq!(at_size, 2.0);
    let raster = svg.rasterize(at_size).expect("raster");
    assert_eq!((raster.width_px, raster.height_px), (64, 32));
    assert_eq!(svg.raster_scale((128.0, 64.0), two.svg_screen_scale()), 8.0);
    assert_eq!(svg.raster_scale((32.0, 16.0), DpiScale::from_dpi(144).svg_screen_scale()), 1.5);
    assert_eq!(svg.raster_scale((32.0, 16.0), DpiScale::default().svg_screen_scale()), 1.0);
}
