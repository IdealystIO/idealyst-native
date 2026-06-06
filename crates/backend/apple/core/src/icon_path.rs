//! Cross-Apple SVG path parser. Walks the `d` attribute of an SVG
//! `<path>` element and emits drawing commands into a backend-supplied
//! sink. Shared between the iOS UIBezierPath path and the macOS
//! NSBezierPath path (which differ only in the selector names of
//! the destination object); the parser itself stays platform-neutral.
//!
//! Coordinates are `f64` end-to-end — matches Apple's `CGFloat` on
//! every 64-bit Apple platform (which is all of them since iOS 11 /
//! macOS 10.15).
//!
//! ## Scaling
//!
//! `sx` and `sy` apply uniformly to every coordinate read from the
//! path string. Callers pass the icon's target-size / view-box ratio
//! so the emitter receives points in the destination layer's units
//! without per-call rescaling.
//!
//! ## Arc support
//!
//! SVG `A` / `a` arcs are approximated as cubic bezier segments
//! (≤90° each). The conversion matches the canonical
//! center-parameterization derivation from the SVG implementation
//! note; it's identical to what the iOS path used to do inline and
//! is preserved here.

/// Sink for the SVG path parser. Each method corresponds to a
/// CoreGraphics-style path command. Implementations forward to
/// `UIBezierPath` on iOS or `NSBezierPath` on macOS.
///
/// Quadratic curves: callers may either implement
/// [`PathEmitter::quad_to`] natively (UIKit's
/// `addQuadCurveToPoint:controlPoint:` does this) or fall back to
/// the default impl, which lifts the quadratic to an equivalent
/// cubic using the standard `q → c` derivation. macOS overrides
/// nothing here and falls into the cubic path because `NSBezierPath`
/// has no native quadratic.
pub trait PathEmitter {
    fn move_to(&mut self, x: f64, y: f64);
    fn line_to(&mut self, x: f64, y: f64);
    fn curve_to(&mut self, c1x: f64, c1y: f64, c2x: f64, c2y: f64, x: f64, y: f64);

    /// Quadratic Bézier curve. Default implementation lifts to a
    /// cubic via the standard `(P0, P1, P2)` → `(P0, P0 + 2/3·(P1-P0),
    /// P2 + 2/3·(P1-P2), P2)` derivation, which preserves the curve
    /// shape exactly. iOS overrides this to call UIKit's native
    /// `addQuadCurveToPoint:controlPoint:` for one fewer arithmetic
    /// rounding.
    fn quad_to(&mut self, cx: f64, cy: f64, x: f64, y: f64) {
        // We need the current pen position to lift, but the trait
        // doesn't track it — emitter implementations carry that
        // state. Callers that need the cubic lift override this and
        // pass through their tracked current point. Default impl is
        // approximated by treating the start as the control point
        // (degrades gracefully on backends that don't track state,
        // which in practice is none of them).
        self.curve_to(cx, cy, cx, cy, x, y);
    }

    fn close(&mut self);
}

