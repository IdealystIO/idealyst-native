//! Pie and donut charts.
//!
//! One flat list of slices, no axes, no domain — see [`crate::polar`] for
//! why this is a separate spec rather than a `ChartSpec` variant.
//!
//! Everything that makes a donut good is here and nothing else is: an inner
//! radius, a start angle, a padding angle, and per-slice emphasis. A donut
//! with a value in the middle is the one genuinely defensible part-to-whole
//! chart, and it is what `center_label` exists for.

use std::f32::consts::TAU;
use std::rc::Rc;

use crate::hit::HitIndex;
use crate::polar::{fade, point_on, rad, wedge_path, PolarOutput, SliceHighlight, WEDGE_FILL_RULE};
use crate::scene::{
    pt, ChartScene, Color, HAlign, LabelPlacement, LabelRole, Layer, Mark, Paint, Path, Point, Rect,
    Stroke, VAlign,
};
use crate::spec::{datum, Emphasis};

/// One wedge.
#[derive(Clone, PartialEq, Debug)]
pub struct Slice {
    pub label: String,
    /// Non-positive values draw nothing. A pie asserts that its parts sum to
    /// a whole, and a negative part has no share of one — silently plotting
    /// its magnitude would be a lie about the data.
    pub value: f64,
    pub color: Color,
    /// Pull this slice out of the ring, as a fraction of the outer radius.
    /// The static form of emphasis: "this is the one we are talking about",
    /// independent of the pointer.
    pub explode: f32,
    /// Hidden slices keep their color slot and legend entry but draw
    /// nothing, and do not count toward the total — that is what a legend
    /// toggle needs.
    pub visible: bool,
}

impl Slice {
    pub fn new(label: impl Into<String>, value: f64, color: Color) -> Self {
        Self { label: label.into(), value, color, explode: 0.0, visible: true }
    }

    pub fn explode(mut self, fraction: f32) -> Self {
        self.explode = fraction;
        self
    }
}

/// Where slice labels go.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PieLabels {
    #[default]
    None,
    /// Centred in the slice. Cheapest, and unreadable on thin slices — this
    /// crate does not measure text, so it cannot drop the ones that will not
    /// fit. Use it when the slices are known to be chunky.
    Inside,
    /// Just outside the ring, radially.
    Outside,
    /// Outside, with a leader line from the slice's edge.
    Leader,
}

/// What a [`SliceStyleFn`] is told about the slice it is styling.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SliceContext<'a> {
    pub index: usize,
    pub label: &'a str,
    pub value: f64,
    /// Share of the total, 0..=1 — the number a conditional format usually
    /// wants ("grey out anything under 5%"), and awkward to recompute in a
    /// callback that only sees one slice.
    pub fraction: f64,
    pub emphasis: Emphasis,
    pub base_color: Color,
}

/// Per-slice overrides. Every field is optional.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct SliceOverride {
    pub color: Option<Color>,
    /// Multiplier on the resolved alpha, 0..=1.
    pub opacity: Option<f32>,
    /// Pull-out, as a fraction of the outer radius. Replaces
    /// [`Slice::explode`] for this slice.
    pub explode: Option<f32>,
}

impl SliceOverride {
    pub fn color(c: Color) -> Self {
        Self { color: Some(c), ..Default::default() }
    }

    pub fn opacity(o: f32) -> Self {
        Self { opacity: Some(o), ..Default::default() }
    }

    pub fn explode(f: f32) -> Self {
        Self { explode: Some(f), ..Default::default() }
    }
}

