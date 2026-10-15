//! Linear gradients rasterized on the CPU, straight onto the device-pixel
//! grid.
//!
//! ## Why not the GDI+ line brush
//!
//! A gradient brush makes GDI+ shade every covered pixel through its general
//! brush path. Under Wine that path costs ~24 ns per device pixel: the welcome
//! scene's four full-height vignette bands took ~21 ms EACH at 2× DPI, ~85 of
//! a ~90 ms frame (measured 2026-10-15, Wine 11.18). Every other GDI+ route
//! that samples per pixel was no better — a texture brush over a 1-px ramp
//! cost 48 ms, a stretched `DrawImage` of that ramp 17–39 ms. An UNscaled
//! `DrawImage` of a ready bitmap took ~2.9 ms, and filling that bitmap here
//! took 0.06 ms (axis-aligned). So the painter computes the pixels itself and
//! hands GDI+ a 1:1 copy.
//!
//! ## What it reproduces
//!
//! Exactly the brush path's output, so the switch is invisible:
//!
//! - **Color ramp**: GDI+'s preset blend — straight (non-premultiplied)
//!   8-bit ARGB interpolated linearly between the padded stops
//!   (`scene::blend_arrays`, passed in unchanged).
//! - **Shape**: the node's rounded-rect fill path (radii clamped to half the
//!   box, as `scene::build_round_rect_path` does), with anti-aliased edges —
//!   fractional box edges get their exact area coverage, corners an analytic
//!   distance-based coverage.
//!
//! It only applies when the world transform is axis-aligned (scale +
//! translate, positive scale): then a logical box maps to a device rectangle
//! and "1:1 onto the device grid" is well-defined. A rotated or mirrored node
//! keeps the brush path ([`axis_aligned`] returns `None`).
//!
//! Pure: no Win32, so it is tested on every host (`tests/gradient_raster.rs`).

/// Logical → device mapping of an axis-aligned world transform:
/// `device = logical * scale + offset`, per axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DeviceMap {
    pub sx: f32,
    pub sy: f32,
    pub dx: f32,
    pub dy: f32,
}

/// A device-pixel rectangle, `(x, y, width, height)`.
pub(crate) type DeviceRect = (i32, i32, i32, i32);

/// Premultiplied ARGB (`PixelFormat32bppPARGB`) pixels, row-major with a
/// stride of `width * 4` bytes, to be drawn 1:1 at device `(x, y)`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Raster {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub pixels: Vec<u32>,
}

/// The mapping for a GDI+ world matrix (`[m11, m12, m21, m22, dx, dy]`, the
/// `GdipGetMatrixElements` order), or `None` when it rotates, skews or
/// mirrors — the cases a device-aligned raster can't express.
pub(crate) fn axis_aligned(m: [f32; 6]) -> Option<DeviceMap> {
    const EPS: f32 = 1e-6;
    let [m11, m12, m21, m22, dx, dy] = m;
    if m12.abs() > EPS || m21.abs() > EPS || m11 <= EPS || m22 <= EPS {
        return None;
    }
    Some(DeviceMap { sx: m11, sy: m22, dx, dy })
}

/// Number of entries in the color lookup table. Adjacent entries differ
/// by at most 255/1023 of a channel step, well under one 8-bit level.
const LUT_LEN: usize = 1024;

