//! Radial bar charts and gauges.
//!
//! Concentric arcs, one per value, each sweeping a share of a common angular
//! range. A gauge is the one-bar case — same geometry, same code path, no
//! separate type.
//!
//! # Why these are strokes, not wedges
//!
//! A ring of constant thickness IS a stroked circular arc. Drawing it as a
//! filled wedge would mean building a four-sided contour per bar and
//! hand-rolling the end caps; as a stroke it is one arc path, `width =
//! thickness`, and `LineCap::Round` gives the rounded ends for free — the
//! same caps every renderer already implements correctly. The hit index
//! still stores a wedge, because that is the region the stroke covers.

use crate::hit::HitIndex;
use crate::polar::{arc_to, fade, point_on, rad, PolarOutput, SliceHighlight};
use crate::scene::{
    pt, ChartScene, Color, HAlign, LabelPlacement, LabelRole, Layer, LineCap, LineJoin, Mark, Paint,
    Path, Point, Rect, Stroke, VAlign,
};
use crate::spec::{datum, Emphasis};

/// One ring.
#[derive(Clone, PartialEq, Debug)]
pub struct RadialBar {
    pub label: String,
    pub value: f64,
    pub color: Color,
    pub visible: bool,
}

impl RadialBar {
    pub fn new(label: impl Into<String>, value: f64, color: Color) -> Self {
        Self { label: label.into(), value, color, visible: true }
    }
}

/// Gap between a ring and its label.
const LABEL_GAP: f32 = 8.0;

/// A radial bar chart, or a gauge.
#[derive(Clone, PartialEq, Debug)]
pub struct RadialSpec {
    /// Outermost first. That is the convention every radial bar chart uses,
    /// and it matches reading order: the first row of a table becomes the
    /// outer ring.
    pub bars: Vec<RadialBar>,
    /// The value range every bar's sweep is measured against.
    ///
    /// Explicit, never inferred from the data. Rings only mean anything if
    /// they share a scale, and an auto-fitted max would silently make the
    /// largest value a full circle on every chart — so 30% and 95% would
    /// look identical whenever they happened to be the maximum.
    pub min: f64,
    pub max: f64,
    /// Where every bar begins, in degrees clockwise from twelve o'clock.
    pub start_angle: f32,
    /// Angular range a full-valued bar covers, in degrees. `360` is a ring;
    /// `270` with `start_angle: -135` is the classic gauge.
    pub total_angle: f32,
    /// Ring thickness in pixels.
    pub thickness: f32,
    /// Gap between adjacent rings in pixels.
    pub gap: f32,
    /// Outer radius as a fraction of half the shorter side of the rect.
    pub radius_fraction: f32,
    /// The unfilled remainder drawn behind each bar. `None` draws none.
    ///
    /// Worth having on by default: without it a short arc floating in space
    /// gives no sense of what it is short OF, which is the entire question a
    /// gauge is asked.
    pub track: Option<Color>,
    /// Round the arc ends.
    pub rounded: bool,
    /// Text in the middle, as a [`LabelRole::Title`] placement.
    pub center_label: Option<String>,
    /// Second line, as a [`LabelRole::DataLabel`].
    pub center_sublabel: Option<String>,
    /// Emit a label at each ring's start.
    pub labels: bool,
    pub highlight: SliceHighlight,
    /// Pixels added to an emphasised ring's thickness.
    pub hover_grow: f32,
    pub legend: bool,
}

impl Default for RadialSpec {
    fn default() -> Self {
        Self {
            bars: Vec::new(),
            min: 0.0,
            max: 100.0,
            start_angle: 0.0,
            total_angle: 360.0,
            thickness: 14.0,
            gap: 6.0,
            radius_fraction: 0.9,
            track: Some(Color::rgba(128, 128, 128, 38)),
            rounded: true,
            center_label: None,
            center_sublabel: None,
            labels: false,
            highlight: SliceHighlight::default(),
            hover_grow: 4.0,
            legend: false,
        }
    }
}

impl RadialSpec {
    pub fn new(bars: Vec<RadialBar>) -> Self {
        Self { bars, ..Default::default() }
    }

    /// A single-value gauge: a 270° arc opening at the bottom, which is
    /// where a dial's needle would never point and therefore the
    /// conventional place to leave the gap.
    pub fn gauge(label: impl Into<String>, value: f64, max: f64, color: Color) -> Self {
        Self {
            bars: vec![RadialBar::new(label, value, color)],
            min: 0.0,
            max,
            start_angle: -135.0,
            total_angle: 270.0,
            thickness: 18.0,
            ..Default::default()
        }
    }