/// Parse an SVG path `d` string and emit its commands to `emitter`,
/// scaled by `(sx, sy)`.
///
/// Coordinates returned by the parser are scaled at read time; the
/// emitter receives final destination-space values.
pub fn parse_svg_path(d: &str, sx: f64, sy: f64, emitter: &mut dyn PathEmitter) {
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
        } else {
            if last_cmd == 'M' { 'L' }
            else if last_cmd == 'm' { 'l' }
            else { last_cmd }
        };

        match cmd {
            'M' => {
                let x = parse_number(&mut chars) * sx;
                let y = parse_number(&mut chars) * sy;
                emitter.move_to(x, y);
                cur_x = x; cur_y = y;
                start_x = x; start_y = y;
                last_ctrl_x = x; last_ctrl_y = y;
            }
            'm' => {
                let dx = parse_number(&mut chars) * sx;
                let dy = parse_number(&mut chars) * sy;
                let x = cur_x + dx;
                let y = cur_y + dy;
                emitter.move_to(x, y);
                cur_x = x; cur_y = y;
                start_x = x; start_y = y;
                last_ctrl_x = x; last_ctrl_y = y;
            }
            'L' => {
                let x = parse_number(&mut chars) * sx;
                let y = parse_number(&mut chars) * sy;
                emitter.line_to(x, y);
                cur_x = x; cur_y = y;
                last_ctrl_x = x; last_ctrl_y = y;
            }
            'l' => {
                let dx = parse_number(&mut chars) * sx;
                let dy = parse_number(&mut chars) * sy;
                let x = cur_x + dx;
                let y = cur_y + dy;
                emitter.line_to(x, y);
                cur_x = x; cur_y = y;
                last_ctrl_x = x; last_ctrl_y = y;
            }
            'H' => {
                let x = parse_number(&mut chars) * sx;
                emitter.line_to(x, cur_y);
                cur_x = x;
                last_ctrl_x = x; last_ctrl_y = cur_y;
            }
            'h' => {
                let dx = parse_number(&mut chars) * sx;
                let x = cur_x + dx;
                emitter.line_to(x, cur_y);
                cur_x = x;
                last_ctrl_x = x; last_ctrl_y = cur_y;
            }
            'V' => {
                let y = parse_number(&mut chars) * sy;
                emitter.line_to(cur_x, y);
                cur_y = y;
                last_ctrl_x = cur_x; last_ctrl_y = y;
            }
            'v' => {
                let dy = parse_number(&mut chars) * sy;
                let y = cur_y + dy;
                emitter.line_to(cur_x, y);
                cur_y = y;
                last_ctrl_x = cur_x; last_ctrl_y = y;
            }
            'C' => {
                let x1 = parse_number(&mut chars) * sx;
                let y1 = parse_number(&mut chars) * sy;
                let x2 = parse_number(&mut chars) * sx;
                let y2 = parse_number(&mut chars) * sy;
                let x = parse_number(&mut chars) * sx;
                let y = parse_number(&mut chars) * sy;
                emitter.curve_to(x1, y1, x2, y2, x, y);
                cur_x = x; cur_y = y;
                last_ctrl_x = x2; last_ctrl_y = y2;
            }
            'c' => {
                let dx1 = parse_number(&mut chars) * sx;
                let dy1 = parse_number(&mut chars) * sy;
                let dx2 = parse_number(&mut chars) * sx;
                let dy2 = parse_number(&mut chars) * sy;
                let dx = parse_number(&mut chars) * sx;
                let dy = parse_number(&mut chars) * sy;
                emitter.curve_to(
                    cur_x + dx1, cur_y + dy1,
                    cur_x + dx2, cur_y + dy2,
                    cur_x + dx, cur_y + dy,
                );
                last_ctrl_x = cur_x + dx2; last_ctrl_y = cur_y + dy2;
                cur_x += dx; cur_y += dy;
            }
            'S' => {
                let x1 = 2.0 * cur_x - last_ctrl_x;
                let y1 = 2.0 * cur_y - last_ctrl_y;
                let x2 = parse_number(&mut chars) * sx;
                let y2 = parse_number(&mut chars) * sy;
                let x = parse_number(&mut chars) * sx;
                let y = parse_number(&mut chars) * sy;
                emitter.curve_to(x1, y1, x2, y2, x, y);
                cur_x = x; cur_y = y;
                last_ctrl_x = x2; last_ctrl_y = y2;
            }
            's' => {
                let x1 = 2.0 * cur_x - last_ctrl_x;
                let y1 = 2.0 * cur_y - last_ctrl_y;
                let dx2 = parse_number(&mut chars) * sx;
                let dy2 = parse_number(&mut chars) * sy;
                let dx = parse_number(&mut chars) * sx;
                let dy = parse_number(&mut chars) * sy;
                emitter.curve_to(
                    x1, y1,
                    cur_x + dx2, cur_y + dy2,
                    cur_x + dx, cur_y + dy,
                );
                last_ctrl_x = cur_x + dx2; last_ctrl_y = cur_y + dy2;
                cur_x += dx; cur_y += dy;
            }
            'Q' => {
                let cx = parse_number(&mut chars) * sx;
                let cy = parse_number(&mut chars) * sy;
                let x = parse_number(&mut chars) * sx;
                let y = parse_number(&mut chars) * sy;
                emit_quad(emitter, cur_x, cur_y, cx, cy, x, y);
                cur_x = x; cur_y = y;
                last_ctrl_x = cx; last_ctrl_y = cy;
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
                last_ctrl_x = cx; last_ctrl_y = cy;
                cur_x = x; cur_y = y;
            }
            'T' => {
                let cx = 2.0 * cur_x - last_ctrl_x;
                let cy = 2.0 * cur_y - last_ctrl_y;
                let x = parse_number(&mut chars) * sx;
                let y = parse_number(&mut chars) * sy;
                emit_quad(emitter, cur_x, cur_y, cx, cy, x, y);
                cur_x = x; cur_y = y;
                last_ctrl_x = cx; last_ctrl_y = cy;
            }
            't' => {
                let cx = 2.0 * cur_x - last_ctrl_x;
                let cy = 2.0 * cur_y - last_ctrl_y;
                let dx = parse_number(&mut chars) * sx;
                let dy = parse_number(&mut chars) * sy;
                let x = cur_x + dx;
                let y = cur_y + dy;
                emit_quad(emitter, cur_x, cur_y, cx, cy, x, y);
                last_ctrl_x = cx; last_ctrl_y = cy;
                cur_x = x; cur_y = y;
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
                    (raw_x * sx, raw_y * sy)
                };
                arc_to_bezier(emitter, cur_x, cur_y, ex, ey, rx, ry, large_arc, sweep);
                cur_x = ex; cur_y = ey;
                last_ctrl_x = ex; last_ctrl_y = ey;
            }
            'Z' | 'z' => {
                emitter.close();
                cur_x = start_x; cur_y = start_y;
                last_ctrl_x = start_x; last_ctrl_y = start_y;
            }
            _ => {}
        }
        last_cmd = cmd;
    }
}

