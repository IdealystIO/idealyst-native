//! Turning a [`ChartSpec`] into a [`ChartScene`].
//!
//! Pure: same spec and rect in, byte-identical scene out. No interior
//! mutability, no globals, no clock. That is what lets the tests be exact
//! `==` goldens, and what lets a host re-render freely on every signal
//! change without worrying about accumulated state.

use crate::hit::HitIndex;
use crate::scale::{self, ResolvedAxis};
use crate::scene::{
    pt, ChartScene, Color, FillRule, HAlign, LabelPlacement, LabelRole, Layer, LineCap, LineJoin,
    Mark, Paint, Path, Point, PointInstance, Rect, Stroke, VAlign,
};
use crate::spec::{BarLayout, ChartSpec, Datum, SeriesKind};

/// Space reserved around the data area for axis furniture.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Padding {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Padding {
    pub const fn all(v: f32) -> Self {
        Self { left: v, top: v, right: v, bottom: v }
    }

    pub const fn new(left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self { left, top, right, bottom }
    }
}

/// Text measurement, for hosts that draw their own labels.
///
/// Optional by design. The idealyst SDK never implements this: it renders
/// axis labels as real text nodes inside flex-laid-out gutters, so the
/// framework's own layout sizes the gutter to the widest label and no
/// measurement is needed here at all. Only hosts that rasterize text
/// themselves — the bundled SVG renderer, or a standalone raster consumer —
/// need to supply it, and then only to compute gutter widths.
///
/// This is the inversion plotters could not do: there, measurement is a
/// required method on the drawing backend, so *every* consumer owns a font
/// stack whether or not it has one.
pub trait LabelMetrics {
    /// Rendered size of `text` in pixels, for a label of the given role.
    fn measure(&self, text: &str, role: LabelRole) -> (f32, f32);
}

/// How the data area is derived from the rect handed to the renderer.
pub enum Gutters<'a> {
    /// The rect IS the data area; the host positions labels itself. This is
    /// the idealyst path — the plot rect arrives from the framework's
    /// layout, already excluding the gutters it laid out.
    None,
    /// Inset the rect by fixed amounts.
    Fixed(Padding),
    /// Inset by enough to fit the measured labels.
    Measured(&'a dyn LabelMetrics),
}

// Hand-written: `LabelMetrics` is a caller-supplied trait object and
// requiring `Debug` on it would push that bound onto every host for the
// sake of this one impl.
impl std::fmt::Debug for Gutters<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Gutters::None => f.write_str("Gutters::None"),
            Gutters::Fixed(p) => f.debug_tuple("Gutters::Fixed").field(p).finish(),
            Gutters::Measured(_) => f.write_str("Gutters::Measured(..)"),
        }
    }
}

/// The result of a render.
#[derive(Clone, PartialEq, Debug)]
pub struct ChartOutput {
    pub scene: ChartScene,
    /// The resolved x axis — exposed so a pan/zoom addon can read the
    /// current window via [`ResolvedAxis::domain`] and write a transformed
    /// one back onto the spec.
    pub x: ResolvedAxis,
    pub y: ResolvedAxis,
    pub hit: HitIndex,
}

/// Render with no gutters: the rect given is the data area.
pub fn render(spec: &ChartSpec, rect: Rect) -> ChartOutput {
    render_with(spec, rect, &Gutters::None)
}

/// Gap in pixels between an axis and its labels.
const LABEL_GAP: f32 = 8.0;
/// Length of a tick mark drawn outside the plot area.
const TICK_LEN: f32 = 4.0;