/// Rasterize a linear gradient over the logical `w × h` box.
///
/// - `start` / `end`: the gradient axis in logical box coordinates
///   (`crate::linear_points`).
/// - `colors` / `positions`: the padded preset-blend arrays
///   (`scene::blend_arrays`) — ascending positions from 0.0 to 1.0, straight
///   `0xAARRGGBB` colors with the node's opacity already multiplied in.
/// - `radii`: per-corner `[tl, tr, br, bl]`, logical px.
/// - `visible`: the device rectangle that can show anything (the graphics'
///   visible clip bounds); pixels outside it are not computed.
///
/// `None` when nothing of the box is visible.
#[allow(clippy::too_many_arguments)]
pub(crate) fn linear(
    start: (f32, f32),
    end: (f32, f32),
    colors: &[u32],
    positions: &[f32],
    w: f32,
    h: f32,
    radii: [f32; 4],
    map: DeviceMap,
    visible: DeviceRect,
) -> Option<Raster> {
    if w <= 0.0 || h <= 0.0 || colors.is_empty() || colors.len() != positions.len() {
        return None;
    }
    // The box on the device grid.
    let bx0 = map.dx;
    let by0 = map.dy;
    let bx1 = map.dx + w * map.sx;
    let by1 = map.dy + h * map.sy;

    let (vx, vy, vw, vh) = visible;
    let x0 = (bx0.floor() as i32).max(vx);
    let y0 = (by0.floor() as i32).max(vy);
    let x1 = (bx1.ceil() as i32).min(vx.saturating_add(vw));
    let y1 = (by1.ceil() as i32).min(vy.saturating_add(vh));
    if x1 <= x0 || y1 <= y0 {
        return None;
    }

    let lut = build_lut(colors, positions);

    // t is affine in the device pixel coordinates: t = tx·px + ty·py + t0,
    // evaluated at pixel centres. In logical space t = ((p − start)·d)/|d|²
    // with p = ((px + .5 − dx)/sx, (py + .5 − dy)/sy).
    let (ddx, ddy) = (end.0 - start.0, end.1 - start.1);
    let len2 = ddx * ddx + ddy * ddy;
    let (tx, ty, t0) = if len2 > 0.0 {
        let tx = ddx / (len2 * map.sx);
        let ty = ddy / (len2 * map.sy);
        let t0 = ((0.5 - map.dx) / map.sx - start.0) * ddx / len2
            + ((0.5 - map.dy) / map.sy - start.1) * ddy / len2;
        (tx, ty, t0)
    } else {
        (0.0, 0.0, 0.0)
    };

    let corners = Corners::new(radii, w, h, map, (bx0, by0, bx1, by1));
    let (corner_top, corner_bottom) = corners.row_bands();

    let width = (x1 - x0) as usize;
    let height = (y1 - y0) as usize;
    let mut pixels = vec![0u32; width * height];

    // Coverage is separable away from the corners: (column overlap) ×
    // (row overlap), each 1 except on the box's edge pixels.
    let col_cov: Vec<f32> = (x0..x1).map(|px| overlap(px as f32, bx0, bx1)).collect();
    let color_at = |t: f32| lut[(t.clamp(0.0, 1.0) * (LUT_LEN - 1) as f32 + 0.5) as usize];

    // The per-pixel path: corner rows, and every row of an oblique axis.
    let shade_row = |out: &mut [u32], py: i32, cov_y: f32| {
        let fy = py as f32 + 0.5;
        let row_t = ty * py as f32 + t0;
        for (col, px) in (x0..x1).enumerate() {
            let cov = col_cov[col] * cov_y;
            if cov <= 0.0 {
                continue;
            }
            let cov = cov * corners.coverage(px as f32 + 0.5, fy);
            if cov > 0.0 {
                out[col] = premultiply(color_at(tx * px as f32 + row_t), cov);
            }
        }
    };

    // An axis-aligned gradient (the common case — and the snapped CSS axis
    // angles are exactly that) varies along one axis only, so each
    // straight row is a copy or a fill rather than per-pixel shading.
    // This is the difference between ~20 ms and well under 1 ms for a
    // full-height band at 2× under Wine (x86 under Rosetta).
    let along_x = ty == 0.0;
    let along_y = tx == 0.0 && !along_x;
    // `along_x`: a full-coverage row is the same for every row.
    let template: Vec<u32> = if along_x {
        (x0..x1)
            .enumerate()
            .map(|(col, px)| {
                let c = col_cov[col];
                if c > 0.0 { premultiply(color_at(tx * px as f32 + t0), c) } else { 0 }
            })
            .collect()
    } else {
        Vec::new()
    };

    for (row, py) in (y0..y1).enumerate() {
        let cov_y = overlap(py as f32, by0, by1);
        if cov_y <= 0.0 {
            continue;
        }
        let out = &mut pixels[row * width..(row + 1) * width];
        let in_corner_band = (py as f32) < corner_top || (py as f32 + 1.0) > corner_bottom;
        if in_corner_band || !(along_x || along_y) {
            shade_row(out, py, cov_y);
        } else if along_x {
            if cov_y >= 1.0 {
                out.copy_from_slice(&template);
            } else {
                for (col, px) in (x0..x1).enumerate() {
                    let cov = col_cov[col] * cov_y;
                    if cov > 0.0 {
                        out[col] = premultiply(color_at(tx * px as f32 + t0), cov);
                    }
                }
            }
        } else {
            // along_y: one color per row.
            let c = color_at(ty * py as f32 + t0);
            let full = premultiply(c, cov_y);
            for (col, slot) in out.iter_mut().enumerate() {
                let cx = col_cov[col];
                *slot = if cx >= 1.0 {
                    full
                } else if cx > 0.0 {
                    premultiply(c, cx * cov_y)
                } else {
                    0
                };
            }
        }
    }
    Some(Raster { x: x0, y: y0, width: width as i32, height: height as i32, pixels })
}

/// Fraction of the unit pixel span `[p, p + 1]` inside `[lo, hi]`.
#[inline]
fn overlap(p: f32, lo: f32, hi: f32) -> f32 {
    ((p + 1.0).min(hi) - p.max(lo)).clamp(0.0, 1.0)
}

/// `[a, r, g, b]` in 0..=255 (straight) → PARGB with `coverage` applied.
/// Rounds by `+ 0.5` truncation (every value is non-negative): `f32::round`
/// is a libm call per channel, the hot spot of the per-pixel path.
#[inline]
fn premultiply(c: [f32; 4], coverage: f32) -> u32 {
    let a = c[0] * coverage;
    let k = a / 255.0;
    let ch = |v: f32| (v * k + 0.5) as u32;
    (((a + 0.5) as u32) << 24) | (ch(c[1]) << 16) | (ch(c[2]) << 8) | ch(c[3])
}