    pub fn range(mut self, min: f64, max: f64) -> Self {
        self.min = min;
        self.max = max;
        self
    }

    pub fn start_angle(mut self, degrees: f32) -> Self {
        self.start_angle = degrees;
        self
    }

    pub fn total_angle(mut self, degrees: f32) -> Self {
        self.total_angle = degrees;
        self
    }

    pub fn thickness(mut self, px: f32) -> Self {
        self.thickness = px;
        self
    }

    pub fn gap(mut self, px: f32) -> Self {
        self.gap = px;
        self
    }

    pub fn radius_fraction(mut self, f: f32) -> Self {
        self.radius_fraction = f;
        self
    }

    pub fn track(mut self, color: Option<Color>) -> Self {
        self.track = color;
        self
    }

    pub fn rounded(mut self, on: bool) -> Self {
        self.rounded = on;
        self
    }

    pub fn center(mut self, label: impl Into<String>) -> Self {
        self.center_label = Some(label.into());
        self
    }

    pub fn center_sub(mut self, label: impl Into<String>) -> Self {
        self.center_sublabel = Some(label.into());
        self
    }

    pub fn labels(mut self, on: bool) -> Self {
        self.labels = on;
        self
    }

    pub fn highlight(mut self, h: SliceHighlight) -> Self {
        self.highlight = h;
        self
    }

    pub fn legend(mut self, on: bool) -> Self {
        self.legend = on;
        self
    }

    /// A bar's share of the range, clamped to 0..=1.
    ///
    /// Clamped rather than allowed to overshoot: an arc past a full turn
    /// wraps and starts overwriting itself, so a value above `max` would
    /// read as a SMALLER one. Clamping at least reads as "at or beyond the
    /// top", which is true.
    pub fn fraction(&self, value: f64) -> f32 {
        let span = self.max - self.min;
        if span.abs() < f64::EPSILON {
            return 0.0;
        }
        (((value - self.min) / span) as f32).clamp(0.0, 1.0)
    }
}