/// Render into `rect`, deriving the data area per `gutters`.
pub fn render_with(spec: &ChartSpec, rect: Rect, gutters: &Gutters<'_>) -> ChartOutput {
    // Resolve axes against the FULL rect first. Gutter width depends on the
    // tick labels, which depend on the domain, which does not depend on the
    // plot rect — so this ordering is well-founded and needs no iteration.
    let x = scale::resolve(&spec.x, x_values(spec).into_iter());
    let y = scale::resolve(&spec.y, y_values(spec).into_iter());

    let pad = match gutters {
        Gutters::None => Padding::default(),
        Gutters::Fixed(p) => *p,
        Gutters::Measured(m) => measured_padding(spec, &x, &y, *m),
    };
    let plot = rect.inset(pad.left, pad.top, pad.right, pad.bottom);

    let mut scene = ChartScene { marks: Vec::new(), labels: Vec::new(), plot };
    let mut hit = HitIndex::new(plot);

    // A plot with no area cannot show anything, and every mark it would
    // emit collapses onto a degenerate line. Return early rather than
    // producing that geometry: hosts measure their plot AFTER first mount,
    // so this is the state every chart passes through on frame one, and
    // rendering a full scene there is pure waste — plus the marks are
    // meaningless, which makes them a bad thing to hand a GPU renderer.
    // The axes are still resolved, so a caller can read the domain before
    // the first layout.
    if plot.w <= 0.0 || plot.h <= 0.0 {
        return ChartOutput { scene, x, y, hit };
    }

    draw_grid(&mut scene, spec, &x, &y, plot);
    draw_axis_labels(&mut scene, spec, &x, &y, plot);
    draw_series(&mut scene, &mut hit, spec, &x, &y, plot);
    if spec.legend {
        draw_legend(&mut scene, spec, plot);
    }
    scene.sort_layers();

    ChartOutput { scene, x, y, hit }
}

// ---------------------------------------------------------------------------
// Domain inputs
// ---------------------------------------------------------------------------

fn x_values(spec: &ChartSpec) -> Vec<f64> {
    spec.visible().flat_map(|(_, s)| s.data.iter().map(|d| d.x)).collect()
}

/// Y values that must be inside the domain.
///
/// Stacked bars are the subtle case: the domain has to cover the stack
/// TOTAL, not the tallest individual segment, or the top of the stack is
/// clipped. Positive and negative parts accumulate separately so a series
/// with mixed signs still fits.
fn y_values(spec: &ChartSpec) -> Vec<f64> {
    if spec.bar_layout == BarLayout::Stacked && spec.has_bars() {
        let mut totals: Vec<(f64, f64, f64)> = Vec::new(); // (x, pos_sum, neg_sum)
        for (_, s) in spec.visible().filter(|(_, s)| matches!(s.kind, SeriesKind::Bar { .. })) {
            for d in &s.data {
                let slot = match totals.iter_mut().find(|(x, _, _)| *x == d.x) {
                    Some(t) => t,
                    None => {
                        totals.push((d.x, 0.0, 0.0));
                        totals.last_mut().expect("just pushed")
                    }
                };
                if d.y >= 0.0 {
                    slot.1 += d.y;
                } else {
                    slot.2 += d.y;
                }
            }
        }
        let mut out: Vec<f64> = totals.iter().flat_map(|(_, p, n)| [*p, *n]).collect();
        // Non-bar series in a stacked chart still need to fit.
        out.extend(
            spec.visible()
                .filter(|(_, s)| !matches!(s.kind, SeriesKind::Bar { .. }))
                .flat_map(|(_, s)| s.data.iter().map(|d| d.y)),
        );
        return out;
    }
    spec.visible().flat_map(|(_, s)| s.data.iter().map(|d| d.y)).collect()
}

