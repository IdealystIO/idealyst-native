//! Logical ↔ device pixel policy for per-monitor DPI.
//!
//! ## Units
//!
//! Everything the framework sees from this backend is in **logical
//! pixels** — device-independent pixels at 96 DPI, the Win32 name for
//! the same unit as Apple points, Android dp, GTK application pixels and
//! CSS px. Layout (Taffy), node frames, text measurement, font sizes,
//! the viewport signal, hit-testing, pointer positions and `on_layout`
//! rects never change with the monitor. **Device pixels** exist only at
//! the Win32/GDI+ boundary:
//!
//! | boundary                         | direction        |
//! |----------------------------------|------------------|
//! | `GetClientRect` → layout root    | device → logical |
//! | GDI+ scene paint                 | one root world-transform scale |
//! | child control `DeferWindowPos` / window regions | logical → device (edge-snapped) |
//! | child control `WM_SETFONT` font  | logical size → device height |
//! | DirectComposition visual offsets / clips | logical → device |
//! | graphics `OnReadyEvent.size`     | logical → physical, `scale` reported |
//! | mouse `lParam` → hit test / pointer sink | device → logical |
//! | SVG raster density               | drawn size × scale |
//!
//! That is the same split every other backend has (UIKit/AppKit points
//! with `contentsScale`, Android dp with `density`, GTK with its scale
//! factor): one author tree, the same layout on every monitor, sharper
//! pixels on denser ones (CLAUDE.md §7).
//!
//! ## Why the process is per-monitor-v2 aware
//!
//! A DPI-UNAWARE process is bitmap-stretched by the DWM on any monitor
//! above 96 DPI — text and SVGs blur, and the window paints at 1×.
//! System-aware fixes one DPI at startup and blurs again when the window
//! moves to a monitor with a different scale. Per-monitor-v2 makes the
//! process responsible for scaling (this module) and delivers
//! `WM_DPICHANGED` when the window crosses monitors, with a suggested
//! window rect that keeps the window's logical size; the host applies
//! it, the backend re-scales ([`DpiScale`]), relayouts and repaints.
//!
//! This module is pure (no `windows` types) so it compiles and is tested
//! on every host — `tests/dpi.rs` includes it by `#[path]`.

/// The DPI at which one logical pixel is one device pixel (`USER_DEFAULT_SCREEN_DPI`).
pub const BASE_DPI: u32 = 96;

/// One window's current DPI and the conversions it implies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DpiScale {
    dpi: u32,
}

impl Default for DpiScale {
    fn default() -> Self {
        DpiScale { dpi: BASE_DPI }
    }
}

impl DpiScale {
    /// The scale for `dpi`. `0` — what `GetDpiForWindow` returns for an
    /// invalid window — means 96.
    pub fn from_dpi(dpi: u32) -> Self {
        DpiScale { dpi: if dpi == 0 { BASE_DPI } else { dpi } }
    }

    pub fn dpi(self) -> u32 {
        self.dpi
    }

    /// Device pixels per logical pixel (1.0 at 96 DPI, 1.5 at 144, 2.0 at 192).
    pub fn factor(self) -> f32 {
        self.dpi as f32 / BASE_DPI as f32
    }

    pub fn to_logical(self, device: f32) -> f32 {
        device / self.factor()
    }

    pub fn to_device(self, logical: f32) -> f32 {
        logical * self.factor()
    }

    /// A device-pixel point (mouse `lParam`, client coordinates) in
    /// logical pixels.
    pub fn point_to_logical(self, x: f32, y: f32) -> (f32, f32) {
        (self.to_logical(x), self.to_logical(y))
    }

    /// The client rect's device size as the logical viewport layout
    /// runs against.
    pub fn client_to_logical(self, device_w: i32, device_h: i32) -> (f32, f32) {
        (self.to_logical(device_w.max(0) as f32), self.to_logical(device_h.max(0) as f32))
    }

