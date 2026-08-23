//! Turning a [`ChartSpec`] into a [`ChartScene`].
//!
//! Pure: same spec and rect in, byte-identical scene out. No interior
//! mutability, no globals, no clock. That is what lets the tests be exact
//! `==` goldens, and what lets a host re-render freely on every signal
//! change without worrying about accumulated state.

use crate::hit::HitIndex;
use crate::scale::{self, ResolvedAxis};
use crate::scene::{
    pt, ChartScene, Color, FillRule, GradientStop, HAlign, LabelPlacement, LabelRole, Layer,
    LineCap, LineJoin, Mark, Paint, Path, Point, PointInstance, Rect, Stroke, VAlign,
};
use crate::spec::{
    AreaFill, AnnotationAt, BarLayout, BarStyle, ChartSpec, Datum, Domain, Emphasis,
    HeatmapStyle, Highlight, Interpolation, MarkContext, MarkOverride, PointShape, PointStyle,
    Series, SeriesKind, StepAt,
};

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

/// Render a frame of a transition from `from` to `to`, at progress `t`.
///
/// `t` is an input, never read from a clock — a mid-transition frame is as
/// reproducible as any other render, and the frame loop stays in the host.
///
/// Two things are interpolated: the data (see [`crate::tween::lerp_data`])
/// and the resolved axis DOMAIN. The domain matters as much as the values —
/// without it, new data that changes the range makes the axis jump on the
/// first frame while the marks glide, which reads as a glitch rather than a
/// transition. Tick VALUES come from the destination throughout, so the
/// labels stay put while the gridlines move (see
/// [`scale::resolve_with_ticks`]).
///
/// Specs that cannot be paired point-for-point snap to `to`.
pub fn render_tween(
    from: &ChartSpec,
    to: &ChartSpec,
    at: crate::tween::TweenAt,
    rect: Rect,
    gutters: &Gutters<'_>,
) -> ChartOutput {
    let Some(spec) = crate::tween::lerp_data(from, to, at) else {
        return render_with(to, rect, gutters);
    };
    let e = at.value;

    // Resolve both ENDS, then interpolate the resolved windows. Resolving
    // the tweened data instead would re-fit the domain every frame, so the
    // axis would drift with the marks and never settle where the
    // destination's own auto-fit puts it.
    let (fx, fy) = (
        scale::resolve(&from.x, x_values(from).into_iter()),
        scale::resolve(&from.y, y_values(from).into_iter()),
    );
    let (tx, ty) = (
        scale::resolve(&to.x, x_values(to).into_iter()),
        scale::resolve(&to.y, y_values(to).into_iter()),
    );

    let mut spec = spec;
    spec.x.domain = Domain::Fixed {
        min: crate::tween::lerp_f64(fx.min, tx.min, e),
        max: crate::tween::lerp_f64(fx.max, tx.max, e),
    };
    spec.y.domain = Domain::Fixed {
        min: crate::tween::lerp_f64(fy.min, ty.min, e),
        max: crate::tween::lerp_f64(fy.max, ty.max, e),
    };
    // The `from` spec rides along so a `StyleFn`'s answer can be resolved at
    // BOTH ends and the two interpolated — see `ColorTween`.
    render_inner(
        &spec,
        rect,
        gutters,
        Some((&tx.ticks, &ty.ticks)),
        Some(ColorTween { from, to, t: at.color }),
    )
}

/// BOTH ends of a colour transition, carried down to the three places that
/// resolve a mark's final colour.
///
/// Only a [`StyleFn`](crate::spec::StyleFn)'s own answer needs this. A
/// series' base colour is already interpolated by
/// [`lerp_data`](crate::tween::lerp_data), and emphasis/dim come from the
/// target's highlight because those are instant by design.
///
/// # Why `to` is here and not just `from`
///
/// A callback is a function of the DATUM, so the two colours being
/// interpolated must both come from real endpoint data. Resolving it against
/// the tweened datum makes a threshold recolor flip at whatever frame the
/// interpolated value crosses the threshold — a hard switch in the middle of
/// a smooth transition.
///
/// Resolving only the `from` end that way is not enough, and that bug shipped
/// once: the destination was still taken from the interpolated spec, so with
/// per-channel durations — the whole point of splitting the channels — a
/// finished 180 ms colour fade would sit on whatever the 40%-animated value
/// resolved to, and snap later when it crossed. `regression_color_channel_is_
/// independent_of_the_value_clock` pins it.
///
/// With both ends held, this is the same treatment the axis domain gets
/// above: resolve at the endpoints, interpolate the results.
#[derive(Clone, Copy)]
pub(crate) struct ColorTween<'a> {
    pub from: &'a ChartSpec,
    pub to: &'a ChartSpec,
    /// Pre-eased fraction of the colour channel.
    pub t: f32,
}

