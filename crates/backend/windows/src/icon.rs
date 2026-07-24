//! `Element::Icon` — vector icon rendering on the Win32 backend.
//!
//! The framework's icon data is a set of SVG path `d` strings in a
//! view-box coordinate space (Lucide-style, 24×24). The scene painter
//! (`crate::scene`) calls [`paint_into`] to parse the paths into a GDI+
//! `GpPath` and stroke or fill it into the node's box — the same approach
//! as the macOS `CAShapeLayer` and Linux GSK backends. Mechanism
//! diverges, output converges (repo rule §7).
//!
//! ## Parser provenance
//!
//! [`parse_svg_path`] + [`arc_to_bezier`] are ported **verbatim** from the
//! Linux backend's `icon.rs` (itself ported from the Apple backend). Each
//! backend keeps its own copy rather than sharing a crate — the Apple
//! original pulls in `objc2`, the Linux one pulls in `gtk4`, neither of
//! which belongs in the Windows dep graph. The arc→cubic kappa derivation
//! `(4/3)·tan(θ/4)` and the smooth-curve reflection are identical across
//! all three, pinned by the shared regression tests below, so a given
//! `IconData` produces a visually identical glyph on every backend.
//!
//! ## Stroke vs fill
//!
//! Matches every other backend: an *outlined* icon (`filled == false`, the
//! Lucide default) is STROKED with the icon color, round caps + joins,
//! line width `2 × scale` (Lucide authors at stroke-width 2 in a 24-unit
//! box); a *filled* icon is FILLED honoring its `fill_rule`, no stroke.

use runtime_core::color::Rgba;
use runtime_core::primitives::icon::{FillRule, IconData};

use windows::Win32::Graphics::GdiPlus::{
    GdipAddPathBezier, GdipAddPathLine, GdipClosePathFigure, GdipCreatePath, GdipCreatePen1,
    GdipCreateSolidFill, GdipDeleteBrush, GdipDeletePath, GdipDeletePen, GdipDrawPath,
    GdipFillPath, GdipSetPenEndCap, GdipSetPenLineJoin, GdipSetPenStartCap, GdipStartPathFigure,
    FillModeAlternate, FillModeWinding, GpBrush, GpGraphics, GpPath, GpPen, GpSolidFill,
    LineCapRound, LineJoinRound, UnitPixel,
};

// =========================================================================
// SVG path parser — ported VERBATIM from crates/backend/linux/src/icon.rs
// (`parse_svg_path`, `emit_quad`, `arc_to_bezier`, `skip_ws_comma`,
// `parse_number`). Do not "improve" locally — keep byte-for-byte with the
// other backends so glyphs converge. The only change is the emitter trait
// lives here too (the Linux one is not importable).
// =========================================================================

/// Sink for the SVG path parser. Each method is a path command in
/// *destination* (already scaled + translated) coordinates.
pub trait PathEmitter {
    fn move_to(&mut self, x: f64, y: f64);
    fn line_to(&mut self, x: f64, y: f64);
    fn curve_to(&mut self, c1x: f64, c1y: f64, c2x: f64, c2y: f64, x: f64, y: f64);
    fn close(&mut self);
}