fn measured_padding(
    spec: &ChartSpec,
    x: &ResolvedAxis,
    y: &ResolvedAxis,
    m: &dyn LabelMetrics,
) -> Padding {
    let widest_y = y
        .ticks
        .iter()
        .map(|t| m.measure(&t.label, LabelRole::AxisY).0)
        .fold(0.0f32, f32::max);
    let tallest_x = x
        .ticks
        .iter()
        .map(|t| m.measure(&t.label, LabelRole::AxisX).1)
        .fold(0.0f32, f32::max);

    let mut pad = Padding {
        left: widest_y + LABEL_GAP + TICK_LEN,
        top: 8.0,
        // Half the last x label can overhang the right edge; reserve it so
        // it is not clipped by the surface bounds.
        right: x
            .ticks
            .last()
            .map(|t| m.measure(&t.label, LabelRole::AxisX).0 / 2.0)
            .unwrap_or(0.0)
            .max(8.0),
        bottom: tallest_x + LABEL_GAP + TICK_LEN,
    };
    if spec.x.title.is_some() {
        pad.bottom += tallest_x + LABEL_GAP;
    }
    if spec.y.title.is_some() {
        // The y title is rotated, so it costs its HEIGHT horizontally.
        pad.left += m.measure("X", LabelRole::AxisTitleY).1 + LABEL_GAP;
    }
    if spec.legend {
        pad.top += m.measure("X", LabelRole::Legend).1 + LABEL_GAP;
    }
    pad
}

// ---------------------------------------------------------------------------
// Axis furniture
// ---------------------------------------------------------------------------

fn draw_grid(
    scene: &mut ChartScene,
    spec: &ChartSpec,
    x: &ResolvedAxis,
    y: &ResolvedAxis,
    plot: Rect,
) {
    let grid = Stroke::width(1.0);

    if spec.y.grid {
        for t in &y.ticks {
            let py = y.map(t.value, plot.bottom(), plot.y);
            if py < plot.y - 0.5 || py > plot.bottom() + 0.5 {
                continue;
            }
            scene.push(Mark::Stroke {
                layer: Layer::Grid,
                path: Path::new().move_to(plot.x, py).line_to(plot.right(), py),
                stroke: grid.clone(),
                paint: Paint::solid(spec.grid_color),
            });
        }
    }
    if spec.x.grid {
        for t in &x.ticks {
            let px = x.map(t.value, plot.x, plot.right());
            if px < plot.x - 0.5 || px > plot.right() + 0.5 {
                continue;
            }
            scene.push(Mark::Stroke {
                layer: Layer::Grid,
                path: Path::new().move_to(px, plot.y).line_to(px, plot.bottom()),
                stroke: grid.clone(),
                paint: Paint::solid(spec.grid_color),
            });
        }
    }

    // The zero rule reads as part of the data, not the grid: it is the line
    // bars grow from, so it gets the stronger axis color.
    if y.min < 0.0 && y.max > 0.0 {
        let zero = y.map(0.0, plot.bottom(), plot.y);
        scene.push(Mark::Stroke {
            layer: Layer::Axis,
            path: Path::new().move_to(plot.x, zero).line_to(plot.right(), zero),
            stroke: Stroke::width(1.0),
            paint: Paint::solid(spec.axis_color),
        });
    }
}

fn draw_axis_labels(
    scene: &mut ChartScene,
    spec: &ChartSpec,
    x: &ResolvedAxis,
    y: &ResolvedAxis,
    plot: Rect,
) {
    for t in &y.ticks {
        let py = y.map(t.value, plot.bottom(), plot.y);
        if py < plot.y - 0.5 || py > plot.bottom() + 0.5 {
            continue;
        }
        scene.label(LabelPlacement {
            text: t.label.clone(),
            anchor: pt(plot.x - LABEL_GAP, py),
            h_align: HAlign::Right,
            v_align: VAlign::Middle,
            role: LabelRole::AxisY,
            rotation: 0.0,
            color: None,
        });
    }

    for t in &x.ticks {
        let px = x.map(t.value, plot.x, plot.right());
        if px < plot.x - 0.5 || px > plot.right() + 0.5 {
            continue;
        }
        scene.label(LabelPlacement {
            text: t.label.clone(),
            anchor: pt(px, plot.bottom() + LABEL_GAP),
            h_align: HAlign::Center,
            v_align: VAlign::Top,
            role: LabelRole::AxisX,
            rotation: 0.0,
            color: None,
        });
    }

    if let Some(title) = &spec.x.title {
        scene.label(LabelPlacement {
            text: title.clone(),
            anchor: pt(plot.x + plot.w / 2.0, plot.bottom() + LABEL_GAP * 3.0),
            h_align: HAlign::Center,
            v_align: VAlign::Top,
            role: LabelRole::AxisTitleX,
            rotation: 0.0,
            color: None,
        });
    }
    if let Some(title) = &spec.y.title {
        scene.label(LabelPlacement {
            text: title.clone(),
            anchor: pt(plot.x - LABEL_GAP * 4.0, plot.y + plot.h / 2.0),
            h_align: HAlign::Center,
            v_align: VAlign::Middle,
            role: LabelRole::AxisTitleY,
            // Reading bottom-to-top is the near-universal convention for a
            // vertical axis title; -90 rather than +90 keeps it upright.
            rotation: -90.0,
            color: None,
        });
    }
}