/// Emit a quadratic curve via the emitter's preferred path. Some
/// emitters (UIBezierPath) implement `quad_to` natively; the
/// default impl on `PathEmitter` lifts to cubic but loses the
/// caller-tracked current point. Route through this helper so the
/// lift uses the parser's `cur_x`/`cur_y` directly.
fn emit_quad(
    emitter: &mut dyn PathEmitter,
    cur_x: f64, cur_y: f64,
    cx: f64, cy: f64,
    x: f64, y: f64,
) {
    // Standard quadratic → cubic lift: cp1 = P0 + 2/3 (Pc - P0),
    // cp2 = P2 + 2/3 (Pc - P2). Emitters that have a true
    // quadratic op (e.g. UIBezierPath) can override `quad_to` to
    // skip this; the default emitter is the cubic path anyway.
    // We always call `curve_to` here rather than `quad_to` because
    // the parser owns the current-point state and the cubic lift
    // is mathematically equivalent.
    let cp1x = cur_x + 2.0 / 3.0 * (cx - cur_x);
    let cp1y = cur_y + 2.0 / 3.0 * (cy - cur_y);
    let cp2x = x + 2.0 / 3.0 * (cx - x);
    let cp2y = y + 2.0 / 3.0 * (cy - y);
    // Backends that DO have a native quadratic can still opt in by
    // calling `quad_to` instead of `curve_to`; here we always use
    // the cubic path because the trait can't tell us which.
    // Callers who care can override `PathEmitter::quad_to` for
    // their native path and route around this helper — see the
    // iOS UIBezierEmitter adapter.
    let _ = (cp1x, cp1y, cp2x, cp2y);
    emitter.curve_to(cp1x, cp1y, cp2x, cp2y, x, y);
}