/// Parse an SVG path `d` string and emit its commands to `emitter`,
/// scaled by `(sx, sy)` and offset by `(ox, oy)`. Supports the full Lucide
/// command set: `M/m L/l H/h V/v C/c S/s Q/q T/t A/a Z`. Quadratics are
/// lifted to cubics; arcs are approximated by cubic segments.
pub fn parse_svg_path(
    d: &str,
    sx: f64,
    sy: f64,
    ox: f64,
    oy: f64,
    emitter: &mut dyn PathEmitter,
) {
    let mx = |x: f64| x * sx + ox;
    let my = |y: f64| y * sy + oy;

    let mut cur_x: f64 = 0.0;
    let mut cur_y: f64 = 0.0;
    let mut start_x: f64 = 0.0;
    let mut start_y: f64 = 0.0;
    let mut last_ctrl_x: f64 = 0.0;
    let mut last_ctrl_y: f64 = 0.0;
    let mut last_cmd: char = ' ';

    let mut chars = d.chars().peekable();

    while chars.peek().is_some() {
        skip_ws_comma(&mut chars);
        if chars.peek().is_none() {
            break;
        }

        let cmd = if chars.peek().map_or(false, |c| c.is_ascii_alphabetic()) {
            chars.next().unwrap()
        } else if last_cmd == 'M' {
            'L'
        } else if last_cmd == 'm' {
            'l'
        } else {
            last_cmd
        };

        match cmd {
            'M' => {
                let x = mx(parse_number(&mut chars));
                let y = my(parse_number(&mut chars));
                emitter.move_to(x, y);
                cur_x = x;
                cur_y = y;
                start_x = x;
                start_y = y;
                last_ctrl_x = x;
                last_ctrl_y = y;
            }
            'm' => {
                let dx = parse_number(&mut chars) * sx;
                let dy = parse_number(&mut chars) * sy;
                let x = cur_x + dx;
                let y = cur_y + dy;
                emitter.move_to(x, y);
                cur_x = x;
                cur_y = y;
                start_x = x;
                start_y = y;
                last_ctrl_x = x;
                last_ctrl_y = y;
            }
            'L' => {
                let x = mx(parse_number(&mut chars));
                let y = my(parse_number(&mut chars));
                emitter.line_to(x, y);
                cur_x = x;
                cur_y = y;
                last_ctrl_x = x;
                last_ctrl_y = y;
            }
            'l' => {
                let dx = parse_number(&mut chars) * sx;
                let dy = parse_number(&mut chars) * sy;
                let x = cur_x + dx;
                let y = cur_y + dy;
                emitter.line_to(x, y);
                cur_x = x;
                cur_y = y;
                last_ctrl_x = x;
                last_ctrl_y = y;
            }
            'H' => {
                let x = mx(parse_number(&mut chars));
                emitter.line_to(x, cur_y);
                cur_x = x;
                last_ctrl_x = x;
                last_ctrl_y = cur_y;
            }
            'h' => {
                let dx = parse_number(&mut chars) * sx;
                let x = cur_x + dx;
                emitter.line_to(x, cur_y);
                cur_x = x;
                last_ctrl_x = x;
                last_ctrl_y = cur_y;
            }
            'V' => {
                let y = my(parse_number(&mut chars));
                emitter.line_to(cur_x, y);
                cur_y = y;
                last_ctrl_x = cur_x;
                last_ctrl_y = y;
            }
            'v' => {
                let dy = parse_number(&mut chars) * sy;
                let y = cur_y + dy;
                emitter.line_to(cur_x, y);
                cur_y = y;
                last_ctrl_x = cur_x;
                last_ctrl_y = y;
            }
            'C' => {
                let x1 = mx(parse_number(&mut chars));
                let y1 = my(parse_number(&mut chars));
                let x2 = mx(parse_number(&mut chars));
                let y2 = my(parse_number(&mut chars));
                let x = mx(parse_number(&mut chars));
                let y = my(parse_number(&mut chars));
                emitter.curve_to(x1, y1, x2, y2, x, y);
                cur_x = x;
                cur_y = y;
                last_ctrl_x = x2;
                last_ctrl_y = y2;
            }
            'c' => {
                let dx1 = parse_number(&mut chars) * sx;
                let dy1 = parse_number(&mut chars) * sy;
                let dx2 = parse_number(&mut chars) * sx;
                let dy2 = parse_number(&mut chars) * sy;
                let dx = parse_number(&mut chars) * sx;
                let dy = parse_number(&mut chars) * sy;
                emitter.curve_to(
                    cur_x + dx1,
                    cur_y + dy1,
                    cur_x + dx2,
                    cur_y + dy2,
                    cur_x + dx,
                    cur_y + dy,
                );
                last_ctrl_x = cur_x + dx2;
                last_ctrl_y = cur_y + dy2;
                cur_x += dx;
                cur_y += dy;
            }
            'S' => {
                let x1 = 2.0 * cur_x - last_ctrl_x;
                let y1 = 2.0 * cur_y - last_ctrl_y;
                let x2 = mx(parse_number(&mut chars));
                let y2 = my(parse_number(&mut chars));
                let x = mx(parse_number(&mut chars));
                let y = my(parse_number(&mut chars));
                emitter.curve_to(x1, y1, x2, y2, x, y);
                cur_x = x;
                cur_y = y;
                last_ctrl_x = x2;
                last_ctrl_y = y2;
            }
            's' => {
                let x1 = 2.0 * cur_x - last_ctrl_x;
                let y1 = 2.0 * cur_y - last_ctrl_y;
                let dx2 = parse_number(&mut chars) * sx;
                let dy2 = parse_number(&mut chars) * sy;
                let dx = parse_number(&mut chars) * sx;
                let dy = parse_number(&mut chars) * sy;
                emitter.curve_to(
                    x1,
                    y1,
                    cur_x + dx2,
                    cur_y + dy2,
                    cur_x + dx,
                    cur_y + dy,
                );
                last_ctrl_x = cur_x + dx2;
                last_ctrl_y = cur_y + dy2;
                cur_x += dx;
                cur_y += dy;
            }
            'Q' => {
                let cx = mx(parse_number(&mut chars));
                let cy = my(parse_number(&mut chars));
                let x = mx(parse_number(&mut chars));
                let y = my(parse_number(&mut chars));
                emit_quad(emitter, cur_x, cur_y, cx, cy, x, y);
                cur_x = x;
                cur_y = y;
                last_ctrl_x = cx;
                last_ctrl_y = cy;
            }
            'q' => {
                let dcx = parse_number(&mut chars) * sx;
                let dcy = parse_number(&mut chars) * sy;
                let dx = parse_number(&mut chars) * sx;
                let dy = parse_number(&mut chars) * sy;
                let cx = cur_x + dcx;
                let cy = cur_y + dcy;
                let x = cur_x + dx;
                let y = cur_y + dy;
                emit_quad(emitter, cur_x, cur_y, cx, cy, x, y);
                last_ctrl_x = cx;
                last_ctrl_y = cy;
                cur_x = x;
                cur_y = y;
            }
            'T' => {
                let cx = 2.0 * cur_x - last_ctrl_x;
                let cy = 2.0 * cur_y - last_ctrl_y;
                let x = mx(parse_number(&mut chars));
                let y = my(parse_number(&mut chars));
                emit_quad(emitter, cur_x, cur_y, cx, cy, x, y);
                cur_x = x;
                cur_y = y;
                last_ctrl_x = cx;
                last_ctrl_y = cy;
            }
            't' => {
                let cx = 2.0 * cur_x - last_ctrl_x;
                let cy = 2.0 * cur_y - last_ctrl_y;
                let dx = parse_number(&mut chars) * sx;
                let dy = parse_number(&mut chars) * sy;
                let x = cur_x + dx;
                let y = cur_y + dy;
                emit_quad(emitter, cur_x, cur_y, cx, cy, x, y);
                last_ctrl_x = cx;
                last_ctrl_y = cy;
                cur_x = x;
                cur_y = y;
            }
            'A' | 'a' => {
                let rx = parse_number(&mut chars).abs() * sx;
                let ry = parse_number(&mut chars).abs() * sy;
                let _x_rot = parse_number(&mut chars);
                let large_arc = parse_number(&mut chars) != 0.0;
                let sweep = parse_number(&mut chars) != 0.0;
                let raw_x = parse_number(&mut chars);
                let raw_y = parse_number(&mut chars);
                let (ex, ey) = if cmd == 'a' {
                    (cur_x + raw_x * sx, cur_y + raw_y * sy)
                } else {
                    (mx(raw_x), my(raw_y))
                };
                arc_to_bezier(emitter, cur_x, cur_y, ex, ey, rx, ry, large_arc, sweep);
                cur_x = ex;
                cur_y = ey;
                last_ctrl_x = ex;
                last_ctrl_y = ey;
            }
            'Z' | 'z' => {
                emitter.close();
                cur_x = start_x;
                cur_y = start_y;
                last_ctrl_x = start_x;
                last_ctrl_y = start_y;
            }
            _ => {}
        }
        last_cmd = cmd;
    }
}