fn draw_legend(scene: &mut ChartScene, spec: &ChartSpec, plot: Rect) {
    // Placements only — the host lays the entries out, because their widths
    // depend on text it will measure and we deliberately do not.
    for (i, (_, s)) in spec.visible().enumerate() {
        scene.label(LabelPlacement {
            text: s.name.clone(),
            anchor: pt(plot.x + i as f32 * 100.0, plot.y - LABEL_GAP * 2.0),
            h_align: HAlign::Left,
            v_align: VAlign::Bottom,
            role: LabelRole::Legend,
            rotation: 0.0,
            color: Some(s.color),
        });
    }
}

// ---------------------------------------------------------------------------
// Series
// ---------------------------------------------------------------------------

fn draw_series(
    scene: &mut ChartScene,
    hit: &mut HitIndex,
    spec: &ChartSpec,
    x: &ResolvedAxis,
    y: &ResolvedAxis,
    plot: Rect,
) {
    let bar_series: Vec<usize> = spec
        .visible()
        .filter(|(_, s)| matches!(s.kind, SeriesKind::Bar { .. }))
        .map(|(i, _)| i)
        .collect();

    // Running stack heights, keyed by x, shared across all bar series so
    // each stacks on the last.
    let mut stack: Vec<(f64, f64, f64)> = Vec::new();

    for (si, s) in spec.visible() {
        match &s.kind {
            SeriesKind::Bar { radius } => {
                let slot = bar_series.iter().position(|i| *i == si).unwrap_or(0);
                draw_bars(
                    scene, hit, spec, x, y, plot, si, s, *radius, slot, bar_series.len(),
                    &mut stack,
                );
            }
            SeriesKind::Line { width, smooth, dash, points } => {
                let pts = project(s.data.iter(), x, y, plot);
                push_hits(hit, &pts, si, &s.data);
                if pts.len() >= 2 {
                    let path = build_line(&pts, *smooth);
                    scene.push(Mark::Stroke {
                        layer: Layer::Series,
                        path,
                        stroke: Stroke {
                            width: *width,
                            cap: LineCap::Round,
                            join: LineJoin::Round,
                            dash: dash.clone(),
                            dash_offset: 0.0,
                        },
                        paint: Paint::solid(s.color),
                    });
                }
                if *points {
                    scene.push(Mark::Points {
                        layer: Layer::Series,
                        instances: pts
                            .iter()
                            .map(|p| point_instance(*p, width * 1.6, s.color))
                            .collect(),
                    });
                }
            }
            SeriesKind::Area { width, smooth, gradient } => {
                let pts = project(s.data.iter(), x, y, plot);
                push_hits(hit, &pts, si, &s.data);
                if pts.len() >= 2 {
                    // Fill down to zero when it is in view, otherwise to the
                    // bottom of the plot — an area chart whose baseline is
                    // off-screen should still read as filled.
                    let base = if y.min <= 0.0 && y.max >= 0.0 {
                        y.map(0.0, plot.bottom(), plot.y)
                    } else {
                        plot.bottom()
                    };
                    let mut fill = build_line(&pts, *smooth);
                    let (first, last) = (pts[0], pts[pts.len() - 1]);
                    fill = fill.line_to(last.x, base).line_to(first.x, base).close();
                    let paint = if *gradient {
                        Paint::vertical_fade(s.color.with_alpha(110), plot.y, base)
                    } else {
                        Paint::solid(s.color.with_alpha(70))
                    };
                    scene.push(Mark::Fill {
                        layer: Layer::AreaFill,
                        path: fill,
                        paint,
                        rule: FillRule::NonZero,
                    });
                    scene.push(Mark::Stroke {
                        layer: Layer::Series,
                        path: build_line(&pts, *smooth),
                        stroke: Stroke { width: *width, cap: LineCap::Round, join: LineJoin::Round, ..Default::default() },
                        paint: Paint::solid(s.color),
                    });
                }
            }
            SeriesKind::Scatter { radius } => {
                let pts = project(s.data.iter(), x, y, plot);
                push_hits(hit, &pts, si, &s.data);
                scene.push(Mark::Points {
                    layer: Layer::Series,
                    instances: pts.iter().map(|p| point_instance(*p, *radius, s.color)).collect(),
                });
            }
        }
    }
}

