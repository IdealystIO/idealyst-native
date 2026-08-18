//! `Element::Icon` — vector icon rendering on GTK4.
//!
//! ## Why a custom `GtkWidget` (not a `GtkImage` / `GtkPicture`)
//!
//! The framework's icon data is a set of SVG path `d` strings in a
//! view-box coordinate space (Lucide-style, 24×24). GTK's image widgets
//! want a `GdkPaintable` / file / icon-name — there is no "here are some
//! path strings, stroke them" widget. So `IdealystIcon` subclasses
//! `GtkWidget` directly and paints the glyph in its own [`snapshot`] via
//! the retained-mode GSK path API ([`gsk::PathBuilder`] →
//! [`gsk::Path`], then `Snapshot::append_stroke` / `append_fill`). This
//! mirrors the macOS backend's `CAShapeLayer` approach: the same
//! `IconData.paths` produce a visually identical glyph on every backend
//! (repo rule §7 — mechanism diverges, output converges).
//!
//! ## Stroke vs fill
//!
//! Matches iOS / macOS: an *outlined* icon (`IconData.filled == false`,
//! the Lucide default) is STROKED with the icon color, rounded caps and
//! joins, line width `2 × scale` (Lucide authors at stroke-width 2 in a
//! 24 view-box); a *filled* icon (`filled == true`) is FILLED with the
//! icon color honoring its `fill_rule`, no stroke.
//!
//! ## Sizing
//!
//! The path is authored in view-box units; [`snapshot`] fits it into the
//! widget's allocated box with a uniform `min(w/vw, h/vh)` scale, centered
//! — so `icon(X).size(N)` (a width/height style the layout pass turns into
//! a `set_size_request`) renders at N px. The parser bakes the scale +
//! centering offset into the emitted coordinates.
//!
//! ## Sizing note
//!
//! A plain `GtkWidget` subclass needs no `measure` override: the layout
//! pass calls `set_size_request(w, h)` on every leaf node, and
//! `gtk_widget_measure` clamps min/natural up to the request, so the icon
//! reports its Taffy size to the parent's `size_allocate` for free.

use std::cell::{Cell, RefCell};

use gtk4::glib;
use gtk4::gsk;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

use runtime_shared::primitives::icon::{FillRule, IconData};

use crate::color;

// =========================================================================
// SVG path parser — pure logic, no GTK. Emits scaled/translated commands
// into a `PathEmitter` sink. Ported from the Apple backend's parser
// (`backend_apple_core::icon_path`) rather than depended on: that crate
// pulls in `objc2`, which must never enter the Linux dep graph. The arc →
// cubic-bézier approximation (kappa `(4/3)·tan(θ/4)` per ≤90° segment) is
// the proven derivation from there — see the regression test below.
// =========================================================================

/// Sink for the SVG path parser. Each method is a GSK-style path command
/// in *destination* (already scaled + translated) coordinates. The GTK
/// adapter forwards into a [`gsk::PathBuilder`]; the tests use a pure
/// logging sink so the parser is exercised without a GSK context.
pub trait PathEmitter {
    fn move_to(&mut self, x: f64, y: f64);
    fn line_to(&mut self, x: f64, y: f64);
    fn curve_to(&mut self, c1x: f64, c1y: f64, c2x: f64, c2y: f64, x: f64, y: f64);
    fn close(&mut self);
}