/// Quadratic → cubic lift: `cp1 = P0 + 2/3·(Pc - P0)`, `cp2 = P2 + 2/3·(Pc - P2)`.
fn emit_quad(
    emitter: &mut dyn PathEmitter,
    cur_x: f64,
    cur_y: f64,
    cx: f64,
    cy: f64,
    x: f64,
    y: f64,
) {
    let cp1x = cur_x + 2.0 / 3.0 * (cx - cur_x);
    let cp1y = cur_y + 2.0 / 3.0 * (cy - cur_y);
    let cp2x = x + 2.0 / 3.0 * (cx - x);
    let cp2y = y + 2.0 / 3.0 * (cy - y);
    emitter.curve_to(cp1x, cp1y, cp2x, cp2y, x, y);
}

/// Approximate an SVG endpoint-parameterized arc with cubic segments
/// (≤90° each). Control-handle length is the circular-arc kappa
/// `(4/3)·tan(seg/4)`.
#[allow(clippy::too_many_arguments)]
fn arc_to_bezier(
    emitter: &mut dyn PathEmitter,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    rx: f64,
    ry: f64,
    large_arc: bool,
    sweep: bool,
) {
    if rx < 1e-6 || ry < 1e-6 {
        emitter.line_to(x2, y2);
        return;
    }

    let dx = (x1 - x2) / 2.0;
    let dy = (y1 - y2) / 2.0;

    let mut rx = rx;
    let mut ry = ry;

    let check = (dx * dx) / (rx * rx) + (dy * dy) / (ry * ry);
    if check > 1.0 {
        let s = check.sqrt();
        rx *= s;
        ry *= s;
    }

    let rxsq = rx * rx;
    let rysq = ry * ry;
    let dxsq = dx * dx;
    let dysq = dy * dy;

    let num = (rxsq * rysq - rxsq * dysq - rysq * dxsq).max(0.0);
    let den = rxsq * dysq + rysq * dxsq;
    let sq = if den < 1e-10 { 0.0 } else { (num / den).sqrt() };

    let sign = if large_arc == sweep { -1.0 } else { 1.0 };
    let cx = sign * sq * (rx * dy / ry) + (x1 + x2) / 2.0;
    let cy = sign * sq * -(ry * dx / rx) + (y1 + y2) / 2.0;

    let theta1 = ((y1 - cy) / ry).atan2((x1 - cx) / rx);
    let mut dtheta = ((y2 - cy) / ry).atan2((x2 - cx) / rx) - theta1;

    if sweep && dtheta < 0.0 {
        dtheta += 2.0 * std::f64::consts::PI;
    } else if !sweep && dtheta > 0.0 {
        dtheta -= 2.0 * std::f64::consts::PI;
    }

    let n_segs = (dtheta.abs() / std::f64::consts::FRAC_PI_2).ceil() as usize;
    if n_segs == 0 {
        return;
    }
    let seg_angle = dtheta / n_segs as f64;

    let mut angle = theta1;
    for _ in 0..n_segs {
        let next_angle = angle + seg_angle;
        let alpha = (seg_angle / 4.0).tan() * 4.0 / 3.0;

        let cos_a = angle.cos();
        let sin_a = angle.sin();
        let cos_b = next_angle.cos();
        let sin_b = next_angle.sin();

        let p2x = cx + rx * cos_b;
        let p2y = cy + ry * sin_b;

        let cp1x = cx + rx * cos_a - alpha * rx * sin_a;
        let cp1y = cy + ry * sin_a + alpha * ry * cos_a;
        let cp2x = p2x + alpha * rx * sin_b;
        let cp2y = p2y - alpha * ry * cos_b;

        emitter.curve_to(cp1x, cp1y, cp2x, cp2y, p2x, p2y);
        angle = next_angle;
    }
}