/// A callback that styles individual slices — conditional formatting.
///
/// Same identity rule as [`StyleFn`](crate::spec::StyleFn): [`PieSpec`] is
/// `PartialEq` so a host can memoise on it, closures have no structural
/// equality, so two specs' callbacks compare by `Rc::ptr_eq`. Build the `Rc`
/// once and clone it in; constructing a fresh closure inside the expression
/// that rebuilds the spec makes every spec compare unequal and re-renders
/// the chart on every reactive tick.
pub type SliceStyleFn = Rc<dyn Fn(&SliceContext<'_>) -> SliceOverride>;

/// How much of the available radius outside labels give up.
///
/// This crate deliberately does not measure text (see
/// [`LabelMetrics`](crate::render::LabelMetrics)), so the ring cannot be
/// fitted to the labels that will actually be drawn. A fixed reservation is
/// the honest alternative: wrong by a bounded amount, identical on every
/// machine, and overridable — an author who knows their labels sets
/// [`PieSpec::radius_fraction`] and gets exactly what they asked for.
const OUTSIDE_LABEL_RESERVE: f32 = 0.74;
/// Extra reservation for the leader line's elbow.
const LEADER_RESERVE: f32 = 0.66;
/// Radial length of a leader line's stub.
const LEADER_STUB: f32 = 10.0;
/// Horizontal length of a leader line's elbow.
const LEADER_ELBOW: f32 = 14.0;
/// Gap between the ring (or leader) and the label anchor.
const LABEL_GAP: f32 = 6.0;

/// A pie or donut chart.
#[derive(Clone)]
pub struct PieSpec {
    pub slices: Vec<Slice>,
    /// Inner radius as a fraction of the outer, 0..1. `0.0` is a pie;
    /// `0.6` is a donut.
    pub inner_radius: f32,
    /// Where the first slice begins, in degrees clockwise from twelve
    /// o'clock.
    pub start_angle: f32,
    /// Total angle the slices divide, in degrees. `360` is a full circle;
    /// `180` gives a semicircle, which is the shape a "share of budget"
    /// readout usually wants above a KPI.
    pub total_angle: f32,
    /// Gap between adjacent slices, in degrees. Applied by insetting each
    /// slice, so the boundaries stay where the data puts them.
    pub pad_angle: f32,
    /// Outer radius as a fraction of half the shorter side of the rect.
    pub radius_fraction: f32,
    pub labels: PieLabels,
    /// Text in the middle of a donut. Ignored geometrically — the host
    /// draws it and knows its own font — but emitted as a
    /// [`LabelRole::Title`] placement at the center.
    pub center_label: Option<String>,
    /// Second line under `center_label`, as a [`LabelRole::DataLabel`].
    pub center_sublabel: Option<String>,
    pub highlight: SliceHighlight,
    /// Pixels added to the outer radius of an emphasised slice.
    pub hover_grow: f32,
    /// Fraction of the radius an emphasised slice is pulled out by.
    pub hover_explode: f32,
    /// Optional per-slice styling. See [`SliceStyleFn`] — note the identity
    /// rule.
    pub style_fn: Option<SliceStyleFn>,
    /// Emit legend label placements.
    pub legend: bool,
}

impl Default for PieSpec {
    fn default() -> Self {
        Self {
            slices: Vec::new(),
            inner_radius: 0.0,
            start_angle: 0.0,
            total_angle: 360.0,
            pad_angle: 0.0,
            radius_fraction: 0.9,
            labels: PieLabels::None,
            center_label: None,
            center_sublabel: None,
            highlight: SliceHighlight::default(),
            hover_grow: 6.0,
            hover_explode: 0.0,
            style_fn: None,
            legend: false,
        }
    }
}

// Hand-written for the same reason `Series`' is: `style_fn` is a closure and
// has no structural equality. See [`SliceStyleFn`].
impl PartialEq for PieSpec {
    fn eq(&self, other: &Self) -> bool {
        self.slices == other.slices
            && self.inner_radius == other.inner_radius
            && self.start_angle == other.start_angle
            && self.total_angle == other.total_angle
            && self.pad_angle == other.pad_angle
            && self.radius_fraction == other.radius_fraction
            && self.labels == other.labels
            && self.center_label == other.center_label
            && self.center_sublabel == other.center_sublabel
            && self.highlight == other.highlight
            && self.hover_grow == other.hover_grow
            && self.hover_explode == other.hover_explode
            && self.legend == other.legend
            && match (&self.style_fn, &other.style_fn) {
                (None, None) => true,
                (Some(a), Some(b)) => Rc::ptr_eq(a, b),
                _ => false,
            }
    }
}

impl std::fmt::Debug for PieSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PieSpec")
            .field("slices", &self.slices)
            .field("inner_radius", &self.inner_radius)
            .field("start_angle", &self.start_angle)
            .field("total_angle", &self.total_angle)
            .field("pad_angle", &self.pad_angle)
            .field("radius_fraction", &self.radius_fraction)
            .field("labels", &self.labels)
            .field("center_label", &self.center_label)
            .field("center_sublabel", &self.center_sublabel)
            .field("highlight", &self.highlight)
            .field("legend", &self.legend)
            .field("style_fn", &self.style_fn.is_some())
            .finish()
    }
}