/// Resolve one mark's final colour, folding the series' `StyleFn` override
/// onto `ctx.base_color` — and interpolating that whole resolution against
/// the same resolution on the `from` spec when a transition is running.
///
/// `base_color` is the SAME for both evaluations on purpose: it already
/// carries the interpolated series colour plus the target's emphasis and dim,
/// so only what the callback itself *adds* is interpolated. Feeding each end
/// its own base would interpolate the series colour twice.
pub(crate) fn resolve_mark_color(
    series: &Series,
    ctx: &MarkContext,
    tw: Option<&ColorTween<'_>>,
) -> Color {
    // No transition: the spec in hand IS the answer.
    let Some(tw) = tw else {
        return apply_override(ctx.base_color, &series.override_for(ctx));
    };

    // Resolve one endpoint against ITS OWN datum and ITS OWN callback.
    //
    // A series or datum missing from an endpoint means the pair was not
    // shape-matched, which `lerp_data` already rejected — but the render path
    // must not panic on it, so fall back to the spec in hand.
    let at = |end: &ChartSpec| -> Color {
        match end
            .series
            .get(ctx.series)
            .and_then(|s| s.data.get(ctx.index).map(|d| (s, *d)))
        {
            Some((s, datum)) => {
                apply_override(ctx.base_color, &s.override_for(&MarkContext { datum, ..*ctx }))
            }
            None => apply_override(ctx.base_color, &series.override_for(ctx)),
        }
    };

    let end = at(tw.to);
    if tw.t >= 1.0 {
        return end;
    }
    crate::tween::lerp_color(at(tw.from), end, tw.t)
}

/// Gap in pixels between an axis and its labels.
const LABEL_GAP: f32 = 8.0;
/// Length of a tick mark drawn outside the plot area.
const TICK_LEN: f32 = 4.0;

/// Render into `rect`, deriving the data area per `gutters`.
pub fn render_with(spec: &ChartSpec, rect: Rect, gutters: &Gutters<'_>) -> ChartOutput {
    render_inner(spec, rect, gutters, None, None)
}