    /// The device client size a window needs to show `logical` (the
    /// host's initial `RunOptions` size).
    pub fn logical_size_to_device(self, w: f32, h: f32) -> (i32, i32) {
        (self.to_device(w).round() as i32, self.to_device(h).round() as i32)
    }

    /// A logical rect `(x, y, w, h)` as a device rect, snapping EDGES
    /// rather than origin and size independently: `x1 = round((x + w) ·
    /// s)`, `w = x1 − x0`. Two logical rects that share an edge share
    /// it in device pixels too — independent rounding at 1.5× opens a
    /// one-pixel seam (or overlap) between adjacent native controls.
    pub fn rect_to_device(self, r: (f32, f32, f32, f32)) -> (i32, i32, i32, i32) {
        let s = self.factor();
        let x0 = (r.0 * s).round() as i32;
        let y0 = (r.1 * s).round() as i32;
        let x1 = ((r.0 + r.2) * s).round() as i32;
        let y1 = ((r.1 + r.3) * s).round() as i32;
        (x0, y0, (x1 - x0).max(0), (y1 - y0).max(0))
    }

    /// A logical rect scaled to device space without rounding (for
    /// geometry that is rounded later, e.g. window regions and
    /// composition clips).
    pub fn rect_to_device_f(self, r: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
        let s = self.factor();
        (r.0 * s, r.1 * s, r.2 * s, r.3 * s)
    }

    /// Device `lfHeight` magnitude for a font whose em size is
    /// `logical_px` (native controls draw their own text, in device
    /// pixels). Never below 1 — a zero `lfHeight` asks GDI for its
    /// default size instead.
    pub fn font_px_to_device(self, logical_px: i32) -> i32 {
        ((logical_px as f32 * self.factor()).round() as i32).max(1)
    }

    /// The physical pixel size of a logically-sized surface (graphics
    /// swapchains): what `OnReadyEvent::size` reports.
    pub fn physical_size(self, logical_w: f32, logical_h: f32) -> (u32, u32) {
        (
            self.to_device(logical_w).round().max(0.0) as u32,
            self.to_device(logical_h).round().max(0.0) as u32,
        )
    }

    /// The `screen_scale` an SVG rasterizes at: rasters are made for the
    /// DEVICE pixels the drawn box covers, so a 32×16 SVG drawn at 32×16
    /// logical px on a 2× monitor is rasterized at 64×32 — never
    /// stretched up from a 1× bitmap.
    pub fn svg_screen_scale(self) -> f64 {
        self.factor() as f64
    }
}

/// A metric the system reported at `reported_dpi` (e.g. the message
/// font height from `SPI_GETNONCLIENTMETRICS` in a DPI-aware process,
/// which is pre-scaled for the system DPI) back in logical pixels.
/// Rounded, never below 1.
pub fn metric_to_logical(value: i32, reported_dpi: u32) -> i32 {
    let s = DpiScale::from_dpi(reported_dpi);
    ((value as f32 / s.factor()).round() as i32).max(1)
}

/// `WM_DPICHANGED`: the new DPI from `wParam`. LOWORD is the X-axis DPI,
/// HIWORD the Y-axis; Windows documents them as always equal and apps
/// use X.
pub fn dpichanged_dpi(wparam: usize) -> u32 {
    (wparam & 0xFFFF) as u32
}

/// `WM_DPICHANGED`: the suggested window rect (`lParam` → `RECT`,
/// `(left, top, right, bottom)` in screen coordinates) as the
/// `(x, y, width, height)` `SetWindowPos` takes. Applying it exactly is
/// what keeps the window's LOGICAL size (and the cursor over the same
/// spot of the title bar) when it crosses to a monitor with a different
/// scale — a window that ignores it shows the same device size, i.e.
/// half the logical content at 2×.
pub fn suggested_window_pos(rect: (i32, i32, i32, i32)) -> (i32, i32, i32, i32) {
    let (l, t, r, b) = rect;
    (l, t, (r - l).max(0), (b - t).max(0))
}