impl PieSpec {
    pub fn new(slices: Vec<Slice>) -> Self {
        Self { slices, ..Default::default() }
    }

    /// A donut: `inner` is the hole as a fraction of the outer radius.
    pub fn donut(slices: Vec<Slice>, inner: f32) -> Self {
        Self { slices, inner_radius: inner, ..Default::default() }
    }

    pub fn inner_radius(mut self, f: f32) -> Self {
        self.inner_radius = f;
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

    pub fn pad_angle(mut self, degrees: f32) -> Self {
        self.pad_angle = degrees;
        self
    }

    pub fn radius_fraction(mut self, f: f32) -> Self {
        self.radius_fraction = f;
        self
    }

    pub fn labels(mut self, l: PieLabels) -> Self {
        self.labels = l;
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

    pub fn highlight(mut self, h: SliceHighlight) -> Self {
        self.highlight = h;
        self
    }

    pub fn hover_grow(mut self, px: f32) -> Self {
        self.hover_grow = px;
        self
    }

    pub fn hover_explode(mut self, fraction: f32) -> Self {
        self.hover_explode = fraction;
        self
    }

    pub fn legend(mut self, on: bool) -> Self {
        self.legend = on;
        self
    }

    /// Attach per-slice styling. Hoist the `Rc` — see [`SliceStyleFn`].
    pub fn styled(mut self, f: SliceStyleFn) -> Self {
        self.style_fn = Some(f);
        self
    }

    /// Sum of the visible, positive slice values — the denominator every
    /// share is measured against.
    pub fn total(&self) -> f64 {
        self.slices.iter().filter(|s| s.visible && s.value > 0.0).map(|s| s.value).sum()
    }

    fn override_for(&self, ctx: &SliceContext<'_>) -> SliceOverride {
        match &self.style_fn {
            Some(f) => f(ctx),
            None => SliceOverride::default(),
        }
    }
}

/// Render a pie or donut into `rect`.
///
/// The ring is centred in the rect and sized from its shorter side, so a
/// chart in a wide container stays circular instead of stretching — an
/// ellipse would misrepresent every share, since equal angles would no
/// longer subtend equal area.
pub fn render_pie(spec: &PieSpec, rect: Rect) -> PolarOutput {
    let center = pt(rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
    let reserve = match spec.labels {
        PieLabels::None | PieLabels::Inside => 1.0,
        PieLabels::Outside => OUTSIDE_LABEL_RESERVE,
        PieLabels::Leader => LEADER_RESERVE,
    };
    let radius = (rect.w.min(rect.h) / 2.0) * spec.radius_fraction.clamp(0.0, 1.0) * reserve;

    let mut scene = ChartScene { marks: Vec::new(), labels: Vec::new(), plot: rect };
    let mut hit = HitIndex::new(rect);

    // Same early-out as the cartesian renderer, for the same reason: a host
    // measures its plot after first mount, so every chart passes through a
    // zero-size frame, and the marks it would emit there are degenerate.
    let total = spec.total();
    if radius <= 0.0 || total <= 0.0 {
        return PolarOutput { scene, hit, center, radius: radius.max(0.0) };
    }

    let total_sweep = rad(spec.total_angle.clamp(-360.0, 360.0));
    let pad = rad(spec.pad_angle.max(0.0));
    let r_inner = radius * spec.inner_radius.clamp(0.0, 0.95);
    let hl = &spec.highlight;

    let mut cursor = rad(spec.start_angle);
    for (i, s) in spec.slices.iter().enumerate() {
        if !s.visible || s.value <= 0.0 {
            continue;
        }
        let fraction = s.value / total;
        let full = total_sweep * fraction as f32;
        cursor += full;
        let start_of_slice = cursor - full;

        // Padding insets the slice rather than moving it, so the boundary
        // between two shares stays exactly where the data puts it. A slice
        // thinner than the padding keeps its full sweep — shrinking it to
        // nothing would make small shares disappear, which is the opposite
        // of what a chart is for.
        let (a0, sweep) = if pad > 0.0 && full.abs() > pad {
            (start_of_slice + pad / 2.0, full - pad)
        } else {
            (start_of_slice, full)
        };
        let mid = a0 + sweep / 2.0;

        let emphasis = hl.of(i);
        let dim = hl.dim_for(i);
        let base = fade(s.color, dim);
        let ov = spec.override_for(&SliceContext {
            index: i,
            label: &s.label,
            value: s.value,
            fraction,
            emphasis,
            base_color: base,
        });
        let color = {
            let c = ov.color.unwrap_or(base);
            match ov.opacity {
                Some(o) => fade(c, o),
                None => c,
            }
        };

        let grow = if emphasis == Emphasis::None { 0.0 } else { spec.hover_grow };
        let explode_fraction = ov.explode.unwrap_or_else(|| {
            s.explode + if emphasis == Emphasis::None { 0.0 } else { spec.hover_explode }
        });
        let offset = radius * explode_fraction;
        let c = point_on(center, offset, mid);
        let r_outer = radius + grow;

        scene.push(Mark::Fill {
            layer: Layer::Series,
            path: wedge_path(c, r_inner, r_outer, a0, sweep),
            paint: Paint::solid(color),
            rule: WEDGE_FILL_RULE,
        });

        let anchor = point_on(c, (r_inner + r_outer) / 2.0, mid);
        hit.push_wedge(c, r_inner, r_outer, a0, sweep, anchor, 0, i, datum(i as f64, s.value));

        push_slice_label(&mut scene, spec, s, c, r_inner, r_outer, mid, color);
    }

    push_center_labels(&mut scene, spec, center);
    if spec.legend {
        push_legend(&mut scene, spec, rect);
    }
    scene.sort_layers();

    PolarOutput { scene, hit, center, radius }
}

/// Place one slice's label, and its leader line when it has one.
#[allow(clippy::too_many_arguments)]
fn push_slice_label(
    scene: &mut ChartScene,
    spec: &PieSpec,
    slice: &Slice,
    center: Point,
    r_inner: f32,
    r_outer: f32,
    mid: f32,
    color: Color,
) {
    // Which side of the ring the slice sits on decides which way its label
    // reads away from the chart. `sin(mid) >= 0` is the right half, given
    // the clockwise-from-twelve convention.
    let right_half = mid.sin() >= 0.0;
    match spec.labels {
        PieLabels::None => {}
        PieLabels::Inside => {
            let at = point_on(center, (r_inner + r_outer) / 2.0, mid);
            scene.label(LabelPlacement {
                text: slice.label.clone(),
                anchor: at,
                h_align: HAlign::Center,
                v_align: VAlign::Middle,
                role: LabelRole::DataLabel,
                rotation: 0.0,
                color: None,
            });
        }
        PieLabels::Outside => {
            let at = point_on(center, r_outer + LABEL_GAP, mid);
            scene.label(LabelPlacement {
                text: slice.label.clone(),
                anchor: at,
                h_align: if right_half { HAlign::Left } else { HAlign::Right },
                v_align: VAlign::Middle,
                role: LabelRole::DataLabel,
                rotation: 0.0,
                color: Some(color),
            });
        }
        PieLabels::Leader => {
            let knee = point_on(center, r_outer + LEADER_STUB, mid);
            let elbow_x = knee.x + if right_half { LEADER_ELBOW } else { -LEADER_ELBOW };
            scene.push(Mark::Stroke {
                layer: Layer::Axis,
                path: Path::new()
                    .move_to(
                        point_on(center, r_outer, mid).x,
                        point_on(center, r_outer, mid).y,
                    )
                    .line_to(knee.x, knee.y)
                    .line_to(elbow_x, knee.y),
                stroke: Stroke::width(1.0),
                paint: Paint::solid(color),
            });
            scene.label(LabelPlacement {
                text: slice.label.clone(),
                anchor: pt(
                    elbow_x + if right_half { LABEL_GAP } else { -LABEL_GAP },
                    knee.y,
                ),
                h_align: if right_half { HAlign::Left } else { HAlign::Right },
                v_align: VAlign::Middle,
                role: LabelRole::DataLabel,
                rotation: 0.0,
                color: Some(color),
            });
        }
    }
}

fn push_center_labels(scene: &mut ChartScene, spec: &PieSpec, center: Point) {
    // Two lines are stacked around the center rather than below it, so the
    // pair reads as centred in the hole. The host owns the actual line
    // height; these offsets only say "one above, one below".
    let two = spec.center_label.is_some() && spec.center_sublabel.is_some();
    if let Some(text) = &spec.center_label {
        scene.label(LabelPlacement {
            text: text.clone(),
            anchor: if two { pt(center.x, center.y - LABEL_GAP) } else { center },
            h_align: HAlign::Center,
            v_align: if two { VAlign::Bottom } else { VAlign::Middle },
            role: LabelRole::Title,
            rotation: 0.0,
            color: None,
        });
    }
    if let Some(text) = &spec.center_sublabel {
        scene.label(LabelPlacement {
            text: text.clone(),
            anchor: if two { pt(center.x, center.y + LABEL_GAP) } else { center },
            h_align: HAlign::Center,
            v_align: if two { VAlign::Top } else { VAlign::Middle },
            role: LabelRole::DataLabel,
            rotation: 0.0,
            color: None,
        });
    }
}

/// Legend placements. Hidden slices keep their entry — that is what makes a
/// legend toggle reversible — but zero-value ones do too, since a share that
/// has fallen to nothing is information.
fn push_legend(scene: &mut ChartScene, spec: &PieSpec, rect: Rect) {
    for (i, s) in spec.slices.iter().enumerate() {
        scene.label(LabelPlacement {
            text: s.label.clone(),
            anchor: pt(rect.x + i as f32 * 100.0, rect.y),
            h_align: HAlign::Left,
            v_align: VAlign::Top,
            role: LabelRole::Legend,
            rotation: 0.0,
            color: Some(s.color),
        });
    }
}

/// Interpolate two pies, for a transition.
///
/// Values only, exactly as [`lerp_data`](crate::tween::lerp_data) does for a
/// cartesian spec: colors, labels, and highlight come from `to`, so a slice
/// changing color or becoming selected takes effect at once rather than
/// fading through an intermediate nobody asked for.
///
/// Returns `None` when the slice counts differ — pairing slice 3 of one pie
/// with slice 3 of an unrelated one animates a share into a value it has
/// nothing to do with, which reads worse than not animating.
pub fn lerp_pie(from: &PieSpec, to: &PieSpec, t: f32) -> Option<PieSpec> {
    if from.slices.len() != to.slices.len() {
        return None;
    }
    let e = crate::tween::ease_in_out(t);
    let mut out = to.clone();
    for (i, s) in out.slices.iter_mut().enumerate() {
        s.value = crate::tween::lerp_f64(from.slices[i].value, s.value, e);
    }
    Some(out)
}

/// Render a frame of a transition between two pies.
///
/// Falls back to `to` when the two cannot be paired. Unlike the cartesian
/// tween there is no domain to interpolate — a pie's geometry comes entirely
/// from the values, so interpolating those is the whole job.
pub fn render_pie_tween(from: &PieSpec, to: &PieSpec, t: f32, rect: Rect) -> PolarOutput {
    match lerp_pie(from, to, t) {
        Some(spec) => render_pie(&spec, rect),
        None => render_pie(to, rect),
    }
}

/// The angle, clockwise from twelve o'clock in radians, that a full turn
/// represents. Exposed for hosts doing their own polar math.
pub const FULL_TURN: f32 = TAU;