fn point_instance(p: Point, r: f32, color: Color) -> PointInstance {
    PointInstance { center: p, half: pt(r, r), radius: r, color }
}

/// Project data to pixels, dropping anything the scales cannot place.
///
/// Returns positions paired with their ORIGINAL index, because a dropped
/// point would otherwise shift every subsequent hit-test result onto the
/// wrong datum.
fn project<'a>(
    data: impl Iterator<Item = &'a Datum>,
    x: &ResolvedAxis,
    y: &ResolvedAxis,
    plot: Rect,
) -> Vec<Point> {
    data.filter(|d| x.is_plottable(d.x) && y.is_plottable(d.y))
        .map(|d| {
            pt(
                x.map(d.x, plot.x, plot.right()),
                y.map(d.y, plot.bottom(), plot.y),
            )
        })
        .collect()
}

fn push_hits(hit: &mut HitIndex, pts: &[Point], series: usize, data: &[Datum]) {
    for (i, p) in pts.iter().enumerate() {
        if let Some(d) = data.get(i) {
            hit.push(*p, series, i, *d);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_bars(
    scene: &mut ChartScene,
    hit: &mut HitIndex,
    spec: &ChartSpec,
    x: &ResolvedAxis,
    y: &ResolvedAxis,
    plot: Rect,
    series_index: usize,
    s: &crate::spec::Series,
    radius: f32,
    slot: usize,
    slot_count: usize,
    stack: &mut Vec<(f64, f64, f64)>,
) {
    let n_slots = x.categories.unwrap_or_else(|| distinct_x(spec).max(1));
    let slot_w = if n_slots > 0 { plot.w / n_slots as f32 } else { plot.w };
    let band = slot_w * (1.0 - spec.bar_group_padding).clamp(0.05, 1.0);
    let stacked = spec.bar_layout == BarLayout::Stacked;
    let bar_w = if stacked { band } else { band / slot_count.max(1) as f32 };

    let zero_y = if y.min <= 0.0 && y.max >= 0.0 {
        y.map(0.0, plot.bottom(), plot.y)
    } else if y.min > 0.0 {
        plot.bottom()
    } else {
        plot.y
    };

    for (i, d) in s.data.iter().enumerate() {
        if !x.is_plottable(d.x) || !y.is_plottable(d.y) {
            continue;
        }
        let center = x.map(d.x, plot.x, plot.right());
        let left = if stacked {
            center - band / 2.0
        } else {
            center - band / 2.0 + slot as f32 * bar_w
        };

        let (y_from, y_to) = if stacked {
            let e = match stack.iter_mut().find(|(sx, _, _)| *sx == d.x) {
                Some(e) => e,
                None => {
                    stack.push((d.x, 0.0, 0.0));
                    stack.last_mut().expect("just pushed")
                }
            };
            let base = if d.y >= 0.0 { &mut e.1 } else { &mut e.2 };
            let from = *base;
            *base += d.y;
            (y.map(from, plot.bottom(), plot.y), y.map(*base, plot.bottom(), plot.y))
        } else {
            (zero_y, y.map(d.y, plot.bottom(), plot.y))
        };

        let (top, h) = (y_from.min(y_to), (y_to - y_from).abs());
        let r = Rect::new(left, top, bar_w, h);

        // Round only the outer end. A stacked segment or a downward bar with
        // all four corners rounded reads as a detached pill rather than as
        // part of a column.
        let radii = if d.y >= 0.0 {
            [radius, radius, 0.0, 0.0]
        } else {
            [0.0, 0.0, radius, radius]
        };
        scene.push(Mark::Fill {
            layer: Layer::Series,
            path: Path::rounded_rect(r, radii),
            paint: Paint::solid(s.color),
            rule: FillRule::NonZero,
        });
        // Anchor the tooltip at the bar's outer end, where a pointer
        // approaching from outside the column meets it first.
        hit.push(pt(left + bar_w / 2.0, y_to), series_index, i, *d);
    }
}

fn distinct_x(spec: &ChartSpec) -> usize {
    let mut xs: Vec<f64> = spec.visible().flat_map(|(_, s)| s.data.iter().map(|d| d.x)).collect();
    xs.sort_by(f64::total_cmp);
    xs.dedup();
    xs.len()
}

// ---------------------------------------------------------------------------
// Line geometry
// ---------------------------------------------------------------------------

fn build_line(pts: &[Point], smooth: bool) -> Path {
    if smooth && pts.len() >= 3 {
        monotone_cubic(pts)
    } else {
        let mut p = Path::new().move_to(pts[0].x, pts[0].y);
        for q in &pts[1..] {
            p = p.line_to(q.x, q.y);
        }
        p
    }
}

/// Monotone cubic interpolation (Fritsch-Carlson).
///
/// Chosen over a plain Catmull-Rom spline because it is *shape-preserving*:
/// the curve never overshoots a local extremum. That matters for charts
/// specifically — a smoothed series of non-negative values must not dip
/// below zero between points, and a plateau must stay flat. A naive spline
/// does both, and the resulting curve asserts data that does not exist.
fn monotone_cubic(pts: &[Point]) -> Path {
    let n = pts.len();
    // Secant slopes between consecutive points.
    let mut secant = vec![0.0f32; n - 1];
    for i in 0..n - 1 {
        let dx = pts[i + 1].x - pts[i].x;
        secant[i] = if dx.abs() < f32::EPSILON {
            0.0
        } else {
            (pts[i + 1].y - pts[i].y) / dx
        };
    }

    // Initial tangents: one-sided at the ends, averaged in the interior.
    let mut m = vec![0.0f32; n];
    m[0] = secant[0];
    m[n - 1] = secant[n - 2];
    for i in 1..n - 1 {
        m[i] = if secant[i - 1] * secant[i] <= 0.0 {
            // Sign change or flat: this is a local extremum, so the tangent
            // must be zero or the curve overshoots it.
            0.0
        } else {
            (secant[i - 1] + secant[i]) / 2.0
        };
    }

    // Fritsch-Carlson limiter: keep each tangent inside the circle of
    // radius 3 around its secant, which is the monotonicity condition.
    for i in 0..n - 1 {
        if secant[i].abs() < f32::EPSILON {
            m[i] = 0.0;
            m[i + 1] = 0.0;
            continue;
        }
        let (a, b) = (m[i] / secant[i], m[i + 1] / secant[i]);
        let s = a * a + b * b;
        if s > 9.0 {
            let tau = 3.0 / s.sqrt();
            m[i] = tau * a * secant[i];
            m[i + 1] = tau * b * secant[i];
        }
    }

    let mut path = Path::new().move_to(pts[0].x, pts[0].y);
    for i in 0..n - 1 {
        let h = pts[i + 1].x - pts[i].x;
        let third = h / 3.0;
        path = path.cubic_to(
            pts[i].x + third,
            pts[i].y + m[i] * third,
            pts[i + 1].x - third,
            pts[i + 1].y - m[i + 1] * third,
            pts[i + 1].x,
            pts[i + 1].y,
        );
    }
    path
}