/// The preset blend sampled at `LUT_LEN` evenly spaced `t`, as straight
/// `[a, r, g, b]` floats.
fn build_lut(colors: &[u32], positions: &[f32]) -> Vec<[f32; 4]> {
    let unpack = |c: u32| {
        [
            (c >> 24) as f32,
            ((c >> 16) & 0xFF) as f32,
            ((c >> 8) & 0xFF) as f32,
            (c & 0xFF) as f32,
        ]
    };
    (0..LUT_LEN)
        .map(|i| {
            let t = i as f32 / (LUT_LEN - 1) as f32;
            sample(colors, positions, t, &unpack)
        })
        .collect()
}

fn sample(colors: &[u32], positions: &[f32], t: f32, unpack: &dyn Fn(u32) -> [f32; 4]) -> [f32; 4] {
    let last = colors.len() - 1;
    if last == 0 || t <= positions[0] {
        return unpack(colors[0]);
    }
    for i in 0..last {
        let (p0, p1) = (positions[i], positions[i + 1]);
        if t <= p1 {
            let (c0, c1) = (unpack(colors[i]), unpack(colors[i + 1]));
            // A hard stop (p0 == p1) takes the later color.
            let f = if p1 > p0 { (t - p0) / (p1 - p0) } else { 1.0 };
            return [0, 1, 2, 3].map(|k| c0[k] + (c1[k] - c0[k]) * f);
        }
    }
    unpack(colors[last])
}

/// The rounded corners of the box on the device grid. Radii clamp to half
/// the box's shorter side, as `scene::build_round_rect_path` does; under a
/// non-uniform scale a corner is an ellipse.
struct Corners {
    /// `(centre_x, centre_y, radius_x, radius_y)` per corner, or `None`
    /// for a square corner. Order `[tl, tr, br, bl]`.
    c: [Option<(f32, f32, f32, f32)>; 4],
}

impl Corners {
    fn new(radii: [f32; 4], w: f32, h: f32, map: DeviceMap, b: (f32, f32, f32, f32)) -> Self {
        let (x0, y0, x1, y1) = b;
        let rmax = w.min(h) / 2.0;
        let mut c = [None; 4];
        // Same "square" threshold as the path builder.
        if radii.iter().any(|r| *r >= 0.5) {
            for (i, slot) in c.iter_mut().enumerate() {
                let r = radii[i].clamp(0.0, rmax);
                // Square below half a pixel, per corner, as the path does.
                if r < 0.5 {
                    continue;
                }
                let (rx, ry) = (r * map.sx, r * map.sy);
                let (cx, cy) = match i {
                    0 => (x0 + rx, y0 + ry),
                    1 => (x1 - rx, y0 + ry),
                    2 => (x1 - rx, y1 - ry),
                    _ => (x0 + rx, y1 - ry),
                };
                *slot = Some((cx, cy, rx, ry));
            }
        }
        Corners { c }
    }

    /// Device-y bounds of the rows a corner can touch: rows above
    /// `.0` (the lowest top-corner centre) or below `.1` (the highest
    /// bottom-corner centre). Every other row has full corner coverage.
    fn row_bands(&self) -> (f32, f32) {
        let top = [self.c[0], self.c[1]]
            .iter()
            .flatten()
            .map(|c| c.1)
            .fold(f32::NEG_INFINITY, f32::max);
        let bottom = [self.c[2], self.c[3]]
            .iter()
            .flatten()
            .map(|c| c.1)
            .fold(f32::INFINITY, f32::min);
        (top, bottom)
    }

    /// Coverage in `[0, 1]` of the pixel centred at `(x, y)` by the
    /// rounded corners: 1 away from any corner, an anti-aliased falloff
    /// across a corner's arc.
    #[inline]
    fn coverage(&self, x: f32, y: f32) -> f32 {
        for (i, corner) in self.c.iter().enumerate() {
            let Some((cx, cy, rx, ry)) = *corner else { continue };
            let inside_quadrant = match i {
                0 => x < cx && y < cy,
                1 => x > cx && y < cy,
                2 => x > cx && y > cy,
                _ => x < cx && y > cy,
            };
            if !inside_quadrant {
                continue;
            }
            // Signed distance to the ellipse, first-order: (d − 1) / |∇d|
            // with d the normalized radius. Exact for a circle.
            let (nx, ny) = ((x - cx) / rx, (y - cy) / ry);
            let d = (nx * nx + ny * ny).sqrt();
            if d <= 0.0 {
                return 1.0;
            }
            let grad = ((nx / rx).powi(2) + (ny / ry).powi(2)).sqrt() / d;
            let sd = (d - 1.0) / grad.max(f32::EPSILON);
            return (0.5 - sd).clamp(0.0, 1.0);
        }
        1.0
    }
}