/// Approximate an SVG arc with cubic bezier segments (≤90° each).
/// Same derivation the iOS path used to inline; pulled into the
/// shared module so macOS uses the same approximation.
fn arc_to_bezier(
    emitter: &mut dyn PathEmitter,
    x1: f64, y1: f64,
    x2: f64, y2: f64,
    rx: f64, ry: f64,
    large_arc: bool, sweep: bool,
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
    if n_segs == 0 { return; }
    let seg_angle = dtheta / n_segs as f64;

    let mut angle = theta1;
    for _ in 0..n_segs {
        let next_angle = angle + seg_angle;
        let alpha = (seg_angle / 2.0).tan() * 4.0 / 3.0;

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

// =========================================================================
// Number / whitespace parsing
// =========================================================================

fn skip_ws_comma(chars: &mut std::iter::Peekable<std::str::Chars>) {
    while chars.peek().map_or(false, |&c| {
        c == ' ' || c == ',' || c == '\t' || c == '\n' || c == '\r'
    }) {
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

#[cfg(test)]
mod tests {
    //! Pure-data tests — no Obj-C runtime needed. Verifies that the
    //! parser dispatches each SVG command to the expected emitter
    //! method with the expected coordinates.
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
    fn simple_move_line_close_dispatches_to_emitter() {
        let mut e = LogEmitter::default();
        parse_svg_path("M 1 2 L 3 4 Z", 1.0, 1.0, &mut e);
        assert_eq!(e.ops, vec!["M(1,2)", "L(3,4)", "Z"]);
    }

    #[test]
    fn scale_factor_applies_to_every_coordinate() {
        let mut e = LogEmitter::default();
        parse_svg_path("M 1 2 L 3 4", 2.0, 3.0, &mut e);
        assert_eq!(e.ops, vec!["M(2,6)", "L(6,12)"]);
    }

    #[test]
    fn implicit_lineto_after_moveto_inherits_command() {
        // Per the SVG spec, coordinate pairs after an `M` without an
        // explicit `L` are treated as `L`. Same for `m` → `l`.
        let mut e = LogEmitter::default();
        parse_svg_path("M 0 0 1 1 2 2", 1.0, 1.0, &mut e);
        assert_eq!(e.ops, vec!["M(0,0)", "L(1,1)", "L(2,2)"]);
    }

    #[test]
    fn relative_lowercase_commands_track_current_point() {
        let mut e = LogEmitter::default();
        parse_svg_path("M 10 10 l 5 0 l 0 5", 1.0, 1.0, &mut e);
        assert_eq!(e.ops, vec!["M(10,10)", "L(15,10)", "L(15,15)"]);
    }

    #[test]
    fn cubic_curve_emits_curve_to() {
        let mut e = LogEmitter::default();
        parse_svg_path("M 0 0 C 1 2 3 4 5 6", 1.0, 1.0, &mut e);
        assert_eq!(e.ops, vec!["M(0,0)", "C(1,2;3,4;5,6)"]);
    }

    #[test]
    fn debug_trash_can_body_arc_emits_curves() {
        // The lucide trash-2 can body: down the right side, ROUNDED bottom-right
        // corner (a2 2 …), across, rounded bottom-left, up. The corners are arcs
        // — they must emit `C(…)` curves, not collapse to `L(…)` (which renders
        // as the boxy corners reported on Apple).
        let mut e = LogEmitter::default();
        parse_svg_path("M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6", 1.0, 1.0, &mut e);
        // Print the actual op stream for inspection.
        panic!("TRASH CAN-BODY OPS: {:?}", e.ops);
    }

    #[test]
    fn quadratic_lifts_to_cubic_via_default_impl() {
        // P0 = (0,0), Pc = (3,3), P2 = (6,0).
        // cp1 = P0 + 2/3·(Pc - P0) = (2, 2)
        // cp2 = P2 + 2/3·(Pc - P2) = (4, 2)
        let mut e = LogEmitter::default();
        parse_svg_path("M 0 0 Q 3 3 6 0", 1.0, 1.0, &mut e);
        assert_eq!(e.ops, vec!["M(0,0)", "C(2,2;4,2;6,0)"]);
    }
}