fn skip_ws_comma(chars: &mut std::iter::Peekable<std::str::Chars>) {
    while chars
        .peek()
        .map_or(false, |&c| c == ' ' || c == ',' || c == '\t' || c == '\n' || c == '\r')
    {
        chars.next();
    }
}

fn parse_number(chars: &mut std::iter::Peekable<std::str::Chars>) -> f64 {
    skip_ws_comma(chars);
    let mut s = String::new();

    if chars.peek() == Some(&'-') || chars.peek() == Some(&'+') {
        s.push(chars.next().unwrap());
    }
    while chars.peek().map_or(false, |c| c.is_ascii_digit()) {
        s.push(chars.next().unwrap());
    }
    if chars.peek() == Some(&'.') {
        s.push(chars.next().unwrap());
        while chars.peek().map_or(false, |c| c.is_ascii_digit()) {
            s.push(chars.next().unwrap());
        }
    }
    if chars.peek().map_or(false, |&c| c == 'e' || c == 'E') {
        s.push(chars.next().unwrap());
        if chars.peek() == Some(&'-') || chars.peek() == Some(&'+') {
            s.push(chars.next().unwrap());
        }
        while chars.peek().map_or(false, |c| c.is_ascii_digit()) {
            s.push(chars.next().unwrap());
        }
    }

    s.parse::<f64>().unwrap_or(0.0)
}