/// Render a radial bar chart or gauge into `rect`.
pub fn render_radial(spec: &RadialSpec, rect: Rect) -> PolarOutput {
    let center = pt(rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
    let radius = (rect.w.min(rect.h) / 2.0) * spec.radius_fraction.clamp(0.0, 1.0);

    let mut scene = ChartScene { marks: Vec::new(), labels: Vec::new(), plot: rect };
    let mut hit = HitIndex::new(rect);
    if radius <= 0.0 {
        return PolarOutput { scene, hit, center, radius: 0.0 };
    }

    let start = rad(spec.start_angle);
    let full = rad(spec.total_angle.clamp(-360.0, 360.0));
    let cap = if spec.rounded { LineCap::Round } else { LineCap::Butt };
    let hl = &spec.highlight;

    for (i, b) in spec.bars.iter().enumerate() {
        if !b.visible {
            continue;
        }
        let emphasis = hl.of(i);
        let dim = hl.dim_for(i);
        let grow = if emphasis == Emphasis::None { 0.0 } else { spec.hover_grow };
        let thickness = (spec.thickness + grow).max(0.0);

        // Rings are laid out from the outside in on the UNGROWN thickness,
        // so an emphasised ring thickens in place instead of shoving every
        // ring inside it inward — a hover that reflows the whole chart reads
        // as a glitch.
        let r_outer = radius - i as f32 * (spec.thickness + spec.gap);
        let rc = r_outer - spec.thickness / 2.0;
        if rc <= 0.0 {
            // Out of room. Stop rather than drawing degenerate inner rings
            // on top of each other.
            break;
        }

        if let Some(track) = spec.track {
            scene.push(Mark::Stroke {
                layer: Layer::Grid,
                path: arc_path(center, rc, start, full),
                stroke: stroke_of(thickness, cap),
                paint: Paint::solid(fade(track, dim)),
            });
        }

        let sweep = full * spec.fraction(b.value);
        if sweep.abs() > f32::EPSILON {
            scene.push(Mark::Stroke {
                layer: Layer::Series,
                path: arc_path(center, rc, start, sweep),
                stroke: stroke_of(thickness, cap),
                paint: Paint::solid(fade(b.color, dim)),
            });
        }

        // Hit the whole TRACK, not just the filled arc. The question a
        // pointer over a ring is asking is "what is this ring's value", and
        // it is asking it just as much from the empty part — the same
        // reasoning as the cartesian highlight band covering a whole slot.
        let anchor = point_on(center, rc, start + sweep);
        hit.push_wedge(
            center,
            rc - thickness / 2.0,
            rc + thickness / 2.0,
            start,
            full,
            anchor,
            0,
            i,
            datum(i as f64, b.value),
        );

        if spec.labels {
            // Just before the ring's start, reading away from it. The gap is
            // expressed in pixels and converted to an angle here, so every
            // ring's label sits the same distance from its arc rather than
            // the same number of degrees — which would put the inner ones
            // visibly closer.
            //
            // Half the thickness is part of that gap because a round cap
            // extends the arc backwards past `start` by exactly that much:
            // measuring from the nominal start angle alone puts the label
            // underneath the cap, which is where it was until this line.
            let cap = if spec.rounded { thickness / 2.0 } else { 0.0 };
            let dtheta = (LABEL_GAP + cap) / rc;
            let at = point_on(center, rc, start - dtheta);
            scene.label(LabelPlacement {
                text: b.label.clone(),
                anchor: at,
                h_align: HAlign::Right,
                v_align: VAlign::Middle,
                role: LabelRole::DataLabel,
                rotation: 0.0,
                color: Some(fade(b.color, dim)),
            });
        }
    }

    if let Some(text) = &spec.center_label {
        let two = spec.center_sublabel.is_some();
        scene.label(LabelPlacement {
            text: text.clone(),
            anchor: if two { pt(center.x, center.y - LABEL_GAP / 2.0) } else { center },
            h_align: HAlign::Center,
            v_align: if two { VAlign::Bottom } else { VAlign::Middle },
            role: LabelRole::Title,
            rotation: 0.0,
            color: None,
        });
    }
    if let Some(text) = &spec.center_sublabel {
        let two = spec.center_label.is_some();
        scene.label(LabelPlacement {
            text: text.clone(),
            anchor: if two { pt(center.x, center.y + LABEL_GAP / 2.0) } else { center },
            h_align: HAlign::Center,
            v_align: if two { VAlign::Top } else { VAlign::Middle },
            role: LabelRole::DataLabel,
            rotation: 0.0,
            color: None,
        });
    }
    if spec.legend {
        for (i, b) in spec.bars.iter().enumerate() {
            scene.label(LabelPlacement {
                text: b.label.clone(),
                anchor: pt(rect.x + i as f32 * 100.0, rect.y),
                h_align: HAlign::Left,
                v_align: VAlign::Top,
                role: LabelRole::Legend,
                rotation: 0.0,
                color: Some(b.color),
            });
        }
    }

    scene.sort_layers();
    PolarOutput { scene, hit, center, radius }
}

fn stroke_of(width: f32, cap: LineCap) -> Stroke {
    Stroke { width, cap, join: LineJoin::Round, dash: Vec::new(), dash_offset: 0.0 }
}

/// A bare arc centerline, to be stroked.
fn arc_path(center: Point, r: f32, start: f32, sweep: f32) -> Path {
    let p0 = point_on(center, r, start);
    arc_to(Path::new().move_to(p0.x, p0.y), center, r, start, sweep)
}

/// Interpolate two radial specs, for a transition. Values only — see
/// [`lerp_pie`](crate::pie::lerp_pie).
pub fn lerp_radial(from: &RadialSpec, to: &RadialSpec, t: f32) -> Option<RadialSpec> {
    if from.bars.len() != to.bars.len() {
        return None;
    }
    let e = crate::tween::ease_in_out(t);
    let mut out = to.clone();
    for (i, b) in out.bars.iter_mut().enumerate() {
        b.value = crate::tween::lerp_f64(from.bars[i].value, b.value, e);
    }
    // The range is interpolated too, unlike a pie's (which has none): a
    // gauge whose max changes would otherwise snap its whole scale on the
    // first frame while the arc glides, exactly the glitch `render_tween`
    // interpolates the cartesian domain to avoid.
    out.min = crate::tween::lerp_f64(from.min, to.min, e);
    out.max = crate::tween::lerp_f64(from.max, to.max, e);
    Some(out)
}

/// Render a frame of a transition between two radial specs.
pub fn render_radial_tween(
    from: &RadialSpec,
    to: &RadialSpec,
    t: f32,
    rect: Rect,
) -> PolarOutput {
    match lerp_radial(from, to, t) {
        Some(spec) => render_radial(&spec, rect),
        None => render_radial(to, rect),
    }
}