/// Parse an SVG path `d` string and emit its commands to `emitter`,
/// scaled by `(sx, sy)` and offset by `(ox, oy)`. Coordinates are scaled
/// then translated at read time, so the emitter receives final
/// destination-space values (the widget centers the glyph via `ox`/`oy`).
///
/// Supports the full Lucide command set: `M/m L/l H/h V/v C/c S/s Q/q T/t
/// A/a Z`. Quadratics are lifted to cubics; arcs are approximated by
/// cubic segments (the parser owns pen state, so smooth-curve reflection
/// and arc centering are exact).
pub fn parse_svg_path(
    d: &str,
    sx: f64,
    sy: f64,
    ox: f64,
    oy: f64,
    emitter: &mut dyn PathEmitter,
) {
    // Map a view-box coordinate to destination space (scale then offset).
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

/// Quadratic → cubic lift: `cp1 = P0 + 2/3·(Pc - P0)`, `cp2 = P2 + 2/3·(Pc
/// - P2)`, exact for GSK (which has a native `quad_to`, but the parser
/// owns pen state so the cubic form is simpler and identical in output).
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
/// `(4/3)·tan(seg/4)`; using `/2` overshoots handles ~2.41× and makes
/// rounded corners boxy (the bug pinned by
/// `arc_quarter_circle_uses_correct_kappa_handles`).
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
// GSK adapter — writes parsed commands into a `gsk::PathBuilder`. GSK
// coordinates are f32; the parser runs in f64 (arc trig precision) and
// narrows at the boundary.
// =========================================================================

struct GskEmitter {
    builder: gsk::PathBuilder,
}

impl PathEmitter for GskEmitter {
    fn move_to(&mut self, x: f64, y: f64) {
        self.builder.move_to(x as f32, y as f32);
    }
    fn line_to(&mut self, x: f64, y: f64) {
        self.builder.line_to(x as f32, y as f32);
    }
    fn curve_to(&mut self, c1x: f64, c1y: f64, c2x: f64, c2y: f64, x: f64, y: f64) {
        self.builder
            .cubic_to(c1x as f32, c1y as f32, c2x as f32, c2y as f32, x as f32, y as f32);
    }
    fn close(&mut self) {
        self.builder.close();
    }
}

/// The default icon color when the author sets none — matches the text
/// node default ([`crate::text::TextPaint::default`] = opaque black), the
/// Linux analogue of web's `currentColor` / the nearest text color.
pub(crate) const DEFAULT_ICON_COLOR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// Lucide authors strokes at width 2 in a 24-unit view box. The widget
/// scales that by the glyph's fit-scale so a `size(N)` icon keeps the
/// same relative stroke weight at any size.
const BASE_STROKE_WIDTH: f32 = 2.0;

// =========================================================================
// IdealystIcon — the custom widget.
// =========================================================================

mod imp {
    use super::*;

    pub struct IdealystIcon {
        /// SVG path `d` strings (owned so `update_icon_data` can swap the
        /// glyph without rebuilding the widget).
        pub paths: RefCell<Vec<String>>,
        /// Icon view-box `(w, h)` — the coordinate space `paths` live in.
        pub view_box: Cell<(u16, u16)>,
        /// Straight-sRGB stroke/fill color.
        pub color: Cell<[f32; 4]>,
        /// Whether `color` came from the author rather than inheritance, so an
        /// ambient-colour refresh does not stomp a deliberate tint.
        pub explicit_color: Cell<bool>,
        /// `true` → fill (honoring `even_odd`), `false` → stroke.
        pub filled: Cell<bool>,
        /// Even-odd fill rule when `filled`; else non-zero winding.
        pub even_odd: Cell<bool>,
    }

    impl Default for IdealystIcon {
        fn default() -> Self {
            Self {
                paths: RefCell::new(Vec::new()),
                view_box: Cell::new((24, 24)),
                color: Cell::new(DEFAULT_ICON_COLOR),
                explicit_color: Cell::new(false),
                filled: Cell::new(false),
                even_odd: Cell::new(false),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for IdealystIcon {
        const NAME: &'static str = "IdealystIcon";
        type Type = super::IdealystIcon;
        type ParentType = gtk4::Widget;
    }

    impl ObjectImpl for IdealystIcon {}

    impl WidgetImpl for IdealystIcon {
        // No `measure` override: leaf nodes are sized via `set_size_request`
        // in the layout pass, and `gtk_widget_measure` clamps min/natural
        // up to the request — so the parent's `size_allocate` sees the
        // Taffy size. See the module-level "Sizing note".

        fn snapshot(&self, snapshot: &gtk4::Snapshot) {
            let obj = self.obj();
            let w = obj.width() as f64;
            let h = obj.height() as f64;
            if w <= 0.0 || h <= 0.0 {
                return;
            }
            let (vw, vh) = self.view_box.get();
            let (vw, vh) = (vw.max(1) as f64, vh.max(1) as f64);

            // Uniform fit-scale (square glyph: smaller side wins), centered.
            let scale = (w / vw).min(h / vh);
            let ox = (w - vw * scale) / 2.0;
            let oy = (h - vh * scale) / 2.0;

            let builder = gsk::PathBuilder::new();
            let mut emitter = GskEmitter { builder };
            for d in self.paths.borrow().iter() {
                parse_svg_path(d, scale, scale, ox, oy, &mut emitter);
            }
            let path = emitter.builder.to_path();
            let rgba = color::to_gdk(self.color.get());

            if self.filled.get() {
                let rule = if self.even_odd.get() {
                    gsk::FillRule::EvenOdd
                } else {
                    gsk::FillRule::Winding
                };
                snapshot.append_fill(&path, rule, &rgba);
            } else {
                // Rounded caps + joins at the scaled Lucide stroke weight.
                let stroke = gsk::Stroke::new((BASE_STROKE_WIDTH as f64 * scale) as f32);
                stroke.set_line_cap(gsk::LineCap::Round);
                stroke.set_line_join(gsk::LineJoin::Round);
                snapshot.append_stroke(&path, &stroke, &rgba);
            }
        }
    }
}

glib::wrapper! {
    pub struct IdealystIcon(ObjectSubclass<imp::IdealystIcon>)
        @extends gtk4::Widget;
}

impl Default for IdealystIcon {
    fn default() -> Self {
        Self::new()
    }
}

impl IdealystIcon {
    /// Build an icon widget rendering `data`'s paths in `color` (opaque
    /// black when `None`).
    pub fn new_from_data(data: &IconData, color: [f32; 4]) -> Self {
        let obj: Self = glib::Object::builder().build();
        let imp = obj.imp();
        *imp.paths.borrow_mut() = data.paths.iter().map(|p| p.to_string()).collect();
        imp.view_box.set(data.view_box);
        imp.color.set(color);
        imp.filled.set(data.filled);
        imp.even_odd.set(data.fill_rule == FillRule::EvenOdd);
        obj
    }

    fn new() -> Self {
        glib::Object::builder().build()
    }

    /// Recolor + repaint (reactive `update_icon_color`).
    /// Whether an author set this icon's colour explicitly.
    ///
    /// An icon with no colour of its own follows the ambient text colour (the
    /// primitive's contract, and what web's `currentColor` does), so the
    /// backend must be able to tell "author said black" from "nobody said
    /// anything" — otherwise inheritance would stomp a deliberate colour.
    pub fn has_explicit_color(&self) -> bool {
        self.imp().explicit_color.get()
    }

    /// Set the colour an author asked for. Marks the icon as explicitly
    /// coloured, so ambient-colour refreshes leave it alone.
    pub fn set_explicit_color(&self, color: [f32; 4]) {
        self.imp().explicit_color.set(true);
        self.set_color(color);
    }

    /// The colour this icon strokes/fills with — what a parity read needs.
    pub fn color(&self) -> [f32; 4] {
        self.imp().color.get()
    }

    pub fn set_color(&self, color: [f32; 4]) {
        self.imp().color.set(color);
        self.queue_draw();
    }

    /// Swap the glyph geometry in place + repaint (reactive
    /// `update_icon_data`). Replaces paths / view-box / fill mode.
    pub fn set_data(&self, data: &IconData) {
        let imp = self.imp();
        *imp.paths.borrow_mut() = data.paths.iter().map(|p| p.to_string()).collect();
        imp.view_box.set(data.view_box);
        imp.filled.set(data.filled);
        imp.even_odd.set(data.fill_rule == FillRule::EvenOdd);
        self.queue_draw();
    }
}

#[cfg(test)]
mod tests {
    //! Pure-data parser tests — no GSK / GTK context needed (the sink is
    //! a plain logger), so they run under `cargo test -p backend-linux
    //! --lib` without a display.
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
        // Destination = coord·scale + offset. Relative deltas (lowercase)
        // are scale-only — the offset is baked into the pen's start.
        let mut e = LogEmitter::default();
        parse_svg_path("M 1 2 L 3 4", 2.0, 3.0, 10.0, 100.0, &mut e);
        // M(1·2+10, 2·3+100)=(12,106); L(3·2+10, 4·3+100)=(16,112).
        assert_eq!(e.ops, vec!["M(12,106)", "L(16,112)"]);
    }

    #[test]
    fn implicit_lineto_after_moveto() {
        // Trailing coordinate pairs after `M` are implicit `L` (SVG spec).
        let mut e = LogEmitter::default();
        parse_svg_path("M 0 0 1 1 2 2", 1.0, 1.0, 0.0, 0.0, &mut e);
        assert_eq!(e.ops, vec!["M(0,0)", "L(1,1)", "L(2,2)"]);
    }

    #[test]
    fn relative_hv_and_lineto_track_current_point() {
        // Mix of `l`, `h`, `v` — all relative to the running pen.
        let mut e = LogEmitter::default();
        parse_svg_path("M 10 10 l 5 0 v 5 h -5", 1.0, 1.0, 0.0, 0.0, &mut e);
        assert_eq!(
            e.ops,
            vec!["M(10,10)", "L(15,10)", "L(15,15)", "L(10,15)"]
        );
    }

    #[test]
    fn cubic_curve_emits_curve_to() {
        let mut e = LogEmitter::default();
        parse_svg_path("M 0 0 C 1 2 3 4 5 6", 1.0, 1.0, 0.0, 0.0, &mut e);
        assert_eq!(e.ops, vec!["M(0,0)", "C(1,2;3,4;5,6)"]);
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
    /// (0.5523·r for 90°), else rounded corners overshoot ("boxy" Lucide
    /// glyphs). Pins the trash-2 can corner: radius-2 arc (19,20)→(17,22).
    #[test]
    fn arc_quarter_circle_uses_correct_kappa_handles() {
        let mut e = PointEmitter::default();
        parse_svg_path("M19 6v14a2 2 0 0 1-2 2", 1.0, 1.0, 0.0, 0.0, &mut e);
        assert_eq!(e.curves.len(), 1, "the quarter arc is one cubic segment");
        let (c1x, c1y, c2x, c2y, ex, ey) = e.curves[0];
        let approx = |a: f64, b: f64| (a - b).abs() < 0.01;
        assert!(approx(c1x, 19.0) && approx(c1y, 21.105), "C1 was ({c1x},{c1y})");
        assert!(approx(c2x, 18.105) && approx(c2y, 22.0), "C2 was ({c2x},{c2y})");
        assert!(approx(ex, 17.0) && approx(ey, 22.0), "end was ({ex},{ey})");
    }

    /// Smooth-cubic `S` reflects the previous control point through the
    /// current pen — the Lucide "smooth continuation" many glyphs use.
    #[test]
    fn smooth_cubic_reflects_previous_control() {
        let mut e = PointEmitter::default();
        // After C … ctrl2=(3,4), pen=(5,6). S reflects → cp1 = 2·(5,6)-(3,4) = (7,8).
        parse_svg_path("M 0 0 C 1 2 3 4 5 6 S 8 9 10 11", 1.0, 1.0, 0.0, 0.0, &mut e);
        assert_eq!(e.curves.len(), 2);
        let (c1x, c1y, ..) = e.curves[1];
        assert!((c1x - 7.0).abs() < 1e-9 && (c1y - 8.0).abs() < 1e-9, "reflected cp1 = (7,8)");
    }
}