// =========================================================================
// GDI+ adapter — builds a `GpPath` from the parsed commands. Unlike GSK,
// GDI+ has no explicit "move to"; a new subpath is opened with
// `GdipStartPathFigure` and each segment passes its own start point (the
// tracked current point) so GDI+ never inserts a spurious connecting line.
// =========================================================================

struct GdiPlusEmitter {
    path: *mut GpPath,
    cur: (f32, f32),
}

impl PathEmitter for GdiPlusEmitter {
    fn move_to(&mut self, x: f64, y: f64) {
        unsafe {
            // Start a fresh figure so this subpath isn't joined to the
            // previous one.
            let _ = GdipStartPathFigure(self.path);
        }
        self.cur = (x as f32, y as f32);
    }
    fn line_to(&mut self, x: f64, y: f64) {
        unsafe {
            let _ = GdipAddPathLine(self.path, self.cur.0, self.cur.1, x as f32, y as f32);
        }
        self.cur = (x as f32, y as f32);
    }
    fn curve_to(&mut self, c1x: f64, c1y: f64, c2x: f64, c2y: f64, x: f64, y: f64) {
        unsafe {
            let _ = GdipAddPathBezier(
                self.path,
                self.cur.0,
                self.cur.1,
                c1x as f32,
                c1y as f32,
                c2x as f32,
                c2y as f32,
                x as f32,
                y as f32,
            );
        }
        self.cur = (x as f32, y as f32);
    }
    fn close(&mut self) {
        unsafe {
            let _ = GdipClosePathFigure(self.path);
        }
    }
}

// =========================================================================
// Scene-paint entry point.
// =========================================================================

/// Lucide authors strokes at width 2 in a 24-unit view box; the painter
/// scales that by the glyph's fit-scale so a `size(N)` icon keeps the same
/// relative stroke weight at any size.
const BASE_STROKE_WIDTH: f32 = 2.0;

/// Paint state for one icon node, held on its `NodeMeta`.
pub(crate) struct IconPaint {
    /// SVG path `d` strings (owned so `update_icon_data` can swap the glyph
    /// without rebuilding the node).
    pub paths: Vec<String>,
    /// Icon view-box `(w, h)` — the coordinate space `paths` live in.
    pub view_box: (u16, u16),
    /// Stroke/fill color.
    pub color: Rgba,
    /// `true` → fill (honoring `even_odd`), `false` → stroke.
    pub filled: bool,
    /// Even-odd fill rule when `filled`; else non-zero winding.
    pub even_odd: bool,
}