fn render_inner(
    spec: &ChartSpec,
    rect: Rect,
    gutters: &Gutters<'_>,
    forced_ticks: Option<(&[crate::scale::Tick], &[crate::scale::Tick])>,
    color_tween: Option<ColorTween<'_>>,
) -> ChartOutput {
    // Resolve axes against the FULL rect first. Gutter width depends on the
    // tick labels, which depend on the domain, which does not depend on the
    // plot rect — so this ordering is well-founded and needs no iteration.
    let (fx, fy) = match forced_ticks {
        Some((a, b)) => (Some(a), Some(b)),
        None => (None, None),
    };
    let x = scale::resolve_with_ticks(&spec.x, x_values(spec).into_iter(), fx);
    let y = scale::resolve_with_ticks(&spec.y, y_values(spec).into_iter(), fy);

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

    draw_highlight_band(&mut scene, spec, &x, plot);
    draw_grid(&mut scene, spec, &x, &y, plot);
    draw_axis_labels(&mut scene, spec, &x, &y, rect, plot);
    draw_annotations(&mut scene, spec, &x, &y, plot);
    draw_series(&mut scene, &mut hit, spec, &x, &y, plot, color_tween.as_ref());
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
        for (_, s) in spec.visible().filter(|(_, s)| s.kind.is_bar()) {
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
                .filter(|(_, s)| !s.kind.is_bar())
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
    // Suppressed labels reserve nothing. A sparkline turns both off, and
    // the gutters have to actually vanish or the "no furniture" mode still
    // costs the space the furniture would have taken.
    let widest_y = if spec.y.labels {
        y.ticks
            .iter()
            .map(|t| m.measure(&t.label, LabelRole::AxisY).0)
            .fold(0.0f32, f32::max)
    } else {
        0.0
    };
    let tallest_x = if spec.x.labels {
        x.ticks
            .iter()
            .map(|t| m.measure(&t.label, LabelRole::AxisX).1)
            .fold(0.0f32, f32::max)
    } else {
        0.0
    };

    let gap_y = if spec.y.labels { LABEL_GAP + TICK_LEN } else { 0.0 };
    let gap_x = if spec.x.labels { LABEL_GAP + TICK_LEN } else { 0.0 };
    let mut pad = Padding {
        left: widest_y + gap_y,
        top: 8.0,
        // Half the last x label can overhang the right edge; reserve it so
        // it is not clipped by the surface bounds.
        right: if spec.x.labels {
            x.ticks
                .last()
                .map(|t| m.measure(&t.label, LabelRole::AxisX).0 / 2.0)
                .unwrap_or(0.0)
                .max(8.0)
        } else {
            0.0
        },
        bottom: tallest_x + gap_x,
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

/// Paint the band behind the highlighted column.
///
/// Width comes from the axis's slot, so on a category axis it covers the
/// whole category — including the gaps between grouped bars, which is the
/// point: the column reads as active wherever the pointer is inside it, not
/// only when it is over a bar. On a continuous axis there are no slots, so
/// the band spans the gap to the neighbouring data columns instead.
fn draw_highlight_band(scene: &mut ChartScene, spec: &ChartSpec, x: &ResolvedAxis, plot: Rect) {
    let (Some(color), Some(cx)) = (spec.highlight_band, spec.highlight.column) else {
        return;
    };
    let center = x.map(cx, plot.x, plot.right());
    let width = match x.categories {
        Some(n) if n > 0 => plot.w / n as f32,
        _ => {
            // Continuous axis: use the spacing between adjacent distinct x
            // values, so the band matches the data's own density rather than
            // an arbitrary constant.
            let mut xs: Vec<f64> =
                spec.visible().flat_map(|(_, s)| s.data.iter().map(|d| d.x)).collect();
            xs.sort_by(f64::total_cmp);
            xs.dedup();
            match xs.len() {
                0 | 1 => plot.w * 0.05,
                n => plot.w / (n - 1) as f32,
            }
        }
    };
    let band = Rect::new(center - width / 2.0, plot.y, width, plot.h);
    scene.push(Mark::Fill {
        layer: Layer::Background,
        path: Path::rect(band),
        paint: Paint::solid(color),
        rule: FillRule::NonZero,
    });
}

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
    //
    // Never on a CATEGORY axis. Its resolved window is `-0.5 ..= n-0.5` — the
    // half-slot padding that keeps the first and last slots whole — so zero
    // is strictly inside it for every non-empty category list, and the test
    // below fires. But zero there is not a value, it is the centre of the
    // first category, and the rule draws a seam straight through it.
    if y.categories.is_none() && y.min < 0.0 && y.max > 0.0 {
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
    rect: Rect,
    plot: Rect,
) {
    if spec.y.labels {
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
    }

    if spec.x.labels {
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
            // Anchored against the SURFACE edge, not offset from the plot
            // by a guessed amount. `measured_padding` reserves the outermost
            // band of the left gutter for this title, and a fixed offset
            // from `plot.x` lands wherever the tick labels happen to end —
            // so it collided with them for any axis whose numbers were wide.
            // Hosts that lay out their own gutters (the idealyst SDK routes
            // this role into its own slot) ignore the anchor entirely.
            anchor: pt(rect.x + LABEL_GAP, plot.y + plot.h / 2.0),
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

/// Draw the reference lines and bands.
///
/// Everything is clipped to the resolved window rather than allowed to
/// extend the domain — see [`crate::spec::Annotation`] for why. A marker entirely outside
/// the window emits nothing at all, which is the honest outcome: drawing it
/// pinned to the plot edge would assert a value the axis does not show.
fn draw_annotations(
    scene: &mut ChartScene,
    spec: &ChartSpec,
    x: &ResolvedAxis,
    y: &ResolvedAxis,
    plot: Rect,
) {
    for a in &spec.annotations {
        let stroke = Stroke {
            width: a.width,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            dash: a.dash.clone(),
            dash_offset: 0.0,
        };
        let line_layer = if a.above { Layer::Axis } else { Layer::Grid };
        let band_layer = if a.above { Layer::Overlay } else { Layer::Background };

        match a.at {
            AnnotationAt::Y(v) => {
                if !y.is_plottable(v) {
                    continue;
                }
                let py = y.map(v, plot.bottom(), plot.y);
                if py < plot.y - 0.5 || py > plot.bottom() + 0.5 {
                    continue;
                }
                scene.push(Mark::Stroke {
                    layer: line_layer,
                    path: Path::new().move_to(plot.x, py).line_to(plot.right(), py),
                    stroke,
                    paint: Paint::solid(a.color),
                });
                if let Some(text) = &a.label {
                    scene.label(LabelPlacement {
                        text: text.clone(),
                        // Sits just above the rule at the right edge, where a
                        // series is least likely to be — lines usually run
                        // left-to-right toward their latest value, so the top
                        // right is the emptiest corner of a typical plot.
                        anchor: pt(plot.right() - LABEL_GAP / 2.0, py - LABEL_GAP / 2.0),
                        h_align: HAlign::Right,
                        v_align: VAlign::Bottom,
                        role: LabelRole::Annotation,
                        rotation: 0.0,
                        color: Some(a.color),
                    });
                }
            }
            AnnotationAt::X(v) => {
                if !x.is_plottable(v) {
                    continue;
                }
                let px = x.map(v, plot.x, plot.right());
                if px < plot.x - 0.5 || px > plot.right() + 0.5 {
                    continue;
                }
                scene.push(Mark::Stroke {
                    layer: line_layer,
                    path: Path::new().move_to(px, plot.y).line_to(px, plot.bottom()),
                    stroke,
                    paint: Paint::solid(a.color),
                });
                if let Some(text) = &a.label {
                    scene.label(LabelPlacement {
                        text: text.clone(),
                        anchor: pt(px + LABEL_GAP / 2.0, plot.y + LABEL_GAP / 2.0),
                        h_align: HAlign::Left,
                        v_align: VAlign::Top,
                        role: LabelRole::Annotation,
                        rotation: 0.0,
                        color: Some(a.color),
                    });
                }
            }
            AnnotationAt::YBand { from, to } => {
                let Some((lo, hi)) =
                    span_px(y, from, to, plot.bottom(), plot.y, plot.y, plot.bottom())
                else {
                    continue;
                };
                let r = Rect::new(plot.x, lo, plot.w, hi - lo);
                scene.push(Mark::Fill {
                    layer: band_layer,
                    path: Path::rect(r),
                    paint: Paint::solid(a.color),
                    rule: FillRule::NonZero,
                });
                if let Some(text) = &a.label {
                    scene.label(LabelPlacement {
                        text: text.clone(),
                        anchor: pt(plot.right() - LABEL_GAP / 2.0, lo + LABEL_GAP / 2.0),
                        h_align: HAlign::Right,
                        v_align: VAlign::Top,
                        role: LabelRole::Annotation,
                        rotation: 0.0,
                        color: Some(a.color),
                    });
                }
            }
            AnnotationAt::XBand { from, to } => {
                let Some((lo, hi)) =
                    span_px(x, from, to, plot.x, plot.right(), plot.x, plot.right())
                else {
                    continue;
                };
                let r = Rect::new(lo, plot.y, hi - lo, plot.h);
                scene.push(Mark::Fill {
                    layer: band_layer,
                    path: Path::rect(r),
                    paint: Paint::solid(a.color),
                    rule: FillRule::NonZero,
                });
                if let Some(text) = &a.label {
                    scene.label(LabelPlacement {
                        text: text.clone(),
                        anchor: pt(lo + LABEL_GAP / 2.0, plot.y + LABEL_GAP / 2.0),
                        h_align: HAlign::Left,
                        v_align: VAlign::Top,
                        role: LabelRole::Annotation,
                        rotation: 0.0,
                        color: Some(a.color),
                    });
                }
            }
        }
    }
}

/// Pixel extent of a data span, clamped to the plot, or `None` when the span
/// falls entirely outside the visible window.
///
/// The clamp is what makes a half-visible band correct: it is truncated at
/// the plot edge rather than either overflowing the data area or vanishing
/// because one end could not be placed.
fn span_px(
    axis: &ResolvedAxis,
    from: f64,
    to: f64,
    lo_px: f32,
    hi_px: f32,
    clamp_lo: f32,
    clamp_hi: f32,
) -> Option<(f32, f32)> {
    if !axis.is_plottable(from) || !axis.is_plottable(to) {
        return None;
    }
    let a = axis.map(from, lo_px, hi_px);
    let b = axis.map(to, lo_px, hi_px);
    let (lo, hi) = (a.min(b), a.max(b));
    let (lo, hi) = (lo.max(clamp_lo), hi.min(clamp_hi));
    (hi - lo > f32::EPSILON).then_some((lo, hi))
}

fn draw_legend(scene: &mut ChartScene, spec: &ChartSpec, plot: Rect) {
    // Placements only — the host lays the entries out, because their widths
    // depend on text it will measure and we deliberately do not.
    //
    // Heatmaps are skipped: a legend entry is a color swatch, and a heatmap
    // row has no single color — it has a ramp. The right legend for one is a
    // gradient scale bar, which is a different thing the host draws from
    // `HeatmapStyle::ramp`.
    for (i, (_, s)) in spec.visible().filter(|(_, s)| !s.kind.is_heatmap()).enumerate() {
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

/// The outermost bar series in each direction of one stacked column.
#[derive(Clone, Copy, PartialEq, Debug)]
struct StackCap {
    x: f64,
    /// Series index at the top of the positive stack, if any.
    top: Option<usize>,
    /// Series index at the bottom of the negative stack, if any.
    bottom: Option<usize>,
}

fn draw_series(
    scene: &mut ChartScene,
    hit: &mut HitIndex,
    spec: &ChartSpec,
    x: &ResolvedAxis,
    y: &ResolvedAxis,
    plot: Rect,
    tw: Option<&ColorTween<'_>>,
) {
    let bar_series: Vec<usize> =
        spec.visible().filter(|(_, s)| s.kind.is_bar()).map(|(i, _)| i).collect();

    // Running stack heights, keyed by x, shared across all bar series so
    // each stacks on the last.
    let mut stack: Vec<(f64, f64, f64)> = Vec::new();

    // Which series sits at the OUTER end of each stack, per x.
    //
    // Only that segment may round its corners. Rounding every segment — the
    // rule for grouped bars, where each bar is its own column — puts a
    // rounded seam between stacked segments, so the column reads as a pile
    // of separate pills instead of one bar. Positive and negative stacks cap
    // independently, since a column can grow in both directions.
    let stack_caps: Vec<StackCap> = if spec.bar_layout == BarLayout::Stacked {
        let mut caps: Vec<StackCap> = Vec::new();
        for (si, s) in spec.visible().filter(|(_, s)| s.kind.is_bar()) {
            for d in &s.data {
                let e = match caps.iter_mut().find(|c| c.x == d.x) {
                    Some(e) => e,
                    None => {
                        caps.push(StackCap { x: d.x, top: None, bottom: None });
                        caps.last_mut().expect("just pushed")
                    }
                };
                // A zero-height segment draws nothing and must not claim the
                // cap from the series below it.
                if d.y > 0.0 {
                    e.top = Some(si);
                } else if d.y < 0.0 {
                    e.bottom = Some(si);
                }
            }
        }
        caps
    } else {
        Vec::new()
    };

    let hl = &spec.highlight;

    for (si, s) in spec.visible() {
        // A series with nothing emphasised fades when `dim_others` is on, so
        // the highlighted one actually stands out instead of merely being a
        // little larger.
        let touched = hl.touches_series(si, &s.data);
        let dim = if hl.dim_others && !hl.is_empty() && !touched {
            hl.dim_opacity.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let series_emphasis = if touched { Emphasis::Hovered } else { Emphasis::None };
        let tint = |c: Color| fade(c, dim);

        match &s.kind {
            SeriesKind::Bar(style) => {
                let slot = bar_series.iter().position(|i| *i == si).unwrap_or(0);
                draw_bars(
                    scene, hit, spec, x, y, plot, si, s, style, slot, bar_series.len(),
                    &mut stack, &stack_caps, dim, tw,
                );
            }
            SeriesKind::Line(style) => {
                let pts = project(s.data.iter(), x, y, plot);
                push_hits(hit, &pts, si, &s.data);
                if pts.len() >= 2 {
                    scene.push(Mark::Stroke {
                        layer: Layer::Series,
                        path: build_line(&pts, style.interpolation),
                        stroke: Stroke {
                            width: style.width_for(series_emphasis),
                            cap: LineCap::Round,
                            join: LineJoin::Round,
                            dash: style.dash.clone(),
                            dash_offset: 0.0,
                        },
                        paint: Paint::solid(tint(s.color)),
                    });
                }
                if let Some(ps) = &style.points {
                    push_points(scene, &pts, s, ps, si, hl, dim, tw);
                }
            }
            SeriesKind::Area(style) => {
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
                    if let Some(paint) = area_paint(&style.fill, s.color, plot.y, base, dim) {
                        let mut fill = build_line(&pts, style.line.interpolation);
                        let (first, last) = (pts[0], pts[pts.len() - 1]);
                        fill = fill.line_to(last.x, base).line_to(first.x, base).close();
                        scene.push(Mark::Fill {
                            layer: Layer::AreaFill,
                            path: fill,
                            paint,
                            rule: FillRule::NonZero,
                        });
                    }
                    scene.push(Mark::Stroke {
                        layer: Layer::Series,
                        path: build_line(&pts, style.line.interpolation),
                        stroke: Stroke {
                            width: style.line.width_for(series_emphasis),
                            cap: LineCap::Round,
                            join: LineJoin::Round,
                            dash: style.line.dash.clone(),
                            dash_offset: 0.0,
                        },
                        paint: Paint::solid(tint(s.color)),
                    });
                }
                if let Some(ps) = &style.line.points {
                    push_points(scene, &pts, s, ps, si, hl, dim, tw);
                }
            }
            SeriesKind::Scatter(style) => {
                let pts = project(s.data.iter(), x, y, plot);
                push_hits(hit, &pts, si, &s.data);
                push_points(scene, &pts, s, style, si, hl, dim, tw);
            }
            SeriesKind::Heatmap(style) => {
                draw_heatmap(scene, hit, x, y, plot, si, s, style, hl, dim, tw);
            }
        }
    }
}

/// Scale a color's alpha by `factor`, for the `dim_others` fade.
fn fade(c: Color, factor: f32) -> Color {
    if factor >= 1.0 {
        return c;
    }
    Color { a: (c.a as f32 * factor.clamp(0.0, 1.0)).round() as u8, ..c }
}

/// Same color at a given fraction of full opacity.
fn at_opacity(c: Color, opacity: f32) -> Color {
    Color { a: (255.0 * opacity.clamp(0.0, 1.0)).round() as u8, ..c }
}

/// The paint for an area's fill, or `None` for [`AreaFill::None`].
fn area_paint(fill: &AreaFill, color: Color, top_y: f32, base_y: f32, dim: f32) -> Option<Paint> {
    Some(match fill {
        AreaFill::None => return None,
        AreaFill::Flat { opacity } => Paint::Solid(fade(at_opacity(color, *opacity), dim)),
        AreaFill::Gradient { top_opacity, bottom_opacity } => Paint::Linear {
            from: pt(0.0, top_y),
            to: pt(0.0, base_y),
            stops: vec![
                GradientStop { offset: 0.0, color: fade(at_opacity(color, *top_opacity), dim) },
                GradientStop { offset: 1.0, color: fade(at_opacity(color, *bottom_opacity), dim) },
            ],
        },
        AreaFill::Stops(stops) => Paint::Linear {
            from: pt(0.0, top_y),
            to: pt(0.0, base_y),
            stops: stops
                .iter()
                .map(|s| GradientStop { offset: s.offset, color: fade(s.color, dim) })
                .collect(),
        },
    })
}

/// Emit a series' point markers, sized per datum by its emphasis.
///
/// Rings are a second batch UNDER the fills rather than a stroke per point:
/// `Mark::Points` is the instanced batch, and keeping both passes in that
/// form means a ringed scatter still costs two ops instead of two per point.
#[allow(clippy::too_many_arguments)]
fn push_points(
    scene: &mut ChartScene,
    pts: &[Point],
    series: &Series,
    style: &PointStyle,
    series_index: usize,
    hl: &Highlight,
    dim: f32,
    tw: Option<&ColorTween<'_>>,
) {
    if pts.is_empty() {
        return;
    }
    let data = &series.data;
    let base_fill = fade(style.fill.unwrap_or(series.color), dim);

    // Resolve size and color per point ONCE, then build both batches from
    // the result — a `style_fn` must not be called twice per mark, since a
    // caller is entitled to put non-trivial work in it.
    let resolved: Vec<(f32, Color)> = pts
        .iter()
        .enumerate()
        .map(|(i, _)| {
            let Some(d) = data.get(i) else {
                return (style.radius, base_fill);
            };
            let emphasis = hl.of(series_index, i, d.x);
            let radius = style.radius_for(emphasis);
            let ctx = MarkContext {
                series: series_index,
                index: i,
                datum: *d,
                emphasis,
                base_color: base_fill,
            };
            // Radius comes from the target end only: a marker resizing is a
            // hover response, and `Emphasis` is instant by design.
            let r = series.override_for(&ctx).radius.unwrap_or(radius);
            (r, resolve_mark_color(series, &ctx, tw))
        })
        .collect();

    if let Some(ring) = style.ring {
        scene.push(Mark::Points {
            layer: Layer::Series,
            instances: pts
                .iter()
                .zip(&resolved)
                .map(|(p, (r, _))| {
                    shape_instance(*p, r + ring.width, style.shape, fade(ring.color, dim))
                })
                .collect(),
        });
    }
    scene.push(Mark::Points {
        layer: Layer::Series,
        instances: pts
            .iter()
            .zip(&resolved)
            .map(|(p, (r, c))| shape_instance(*p, *r, style.shape, *c))
            .collect(),
    });
}

/// Fold a [`MarkOverride`] onto a resolved color.
fn apply_override(base: Color, ov: &MarkOverride) -> Color {
    let c = ov.color.unwrap_or(base);
    match ov.opacity {
        Some(o) => fade(c, o),
        None => c,
    }
}

/// One marker instance, with the corner radius that realises its shape.
fn shape_instance(p: Point, r: f32, shape: PointShape, color: Color) -> PointInstance {
    let corner = match shape {
        // Radius == half-extent is a circle, by `ShapeInstance`'s own rule.
        PointShape::Circle => r,
        PointShape::Square => 0.0,
        PointShape::RoundedSquare => r * 0.35,
    };
    PointInstance { center: p, half: pt(r, r), radius: corner, color }
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
    style: &BarStyle,
    slot: usize,
    slot_count: usize,
    stack: &mut Vec<(f64, f64, f64)>,
    stack_caps: &[StackCap],
    dim: f32,
    tw: Option<&ColorTween<'_>>,
) {
    let radius = style.radius;
    let n_slots = x.categories.unwrap_or_else(|| distinct_x(spec).max(1));
    let slot_w = if n_slots > 0 { plot.w / n_slots as f32 } else { plot.w };
    // A per-series `width_fraction` overrides the chart-wide padding, so the
    // common case of uniform spacing stays a single setting on the spec.
    let band = slot_w
        * style
            .width_fraction
            .unwrap_or(1.0 - spec.bar_group_padding)
            .clamp(0.05, 1.0);
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

        // Round only the outer end, and in a stack only on the segment that
        // IS the outer end — see `stack_caps`. An interior segment with
        // rounded corners leaves a visible seam mid-column.
        let caps_this = if stacked {
            stack_caps
                .iter()
                .find(|c| c.x == d.x)
                .map(|c| {
                    if d.y >= 0.0 {
                        c.top == Some(series_index)
                    } else {
                        c.bottom == Some(series_index)
                    }
                })
                .unwrap_or(false)
        } else {
            true
        };
        let radii = match (caps_this, d.y >= 0.0) {
            (false, _) => [0.0; 4],
            (true, true) => [radius, radius, 0.0, 0.0],
            (true, false) => [0.0, 0.0, radius, radius],
        };
        let emphasis = spec.highlight.of(series_index, i, d.x);
        let base = match (emphasis, style.hover_color) {
            (Emphasis::None, _) | (_, None) => s.color,
            (_, Some(c)) => c,
        };
        let base = fade(base, dim);
        let ctx = MarkContext {
            series: series_index,
            index: i,
            datum: *d,
            emphasis,
            base_color: base,
        };
        scene.push(Mark::Fill {
            layer: Layer::Series,
            path: Path::rounded_rect(r, radii),
            paint: Paint::solid(resolve_mark_color(s, &ctx, tw)),
            rule: FillRule::NonZero,
        });
        // Index the bar's whole body, anchoring the tooltip at its outer
        // end where a pointer approaching from outside the column meets it
        // first. A zero-height bar still gets a rect so a hover near the
        // baseline resolves to it — degenerate geometry, but a real datum.
        hit.push_rect(r, pt(left + bar_w / 2.0, y_to), series_index, i, *d);
    }
}

/// Draw one heatmap ROW.
///
/// Each series is a row (see [`SeriesKind::Heatmap`]), so this runs once per
/// row and the y position comes from `Datum::y` — which means a heatmap gets
/// axis resolution, tick labels, gutters and the hit index from the same
/// cartesian pipeline as everything else, with no code of its own.
///
/// Both axes want to be [`AxisKind::Category`](crate::spec::AxisKind): its
/// half-slot padding is what makes the first and last cells whole rather
/// than sliced in half by the plot edge.
#[allow(clippy::too_many_arguments)]
fn draw_heatmap(
    scene: &mut ChartScene,
    hit: &mut HitIndex,
    x: &ResolvedAxis,
    y: &ResolvedAxis,
    plot: Rect,
    series_index: usize,
    s: &Series,
    style: &HeatmapStyle,
    hl: &Highlight,
    dim: f32,
    tw: Option<&ColorTween<'_>>,
) {
    // Pixels per data unit, taken straight from the scale. One expression
    // covers both axis kinds: on a category axis a unit IS a slot, and on a
    // continuous one it is whatever the domain makes it — so there is no
    // "is this categorical?" branch to get wrong.
    let cell_w = (x.map(1.0, plot.x, plot.right()) - x.map(0.0, plot.x, plot.right())).abs();
    let cell_h = (y.map(1.0, plot.bottom(), plot.y) - y.map(0.0, plot.bottom(), plot.y)).abs();
    if cell_w <= 0.0 || cell_h <= 0.0 {
        return;
    }

    let (lo, hi) = style.domain.unwrap_or_else(|| auto_intensity(&s.data));
    let span = hi - lo;

    for (i, d) in s.data.iter().enumerate() {
        if !x.is_plottable(d.x) || !y.is_plottable(d.y) {
            continue;
        }
        let cx = x.map(d.x, plot.x, plot.right());
        let cy = y.map(d.y, plot.bottom(), plot.y);
        let r = Rect::new(
            cx - cell_w / 2.0 + style.gap / 2.0,
            cy - cell_h / 2.0 + style.gap / 2.0,
            (cell_w - style.gap).max(0.0),
            (cell_h - style.gap).max(0.0),
        );
        if r.w <= 0.0 || r.h <= 0.0 {
            continue;
        }

        let t = if span.abs() < f64::EPSILON {
            0.0
        } else {
            ((d.w - lo) / span) as f32
        };
        let emphasis = hl.of(series_index, i, d.x);
        let base = fade(style.ramp.sample(t), dim);
        let ctx = MarkContext {
            series: series_index,
            index: i,
            datum: *d,
            emphasis,
            base_color: base,
        };
        scene.push(Mark::Fill {
            layer: Layer::Series,
            path: Path::rounded_rect(r, [style.radius; 4]),
            paint: Paint::solid(resolve_mark_color(s, &ctx, tw)),
            rule: FillRule::NonZero,
        });

        // A cell fills its slot, so emphasis cannot grow it without
        // overlapping its neighbours. An outline is the one form that adds
        // no geometry anywhere else.
        if emphasis != Emphasis::None {
            if let Some(ring) = style.emphasis_ring {
                scene.push(Mark::Stroke {
                    layer: Layer::Overlay,
                    path: Path::rounded_rect(r, [style.radius; 4]),
                    stroke: Stroke::width(ring.width),
                    paint: Paint::solid(ring.color),
                });
            }
        }

        hit.push_rect(r, pt(cx, cy), series_index, i, *d);
    }
}

/// The intensity range to sample the ramp over when none was given.
///
/// A degenerate span falls back to including zero, so a row of identical
/// values still renders as a definite color rather than dividing by nothing.
/// If they are all zero there is genuinely no variation to show, and the
/// bottom of the ramp is the honest answer.
fn auto_intensity(data: &[Datum]) -> (f64, f64) {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for d in data.iter().filter(|d| d.w.is_finite()) {
        lo = lo.min(d.w);
        hi = hi.max(d.w);
    }
    if !lo.is_finite() || !hi.is_finite() {
        return (0.0, 1.0);
    }
    if (hi - lo).abs() < f64::EPSILON {
        return (lo.min(0.0), hi.max(0.0));
    }
    (lo, hi)
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

fn build_line(pts: &[Point], interp: Interpolation) -> Path {
    match interp {
        // Monotone needs three points to have an interior tangent to limit;
        // with two it degenerates to the straight segment anyway.
        Interpolation::Monotone if pts.len() >= 3 => monotone_cubic(pts),
        Interpolation::Step(at) => step_path(pts, at),
        _ => {
            let mut p = Path::new().move_to(pts[0].x, pts[0].y);
            for q in &pts[1..] {
                p = p.line_to(q.x, q.y);
            }
            p
        }
    }
}

/// Piecewise-constant interpolation.
///
/// Every variant emits exactly two segments per interval, so the path stays
/// axis-aligned and an area fill built from it closes cleanly against the
/// baseline with no diagonal edge — which is the whole reason a stepped
/// area reads as a histogram rather than as a lumpy polygon.
fn step_path(pts: &[Point], at: StepAt) -> Path {
    let mut p = Path::new().move_to(pts[0].x, pts[0].y);
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        match at {
            StepAt::After => {
                p = p.line_to(b.x, a.y).line_to(b.x, b.y);
            }
            StepAt::Before => {
                p = p.line_to(a.x, b.y).line_to(b.x, b.y);
            }
            StepAt::Mid => {
                let mid = (a.x + b.x) / 2.0;
                p = p.line_to(mid, a.y).line_to(mid, b.y).line_to(b.x, b.y);
            }
        }
    }
    p
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