impl IconPaint {
    pub(crate) fn from_data(data: &IconData, color: Rgba) -> Self {
        IconPaint {
            paths: data.paths.iter().map(|p| p.to_string()).collect(),
            view_box: data.view_box,
            color,
            filled: data.filled,
            even_odd: data.fill_rule == FillRule::EvenOdd,
        }
    }

    /// Overwrite geometry from new `IconData`, keeping the current color.
    pub(crate) fn set_data(&mut self, data: &IconData) {
        self.paths = data.paths.iter().map(|p| p.to_string()).collect();
        self.view_box = data.view_box;
        self.filled = data.filled;
        self.even_odd = data.fill_rule == FillRule::EvenOdd;
    }
}

/// Paint the glyph into the current graphics at `(0, 0)`..`(w, h)` —
/// world transform already positions the node's box. The view-box is
/// fitted with a uniform centered scale. `alpha` is the cumulative scene
/// opacity multiplied into the icon color.
pub(crate) unsafe fn paint_into(g: *mut GpGraphics, p: &IconPaint, w: f32, h: f32, alpha: f32) {
    let (w, h) = (w as f64, h as f64);
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let (vw, vh) = p.view_box;
    let (vw, vh) = (vw.max(1) as f64, vh.max(1) as f64);
    // Uniform fit-scale (square glyph: smaller side wins), centered.
    let scale = (w / vw).min(h / vh);
    let ox = (w - vw * scale) / 2.0;
    let oy = (h - vh * scale) / 2.0;

    let mut path: *mut GpPath = std::ptr::null_mut();
    let fill_mode = if p.even_odd { FillModeAlternate } else { FillModeWinding };
    if GdipCreatePath(fill_mode, &mut path).0 != 0 || path.is_null() {
        return;
    }
    let mut emitter = GdiPlusEmitter { path, cur: (0.0, 0.0) };
    for d in &p.paths {
        parse_svg_path(d, scale, scale, ox, oy, &mut emitter);
    }
    let argb = crate::scene::argb_with_alpha(p.color.to_argb_u32(), alpha);
    if p.filled {
        let mut brush: *mut GpSolidFill = std::ptr::null_mut();
        if GdipCreateSolidFill(argb, &mut brush).0 == 0 {
            let _ = GdipFillPath(g, brush as *mut GpBrush, path);
            let _ = GdipDeleteBrush(brush as *mut GpBrush);
        }
    } else {
        // Rounded caps + joins at the scaled Lucide weight.
        let width = (BASE_STROKE_WIDTH as f64 * scale) as f32;
        let mut pen: *mut GpPen = std::ptr::null_mut();
        if GdipCreatePen1(argb, width, UnitPixel, &mut pen).0 == 0 {
            let _ = GdipSetPenStartCap(pen, LineCapRound);
            let _ = GdipSetPenEndCap(pen, LineCapRound);
            let _ = GdipSetPenLineJoin(pen, LineJoinRound);
            let _ = GdipDrawPath(g, pen, path);
            let _ = GdipDeletePen(pen);
        }
    }
    let _ = GdipDeletePath(path);
}

#[cfg(test)]
mod tests {
    //! Pure-data parser tests (no GDI+ context). These mirror the Linux +
    //! Apple backends' tests so the three parsers stay byte-identical.
    use super::*;

    #[derive(Default)]
    struct LogEmitter {
        ops: Vec<String>,
    }
    impl PathEmitter for LogEmitter {
        fn move_to(&mut self, x: f64, y: f64) {
            self.ops.push(format!("M({x},{y})"));
        }
        fn line_to(&mut self, x: f64, y: f64) {
            self.ops.push(format!("L({x},{y})"));
        }
        fn curve_to(&mut self, c1x: f64, c1y: f64, c2x: f64, c2y: f64, x: f64, y: f64) {
            self.ops.push(format!("C({c1x},{c1y};{c2x},{c2y};{x},{y})"));
        }
        fn close(&mut self) {
            self.ops.push("Z".into());
        }
    }

    #[test]
    fn move_line_close_dispatches_in_order() {
        let mut e = LogEmitter::default();
        parse_svg_path("M 1 2 L 3 4 Z", 1.0, 1.0, 0.0, 0.0, &mut e);
        assert_eq!(e.ops, vec!["M(1,2)", "L(3,4)", "Z"]);
    }

    #[test]
    fn scale_and_offset_apply_to_absolute_coords() {
        let mut e = LogEmitter::default();
        parse_svg_path("M 1 2 L 3 4", 2.0, 3.0, 10.0, 100.0, &mut e);
        assert_eq!(e.ops, vec!["M(12,106)", "L(16,112)"]);
    }

    #[test]
    fn implicit_lineto_after_moveto() {
        let mut e = LogEmitter::default();
        parse_svg_path("M 0 0 1 1 2 2", 1.0, 1.0, 0.0, 0.0, &mut e);
        assert_eq!(e.ops, vec!["M(0,0)", "L(1,1)", "L(2,2)"]);
    }

    #[test]
    fn relative_hv_and_lineto_track_current_point() {
        let mut e = LogEmitter::default();
        parse_svg_path("M 10 10 l 5 0 v 5 h -5", 1.0, 1.0, 0.0, 0.0, &mut e);
        assert_eq!(e.ops, vec!["M(10,10)", "L(15,10)", "L(15,15)", "L(10,15)"]);
    }

    #[test]
    fn quadratic_lifts_to_cubic() {
        // P0=(0,0) Pc=(3,3) P2=(6,0) → cp1=(2,2), cp2=(4,2).
        let mut e = LogEmitter::default();
        parse_svg_path("M 0 0 Q 3 3 6 0", 1.0, 1.0, 0.0, 0.0, &mut e);
        assert_eq!(e.ops, vec!["M(0,0)", "C(2,2;4,2;6,0)"]);
    }

    #[derive(Default)]
    struct PointEmitter {
        curves: Vec<(f64, f64, f64, f64, f64, f64)>,
    }
    impl PathEmitter for PointEmitter {
        fn move_to(&mut self, _x: f64, _y: f64) {}
        fn line_to(&mut self, _x: f64, _y: f64) {}
        fn curve_to(&mut self, c1x: f64, c1y: f64, c2x: f64, c2y: f64, x: f64, y: f64) {
            self.curves.push((c1x, c1y, c2x, c2y, x, y));
        }
        fn close(&mut self) {}
    }

    /// A quarter-circle arc must use the kappa `(4/3)·tan(θ/4)` handles
    /// (0.5523·r for 90°). Pins the trash-2 can corner (same assertion as
    /// the Linux + Apple backends).
    #[test]
    fn arc_quarter_circle_uses_correct_kappa_handles() {
        let mut e = PointEmitter::default();
        parse_svg_path("M19 6v14a2 2 0 0 1-2 2", 1.0, 1.0, 0.0, 0.0, &mut e);
        assert_eq!(e.curves.len(), 1);
        let (c1x, c1y, c2x, c2y, ex, ey) = e.curves[0];
        let approx = |a: f64, b: f64| (a - b).abs() < 0.01;
        assert!(approx(c1x, 19.0) && approx(c1y, 21.105), "C1 was ({c1x},{c1y})");
        assert!(approx(c2x, 18.105) && approx(c2y, 22.0), "C2 was ({c2x},{c2y})");
        assert!(approx(ex, 17.0) && approx(ey, 22.0), "end was ({ex},{ey})");
    }

    #[test]
    fn smooth_cubic_reflects_previous_control() {
        let mut e = PointEmitter::default();
        parse_svg_path("M 0 0 C 1 2 3 4 5 6 S 8 9 10 11", 1.0, 1.0, 0.0, 0.0, &mut e);
        assert_eq!(e.curves.len(), 2);
        let (c1x, c1y, ..) = e.curves[1];
        assert!((c1x - 7.0).abs() < 1e-9 && (c1y - 8.0).abs() < 1e-9, "reflected cp1 = (7,8)");
    }
}
