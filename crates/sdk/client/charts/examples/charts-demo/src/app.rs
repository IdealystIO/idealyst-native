//! The demo screen: a chart, a row of controls, a live hover readout, and a
//! tooltip.
//!
//! The tooltip is the part worth reading. The SDK renders no hover surface —
//! it reports `on_hover` and nothing else — so everything below the
//! `Placement` enum is APP code, not framework code. That is the point of the
//! design: three genuinely different placements (follow the cursor, snap to
//! the hovered mark, track x while pinning y) are ~40 lines each on the same
//! callback, and none of them is privileged by the SDK.

use std::cell::RefCell;
use std::rc::Rc;

use charts::prelude::*;
use charts::{ChartHover, DatumRef, MarkBounds, MarkContext, MarkOverride, PolarHover, StyleFn};
use idea_ui::{install_idea_theme, light_theme, tone, variant, Button, VariantRef};
use runtime_core::{after_animation_frame, component, rx, signal, switch, ui, when,
    AnchorableHandle, Element,
    IntoElement, LayoutSubscription, Length, Position, Ref, Signal, StyleApplication, StyleRules,
    StyleSheet, ViewHandle};

/// Values glide; colours fade FASTER.
///
/// The two are separate `Transition`s on purpose, and the demo picks
/// deliberately different numbers to show why. 420 ms is right for a bar
/// changing height — long enough to follow, short enough not to feel slow.
/// The same 420 ms on a hue change reads as sluggish, because there is no
/// distance for the eye to track, so the colour channel runs at 180 ms.
///
/// Exactly the `Transition` the style system uses: this is the same type and
/// the same `Easing` a stylesheet spells for `background_transition`.
const VALUE_GLIDE: Option<Transition> = Some(Transition {
    duration_ms: 420,
    easing: Easing::EaseInOut,
});
const COLOR_FADE: Option<Transition> = Some(Transition {
    duration_ms: 180,
    easing: Easing::EaseOut,
});

/// Where the demo puts its tooltip.
///
/// Three placements that real charts actually use, to show that the SDK
/// privileges none of them. Each is a pure function of the pointer frame
/// (`charts::PointerFrame`) and the hovered entries — see [`place`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Placement {
    /// Follows the pointer on both axes. Simple, and usually the worst of
    /// the three: the bubble jitters over a mark the user is trying to read.
    Cursor,
    /// Snaps beside the hovered mark. For bars this uses the bar's `bounds`
    /// rather than its anchor, so the bubble sits at the bar's vertical
    /// middle instead of hopping to its tip as the value changes.
    #[default]
    Mark,
    /// Tracks x, pins y to the top of the plot. The common "scrubbing a time
    /// series" idiom: the bubble stays on one line and never covers the marks.
    TrackX,
}

impl Placement {
    const ALL: [Placement; 3] = [Placement::Cursor, Placement::Mark, Placement::TrackX];

    fn label(self) -> &'static str {
        match self {
            Placement::Cursor => "Tip: cursor",
            Placement::Mark => "Tip: snap to mark",
            Placement::TrackX => "Tip: track x",
        }
    }
}

/// Resolve a hover into the viewport-space point the bubble should sit beside.
///
/// Returns an ANCHOR, not a final position: the offset, the edge flip and the
/// clamp all need the bubble's measured size, which is not known here — see
/// [`resolve`]. Keeping the two apart is what lets the placement mode stay a
/// three-line match while edge handling stays in one place for all of them.
///
/// Everything here is app code. The only SDK inputs are the frame's three
/// coordinate spaces and the entries' `position` / `bounds` — which is
/// exactly the surface the chart promises and nothing more.
fn place(mode: Placement, h: &ChartHover) -> (f32, f32) {
    let at = &h.at;
    match mode {
        // `window` is already viewport-space, so no conversion is needed.
        Placement::Cursor => (at.window.x, at.window.y),

        Placement::Mark => {
            let first = &h.entries[0];
            // A bar gets its body's right edge and vertical middle; anything
            // else gets its anchor. This is the distinction `bounds` exists
            // for: `position` on a bar is its OUTER END, so a bubble placed
            // there rides up and down as the value changes.
            let local = match first.bounds {
                MarkBounds::Rect(r) => charts::Point { x: r.x + r.w, y: r.y + r.h / 2.0 },
                _ => first.position,
            };
            let v = at.to_viewport(local);
            (v.x, v.y)
        }

        // Track x from the cursor, pin y to the plot's top edge. Mixing the
        // two spaces is why the frame carries both: x comes from `window`,
        // y from the plot rect.
        Placement::TrackX => (at.window.x, at.plot.y + 24.0),
    }
}

/// Gap between the anchor and the bubble.
const TIP_GAP: f32 = 12.0;
/// Keep-out margin from the viewport edges.
const TIP_MARGIN: f32 = 8.0;

/// Turn an anchor plus a measured bubble size into a top-left that stays on
/// screen.
///
/// Two different corrections, because the axes fail differently:
///
/// - **x flips.** Past the right edge the bubble moves to the OTHER side of
///   the anchor, so it never covers the mark it describes. Sliding it left
///   instead would park it on top of the very point the user is pointing at.
///   The flip only happens if there is actually room on the left; otherwise
///   it falls through to a clamp.
/// - **y clamps.** There is no "other side" that reads better for a bubble
///   that is already vertically centred on its anchor, and flipping it would
///   make it jump for no visible reason.
///
/// `size` is `(0, 0)` on the frame before the bubble is first measured. That
/// is harmless: the bubble is invisible until it has a size anyway, and the
/// layout callback lands before paint.
fn resolve(anchor: (f32, f32), size: (f32, f32), vp: (f32, f32)) -> (f32, f32) {
    let (ax, ay) = anchor;
    let (bw, bh) = size;
    let (vw, vh) = vp;

    let right = ax + TIP_GAP;
    let x = if right + bw + TIP_MARGIN > vw && ax - TIP_GAP - bw >= TIP_MARGIN {
        ax - TIP_GAP - bw
    } else {
        // No room to flip either: clamp, and never past the left margin.
        right.min((vw - bw - TIP_MARGIN).max(TIP_MARGIN))
    };

    let y = (ay - bh / 2.0).clamp(TIP_MARGIN, (vh - bh - TIP_MARGIN).max(TIP_MARGIN));
    (x, y)
}

/// The bubble — an ordinary view, positioned in viewport space.
///
/// # Three reactive layers, and why
///
/// The obvious shape — build the whole bubble inside
/// `when(|| tip.get().is_some())` — is WRONG, and wrong in a way that looks
/// like it works. `when` dedups on its predicate's BOOLEAN: once `tip` is
/// `Some` the predicate stays `true`, so the branch closure never re-runs and
/// the bubble keeps whichever column you hovered first while the readout
/// beside it moves on. It only appears to work if you test by leaving the
/// chart between hovers, which flips the predicate and forces a rebuild.
///
/// So the work is split by how often each part actually changes:
///
/// - `when` decides EXISTENCE only.
/// - a `switch` keyed on the lines drives CONTENT — rebuilding the text nodes
///   only when the hovered column changes, not on every pixel.
/// - a reactive style closure drives POSITION — re-resolving without
///   rebuilding anything, which is what makes a per-move placement cheap.
///
/// Absolute rather than fixed because the framework has no `Position::Fixed`;
/// this works because the demo's root spans the viewport and does not scroll.
/// An app with a scrolling root would subtract its scroll offset here, or
/// portal the bubble through `anchored_overlay`.
fn tooltip_surface(tip: Signal<Option<(Vec<String>, f32, f32)>>) -> Element {
    // The bubble's own measured size, for the edge flip in `resolve`. Starts
    // at zero — the first frame has no measurement, and `resolve` degrades to
    // "no flip" rather than guessing a width and jumping once the real one
    // lands.
    let size = signal((0.0_f32, 0.0_f32));
    let bubble_ref: Ref<ViewHandle> = Ref::new();

    // `bubble_ref` is not filled until the view is mounted, so subscribing has
    // to wait a frame. The subscription is held in a scope-owned slot: drop it
    // and the callback stops firing, which would freeze the measured size at
    // whatever the first layout reported.
    let holder: Rc<RefCell<Option<LayoutSubscription>>> = Rc::new(RefCell::new(None));
    let setup = {
        let holder = holder.clone();
        after_animation_frame(move || {
            let sub = bubble_ref.with(|h| {
                let r = h.rect();
                size.set((r.width, r.height));
                h.on_layout(move |w, ht| {
                    // Guarded: layout fires on every pass, and an
                    // unconditional set would re-resolve the position style
                    // on every unrelated relayout.
                    let (pw, ph) = size.get();
                    if (pw - w).abs() > 0.5 || (ph - ht).abs() > 0.5 {
                        size.set((w, ht));
                    }
                })
            });
            if let Some(sub) = sub {
                *holder.borrow_mut() = Some(sub);
            }
        })
    };
    {
        let holder = holder.clone();
        runtime_core::on_scope_drop(move || {
            drop(setup);
            holder.borrow_mut().take();
        });
    }

    let rows = switch(
        move || tip.get().map(|(lines, _, _)| lines).unwrap_or_default(),
        move |lines: &Vec<String>| {
            runtime_core::view(
                lines
                    .iter()
                    .map(|l| {
                        runtime_core::text(l.clone())
                            .with_style(std::rc::Rc::new(StyleSheet::r#static(StyleRules {
                                font_size: Some(Length::Px(12.0).into()),
                                color: Some(runtime_core::Color("#f5f5f5".into()).into()),
                                ..Default::default()
                            })))
                            .into_element()
                    })
                    .collect(),
            )
            .with_style(std::rc::Rc::new(StyleSheet::r#static(StyleRules {
                flex_direction: Some(runtime_core::FlexDirection::Column),
                row_gap: Some(Length::Px(2.0).into()),
                ..Default::default()
            })))
            .into_element()
        },
    );

    runtime_core::view(vec![rows])
        .bind(bubble_ref)
        .with_style(move || {
            let anchor = tip.get().map(|(_, x, y)| (x, y)).unwrap_or((0.0, 0.0));
            let vp = runtime_core::viewport_size().get();
            let (x, y) = resolve(anchor, size.get(), (vp.width, vp.height));
            StyleApplication::new(std::rc::Rc::new(StyleSheet::r#static(StyleRules {
                position: Some(Position::Absolute),
                left: Some(Length::Px(x).into()),
                top: Some(Length::Px(y).into()),
                // A definite min-width: an absolutely-positioned box with no
                // width is shrink-to-fit, and the DOM resolves that toward
                // max-content while Taffy resolves it toward MIN-content —
                // which for a text run is one character per line. Same bubble
                // on every backend, no per-platform branch.
                min_width: Some(Length::Px(120.0).into()),
                padding_top: Some(Length::Px(6.0).into()),
                padding_bottom: Some(Length::Px(6.0).into()),
                padding_left: Some(Length::Px(9.0).into()),
                padding_right: Some(Length::Px(9.0).into()),
                border_top_left_radius: Some(Length::Px(6.0).into()),
                border_top_right_radius: Some(Length::Px(6.0).into()),
                border_bottom_left_radius: Some(Length::Px(6.0).into()),
                border_bottom_right_radius: Some(Length::Px(6.0).into()),
                background: Some(runtime_core::Color("#1e1e24".into()).into()),
                // The bubble must never eat pointer events, or moving onto it
                // ends the hover that produced it and it flickers away.
                pointer_events: Some(runtime_core::PointerEvents::None),
                ..Default::default()
            })))
        })
        .into_element()
}

/// Which chart the demo is showing. One enum drives both the button row and
/// the spec builder, so adding a kind is a single edit in each.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Kind {
    #[default]
    Line,
    Smooth,
    Stepped,
    Area,
    Bars,
    Stacked,
    Scatter,
    Heatmap,
    Donut,
    Pie,
    Gauge,
    Radial,
}

impl Kind {
    const ALL: [Kind; 12] = [
        Kind::Line,
        Kind::Smooth,
        Kind::Stepped,
        Kind::Area,
        Kind::Bars,
        Kind::Stacked,
        Kind::Scatter,
        Kind::Heatmap,
        Kind::Donut,
        Kind::Pie,
        Kind::Gauge,
        Kind::Radial,
    ];

    fn label(self) -> &'static str {
        match self {
            Kind::Line => "Line",
            Kind::Smooth => "Smooth",
            Kind::Stepped => "Stepped",
            Kind::Area => "Area",
            Kind::Bars => "Bars",
            Kind::Stacked => "Stacked",
            Kind::Scatter => "Scatter",
            Kind::Heatmap => "Heatmap",
            Kind::Donut => "Donut",
            Kind::Pie => "Pie",
            Kind::Gauge => "Gauge",
            Kind::Radial => "Radial",
        }
    }

    /// Bar and heatmap kinds share the categorical x axis; the rest are
    /// continuous.
    fn is_categorical(self) -> bool {
        matches!(self, Kind::Bars | Kind::Stacked | Kind::Heatmap)
    }

    /// Which component renders it. A pie and a gauge are different SPEC
    /// types, not different settings on one — see `charts_core::polar` for
    /// why folding polar into `ChartSpec` would be a mistake.
    fn family(self) -> Family {
        match self {
            Kind::Donut | Kind::Pie => Family::Pie,
            Kind::Gauge | Kind::Radial => Family::Radial,
            _ => Family::Cartesian,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Family {
    Cartesian,
    Pie,
    Radial,
}

const BLUE: Color = Color::rgb(0x4c, 0x8d, 0xff);
const PINK: Color = Color::rgb(0xff, 0x6b, 0x9d);
const MINT: Color = Color::rgb(0x3d, 0xd5, 0x98);
const AMBER: Color = Color::rgb(0xff, 0xb0, 0x3d);
const RED: Color = Color::rgb(0xe2, 0x4a, 0x33);
const MONTHS: [&str; 8] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug"];
/// Rows of the heatmap. Listed bottom-up, because row 0 sits at the bottom —
/// the y axis points the same way for a heatmap as for everything else.
const HOURS: [&str; 4] = ["18:00", "12:00", "06:00", "00:00"];

/// Deterministic pseudo-data. A real app holds actual values in the signal;
/// the demo only needs something that visibly changes on demand, and a
/// seeded generator keeps successive runs comparable.
fn series_values(seed: u64, offset: f64) -> Vec<f64> {
    (0..MONTHS.len())
        .map(|i| {
            let mixed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add((i as u64).wrapping_mul(1_442_695_040_888_963_407));
            ((mixed >> 33) % 100) as f64 / 10.0 + offset
        })
        .collect()
}

/// Build the spec for the current controls. Pure — the reactive wiring is
/// the `rx!` at the call site, not anything in here.
#[allow(clippy::too_many_arguments)]
fn build_spec(
    kind: Kind,
    seed: u64,
    show_costs: bool,
    fill: AreaFill,
    dim: bool,
    selected: Vec<DatumRef>,
    style_fn: Option<StyleFn>,
    annotate: bool,
) -> ChartSpec {
    if kind == Kind::Heatmap {
        return build_heatmap(seed);
    }
    let a = series_values(seed, 2.0);
    let b = series_values(seed.wrapping_add(9), 1.0);

    // Ring the markers in the surface color so a node reads as a distinct
    // point sitting ON the line rather than a bulge in the stroke.
    let node = PointStyle::new(3.0)
        .hover(7.0)
        .selected(8.5)
        .ring(Color::rgb(0xff, 0xff, 0xff), 2.0);

    let series_kind = match kind {
        Kind::Line => SeriesKind::Line(
            LineStyle::new(2.5).hover_width(4.0).with_points(node),
        ),
        Kind::Smooth => SeriesKind::Line(
            LineStyle::new(2.5).smooth().hover_width(4.0).with_points(node),
        ),
        // `StepAt::After` holds each reading until the next one, which is
        // what a value sampled at intervals actually did — a straight
        // segment between samples asserts a drift that was never measured.
        Kind::Stepped => SeriesKind::Line(
            LineStyle::new(2.5).stepped(StepAt::After).hover_width(4.0).with_points(node),
        ),
        Kind::Area => SeriesKind::Area(
            AreaStyle::default()
                .line(LineStyle::new(2.5).smooth().with_points(node))
                .fill(fill),
        ),
        Kind::Bars | Kind::Stacked => {
            SeriesKind::Bar(BarStyle::new(5.0).hover_color(Color::rgb(0x1f, 0x5f, 0xd0)))
        }
        Kind::Scatter => SeriesKind::Scatter(
            PointStyle::new(4.0).hover(9.0).selected(11.0).ring(Color::rgb(0xff, 0xff, 0xff), 1.5),
        ),
        // Handled above; the polar kinds never reach `build_spec` at all.
        Kind::Heatmap | Kind::Donut | Kind::Pie | Kind::Gauge | Kind::Radial => {
            SeriesKind::line()
        }
    };

    let to_data = |vals: &[f64]| -> Vec<Datum> {
        vals.iter().enumerate().map(|(i, v)| datum(i as f64, *v)).collect()
    };

    let mut revenue = Series::new("revenue", series_kind.clone(), BLUE, to_data(&a));
    let mut costs = Series::new("costs", series_kind, PINK, to_data(&b));
    // The SAME Rc is cloned into both series and reused across renders —
    // building a fresh closure here would give every spec a new pointer, so
    // no two specs would compare equal and the chart would re-render on
    // every reactive tick. See `charts::StyleFn`.
    revenue.style_fn = style_fn.clone();
    costs.style_fn = style_fn;
    // A hidden series keeps its color and its legend slot rather than being
    // removed from the vec — that is exactly what the toggle demonstrates,
    // and dropping it instead would reshuffle every other series' color.
    // Two overlapping gradient fills are unreadable, so an area chart always
    // shows one series.
    costs.visible = show_costs && kind != Kind::Area;

    let x_axis = if kind.is_categorical() { Axis::category(MONTHS) } else { Axis::linear() };

    let mut highlight = Highlight::default().with_points(selected);
    highlight.dim_others = dim;

    let mut spec = ChartSpec::new(vec![revenue, costs])
        .x(x_axis)
        .y(Axis::linear().include_zero(true).title("USD (k)"))
        .bars(if kind == Kind::Stacked { BarLayout::Stacked } else { BarLayout::Grouped })
        .legend(true)
        .highlight(highlight)
        // Band behind the hovered column, so the whole category reads as
        // active even when the pointer is in the gap between grouped bars.
        .highlight_band(Color::rgba(0x33, 0x33, 0x33, 18));

    if annotate {
        // Reference markers, not series: they take no part in domain
        // fitting, so a target well above the data cannot flatten it.
        spec = spec
            .annotate(Annotation::y_band(0.0, 5.0, Color::rgba(0x3d, 0xd5, 0x98, 26)).label("floor"))
            .annotate(Annotation::y_line(9.0, RED).dashed([6.0, 4.0]).label("target"))
            .annotate(Annotation::x_line(5.0, PINK).dashed([3.0, 3.0]).label("launch"));
    }
    spec
}

/// A heatmap: one series per ROW, `cell(column, row, value)` per datum.
fn build_heatmap(seed: u64) -> ChartSpec {
    let ramp = ColorRamp::three(
        Color::rgb(0x1b, 0x2a, 0x4a),
        Color::rgb(0x4c, 0x8d, 0xff),
        Color::rgb(0xff, 0xd1, 0x66),
    );
    let rows: Vec<Series> = HOURS
        .iter()
        .enumerate()
        .map(|(r, name)| {
            let vals = series_values(seed.wrapping_add(r as u64 * 13), 0.0);
            Series::new(
                *name,
                SeriesKind::Heatmap(
                    // Pinned rather than auto-fitted: every row must mean the
                    // same thing by color, and per-row auto-fitting would make
                    // the busiest cell of a quiet row look like the busiest
                    // cell of a loud one.
                    HeatmapStyle::new(ramp.clone()).domain(0.0, 10.0).radius(3.0).gap(3.0),
                ),
                BLUE,
                (0..MONTHS.len()).map(|c| cell(c as f64, r as f64, vals[c])).collect(),
            )
        })
        .collect();
    ChartSpec::new(rows)
        .x(Axis::category(MONTHS))
        .y(Axis::category(HOURS))
        .highlight_band(Color::rgba(0x33, 0x33, 0x33, 18))
}

/// The pie/donut spec. Shares its numbers with the cartesian demo so the
/// same "New data" button visibly drives both.
fn build_pie(kind: Kind, seed: u64, selected: Vec<usize>) -> PieSpec {
    let vals = series_values(seed, 3.0);
    let slices = vec![
        Slice::new("direct", vals[0], BLUE),
        Slice::new("search", vals[1], PINK),
        Slice::new("social", vals[2], MINT),
        Slice::new("email", vals[3], AMBER),
    ];
    let total: f64 = slices.iter().map(|s| s.value).sum();
    let inner = if kind == Kind::Donut { 0.62 } else { 0.0 };
    let mut spec = PieSpec::donut(slices, inner)
        .pad_angle(1.5)
        .hover_grow(8.0)
        .labels(if kind == Kind::Donut { PieLabels::None } else { PieLabels::Leader })
        .legend(true);
    spec.highlight.selected = selected;
    if kind == Kind::Donut {
        spec = spec.center(format!("{total:.0}")).center_sub("sessions".to_string());
    }
    spec
}

/// A gauge is the one-bar case of a radial bar chart — same geometry, same
/// code path, no separate type.
fn build_radial(kind: Kind, seed: u64, selected: Vec<usize>) -> RadialSpec {
    let vals = series_values(seed, 2.0);
    let pct = |v: f64| (v * 9.0).clamp(0.0, 100.0);
    let mut spec = if kind == Kind::Gauge {
        RadialSpec::gauge("throughput", pct(vals[0]), 100.0, MINT)
            .center(format!("{:.0}%", pct(vals[0])))
            .center_sub("of capacity".to_string())
    } else {
        RadialSpec::new(vec![
            RadialBar::new("api", pct(vals[0]), BLUE),
            RadialBar::new("web", pct(vals[1]), PINK),
            RadialBar::new("jobs", pct(vals[2]), MINT),
            RadialBar::new("cache", pct(vals[3]), AMBER),
        ])
        .thickness(18.0)
        .gap(8.0)
        .labels(true)
        .legend(true)
    };
    spec.highlight.selected = selected;
    spec
}

/// The KPI cards under the main chart.
const SPARKS: [(&str, Color); 4] =
    [("Sessions", BLUE), ("Signups", PINK), ("Revenue", MINT), ("Churn", AMBER)];

/// A labelled area-fill preset.
type FillMode = (&'static str, fn() -> AreaFill);

/// The area fill modes the demo cycles through.
const FILLS: [FillMode; 4] = [
    ("Fill: gradient", || AreaFill::Gradient { top_opacity: 0.45, bottom_opacity: 0.0 }),
    ("Fill: deep", || AreaFill::Gradient { top_opacity: 0.85, bottom_opacity: 0.15 }),
    ("Fill: flat", || AreaFill::Flat { opacity: 0.3 }),
    ("Fill: none", || AreaFill::None),
];

/// A control button, filled while its kind is the active one.
///
/// `#[prop(static)]` on `kind_of`: inline props wrap data params in
/// `Reactive<T>` by default, but this one identifies the button and never
/// changes for a given instance — wrapping it would only force a `.get()`
/// on every read.
#[component]
pub fn KindButton(#[prop(static)] kind_of: Kind, active: Signal<Kind>) -> Element {
    let this = kind_of;
    ui! {
        Button(
            label = this.label().to_string(),
            tone = tone::Primary,
            variant = rx!(if active.get() == this {
                VariantRef::from(variant::Filled)
            } else {
                VariantRef::from(variant::Outlined)
            }),
            // `on_click` is `Rc<dyn Fn()>`, not a generic closure param —
            // a bare closure does not coerce, so the cast is required.
            on_click = Rc::new(move || active.set(this)) as Rc<dyn Fn()>,
        )
    }
}

/// A KPI card with an inline sparkline.
///
/// The whole point of sparkline mode: no axes, no grid, no legend, so the
/// chart is small enough to sit beside a number and the surrounding text
/// says what the axes would have.
#[component]
pub fn SparkCard(
    label: String,
    // Identifies the card and never changes for a given instance, so it is
    // static; the explicit default is required because `Color` has no
    // `Default` and inline props need one to build the props struct.
    #[prop(static, default = BLUE)] color: Color,
    seed: u64,
) -> Element {
    let seed_for_value = seed.clone();
    let spark = rx!({
        let vals = series_values(seed.get(), 1.0);
        ChartSpec::new(vec![Series::new(
            "v",
            SeriesKind::Area(
                AreaStyle::default()
                    .line(LineStyle::new(1.75).smooth())
                    .fill(AreaFill::Gradient { top_opacity: 0.35, bottom_opacity: 0.0 }),
            ),
            color,
            vals.iter().enumerate().map(|(i, v)| datum(i as f64, *v)).collect(),
        )])
        .sparkline()
    });
    let value = rx!(format!(
        "{:.1}",
        series_values(seed_for_value.get(), 1.0).last().copied().unwrap_or(0.0)
    ));
    ui! {
        view(style = styles::SparkCard()) {
            text(style = styles::SparkLabel()) { label }
            text(style = styles::SparkValue()) { value }
            view(style = styles::SparkPlot()) {
                // Tooltip off: a 40px-tall chart has nowhere to put one, and
                // the number above it is already the readout.
                Chart(spec = spark, value_transition = VALUE_GLIDE)
            }
        }
    }
}

// `ui!` always closes a component's props with `..Default::default()`, which
// clippy flags when a call site happens to specify every field — as the
// three-prop `SparkCard` does. A macro artifact, not a real redundancy.
#[allow(clippy::needless_update)]
pub fn app() -> Element {
    install_idea_theme(light_theme());

    let kind = signal(Kind::Line);
    let seed = signal(1_u64);
    let show_costs = signal(true);
    let dim = signal(false);
    let fill_idx = signal(0_usize);
    let selected = signal(Vec::<DatumRef>::new());
    // Remembered so "Pin" can promote the last hovered column to a
    // selection without the chart needing a press handler of its own.
    let last_hover = signal(None::<DatumRef>);
    let threshold_on = signal(false);
    let annotate = signal(false);
    // Slice selection for the polar charts. Indices, not `DatumRef`s: a
    // radial chart has one flat list, so there is no series to name.
    let slice_selected = signal(Vec::<usize>::new());
    // Hoisted ONCE, outside the reactive expression that rebuilds the spec —
    // the identity of this Rc is what lets successive specs compare equal.
    let threshold: StyleFn = Rc::new(|ctx: &MarkContext| {
        if ctx.datum.y >= 10.0 {
            MarkOverride::color(Color::rgb(0xe2, 0x4a, 0x33))
        } else {
            MarkOverride::default()
        }
    });
    // What the pointer is over. Driven by the chart's `on_hover`, which is
    // the ONLY hover mechanism the SDK offers — the readout and the tooltip
    // below are both just consumers of it.
    let readout = signal(String::from("Hover the chart…"));
    // The live hover, kept so the tooltip can render from it. Separate from
    // `readout` because the tooltip needs the geometry, not the prose.
    let tip = signal(None::<(Vec<String>, f32, f32)>);
    let placement = signal(Placement::default());

    let on_hover: Rc<dyn Fn(Option<ChartHover>)> = Rc::new(move |h| match h {
        Some(h) => {
            let parts: Vec<String> = h
                .entries
                .iter()
                .map(|e| {
                    let name = if e.series == 0 { "revenue" } else { "costs" };
                    format!("{name} {:.1}", e.datum.y)
                })
                .collect();
            let first = &h.entries[0];
            last_hover.set(Some(DatumRef { series: first.series, index: first.index }));
            readout.set(format!("x={:.0}   ·   {}", first.datum.x, parts.join("   ·   ")));

            // The tooltip. `place` is a pure function of the frame and the
            // entries; swapping the mode swaps the behaviour with no change
            // to the chart.
            let (x, y) = place(placement.get(), &h);
            let mut lines = vec![format!("x = {:.0}", first.datum.x)];
            lines.extend(parts);
            tip.set(Some((lines, x, y)));
        }
        None => {
            // Deliberately does NOT clear `last_hover`. Moving the pointer
            // toward the Pin button leaves the chart and fires this — so
            // clearing here would mean the button always found nothing.
            readout.set(String::from("Hover the chart…"));
            tip.set(None);
        }
    });

    let on_polar_hover: Rc<dyn Fn(Option<PolarHover>)> = Rc::new(move |h| match h {
        Some(h) => {
            readout.set(format!("{}   ·   {:.1}", h.label, h.value));
            // A slice has no meaningful "track x", so the polar path always
            // snaps to the wedge's centroid. Same frame, same conversion.
            let v = h.at.to_viewport(h.hit.position);
            tip.set(Some((
                vec![h.label.clone(), format!("{:.1}", h.value)],
                v.x + 12.0,
                v.y - 16.0,
            )));
        }
        None => {
            readout.set(String::from("Hover the chart…"));
            tip.set(None);
        }
    });

    // The chart area is a `switch` on the FAMILY rather than an `if` chain,
    // because the three components take different spec types — swapping
    // between them is a structural rebuild, which is exactly what `switch`
    // keys on. Keying on the family and not the kind means moving between
    // Donut and Pie reuses the same `PieChart` and animates, instead of
    // tearing it down and remounting.
    let chart_area = switch(
        move || kind.get().family(),
        move |family: &Family| {
            let on_hover = on_hover.clone();
            let on_polar_hover = on_polar_hover.clone();
            let threshold = threshold.clone();
            match family {
                Family::Cartesian => ui! {
                    Chart(
                        spec = rx!(build_spec(
                            kind.get(),
                            seed.get(),
                            show_costs.get(),
                            FILLS[fill_idx.get() % FILLS.len()].1(),
                            dim.get(),
                            selected.get(),
                            // `.clone()` bumps a refcount; the POINTER is
                            // unchanged, so specs still compare equal.
                            threshold_on.get().then(|| threshold.clone()),
                            annotate.get(),
                        )),
                        dim_others = rx!(dim.get()),
                        value_transition = VALUE_GLIDE,
                        color_transition = COLOR_FADE,
                        on_hover = on_hover.clone(),
                    )
                },
                Family::Pie => ui! {
                    PieChart(
                        spec = rx!(build_pie(kind.get(), seed.get(), slice_selected.get())),
                        dim_others = rx!(dim.get()),
                        value_transition = VALUE_GLIDE,
                        color_transition = COLOR_FADE,
                        on_hover = on_polar_hover.clone(),
                    )
                },
                Family::Radial => ui! {
                    RadialChart(
                        spec = rx!(build_radial(kind.get(), seed.get(), slice_selected.get())),
                        dim_others = rx!(dim.get()),
                        value_transition = VALUE_GLIDE,
                        color_transition = COLOR_FADE,
                        on_hover = on_polar_hover.clone(),
                    )
                },
            }
            .into_element()
        },
    );

    // The tooltip lives at the app root, NOT inside the chart — which is
    // what lets it sit over the axis gutters and the controls. A surface
    // rendered inside the plot would be clipped by the plot's own
    // `overflow: hidden`, the clip that keeps marks off the gutters.
    let tooltip = when(
        move || tip.get().is_some(),
        move || tooltip_surface(tip),
        // The closed branch MUST be out of flow too. Both branches occupy the
        // same child slot in the root's flex column, and that column has a
        // `gap`: an in-flow empty view contributes a gap slot while the
        // absolute bubble does not, so the entire page shifts by one gap
        // every time the pointer enters or leaves the chart.
        || {
            runtime_core::view(Vec::new())
                .with_style(std::rc::Rc::new(StyleSheet::r#static(StyleRules {
                    position: Some(Position::Absolute),
                    ..Default::default()
                })))
                .into_element()
        },
    );

    ui! {
        view(style = styles::Root()) {
            text(style = styles::Title()) { "charts SDK".to_string() }
            text(style = styles::Subtitle()) {
                "One spec, one mark IR — Canvas2D on web, CoreGraphics on macOS.".to_string()
            }

            view(style = styles::Controls()) {
                for k in Kind::ALL {
                    KindButton(kind_of = k, active = kind)
                }
            }

            view(style = styles::Controls()) {
                Button(
                    label = "New data".to_string(),
                    tone = tone::Neutral,
                    variant = variant::Soft,
                    on_click = Rc::new(move || seed.set(seed.get().wrapping_add(7))) as Rc<dyn Fn()>,
                )
                Button(
                    label = rx!(if show_costs.get() {
                        "Hide costs".to_string()
                    } else {
                        "Show costs".to_string()
                    }),
                    tone = tone::Neutral,
                    variant = variant::Soft,
                    on_click = Rc::new(move || show_costs.set(!show_costs.get())) as Rc<dyn Fn()>,
                )
                Button(
                    label = rx!(if dim.get() { "Dim: on".to_string() } else { "Dim: off".to_string() }),
                    tone = tone::Neutral,
                    variant = variant::Soft,
                    on_click = Rc::new(move || dim.set(!dim.get())) as Rc<dyn Fn()>,
                )
                Button(
                    label = rx!(FILLS[fill_idx.get() % FILLS.len()].0.to_string()),
                    tone = tone::Neutral,
                    variant = variant::Soft,
                    on_click = Rc::new(move || fill_idx.set((fill_idx.get() + 1) % FILLS.len()))
                        as Rc<dyn Fn()>,
                )
                Button(
                    label = rx!(if threshold_on.get() {
                        "Threshold: on".to_string()
                    } else {
                        "Threshold: off".to_string()
                    }),
                    tone = tone::Neutral,
                    variant = variant::Soft,
                    on_click = Rc::new(move || threshold_on.set(!threshold_on.get()))
                        as Rc<dyn Fn()>,
                )
                Button(
                    label = rx!(if annotate.get() {
                        "Markers: on".to_string()
                    } else {
                        "Markers: off".to_string()
                    }),
                    tone = tone::Neutral,
                    variant = variant::Soft,
                    on_click = Rc::new(move || annotate.set(!annotate.get())) as Rc<dyn Fn()>,
                )
                Button(
                    label = rx!(placement.get().label().to_string()),
                    tone = tone::Neutral,
                    variant = variant::Soft,
                    on_click = Rc::new(move || {
                        let i = Placement::ALL.iter().position(|p| *p == placement.get());
                        let next = (i.unwrap_or(0) + 1) % Placement::ALL.len();
                        placement.set(Placement::ALL[next]);
                    }) as Rc<dyn Fn()>,
                )
                Button(
                    label = rx!(if selected.get().is_empty() {
                        "Pin hovered".to_string()
                    } else {
                        "Unpin".to_string()
                    }),
                    tone = tone::Neutral,
                    variant = variant::Soft,
                    on_click = Rc::new(move || {
                        if selected.get().is_empty() {
                            if let Some(d) = last_hover.get() {
                                selected.set(vec![d]);
                            }
                        } else {
                            selected.set(Vec::new());
                        }
                    }) as Rc<dyn Fn()>,
                )
            }

            view(style = styles::ChartBox()) {
                chart_area
            }

            text(style = styles::Readout()) { readout }

            tooltip

            view(style = styles::SparkRow()) {
                for (i, (name, color)) in SPARKS.iter().enumerate() {
                    SparkCard(
                        label = name.to_string(),
                        color = *color,
                        seed = rx!(seed.get().wrapping_add(i as u64 * 31)),
                    )
                }
            }
        }
    }
}

/// Local sheets. The demo styles itself rather than leaning on idea-ui
/// layout components, so the chart's own sizing behavior is unambiguous:
/// the chart fills `ChartBox`, and `ChartBox` is the only thing giving it a
/// definite height.
mod styles {
    use idea_ui::IdeaThemeRef;
    use runtime_core::{stylesheet, AlignItems, FlexDirection, FlexWrap, Length};

    stylesheet! {
        pub Root<IdeaThemeRef> {
            base(t) {
                flex_direction: FlexDirection::Column,
                gap: t.spacing.sm(),
                padding: t.spacing.lg(),
                width: Length::pct(100.0),
                height: Length::pct(100.0),
                background: t.color.background(),
            }
        }
    }

    stylesheet! {
        pub Title<IdeaThemeRef> {
            base(t) {
                font_size: Length::Px(22.0),
                color: t.color.text(),
            }
        }
    }

    stylesheet! {
        pub Subtitle<IdeaThemeRef> {
            base(t) {
                font_size: Length::Px(13.0),
                color: t.color.text_muted(),
            }
        }
    }

    stylesheet! {
        pub Controls<IdeaThemeRef> {
            base(t) {
                flex_direction: FlexDirection::Row,
                flex_wrap: FlexWrap::Wrap,
                align_items: AlignItems::Center,
                gap: t.spacing.xs(),
            }
        }
    }

    // The chart fills its parent, so the parent is what must have a definite
    // size. `flex_grow` + a min-height keeps it visible on a short window
    // instead of collapsing to zero — a zero-area plot renders nothing by
    // design (see charts-core's `zero_area_plot_renders_no_marks`).
    stylesheet! {
        pub ChartBox<IdeaThemeRef> {
            base(t) {
                flex_grow: 1.0,
                min_height: Length::Px(260.0),
                width: Length::pct(100.0),
                padding: t.spacing.sm(),
                background: t.color.surface(),
                border_radius: t.radius.md(),
            }
        }
    }

    stylesheet! {
        pub SparkRow<IdeaThemeRef> {
            base(t) {
                flex_direction: FlexDirection::Row,
                gap: t.spacing.sm(),
                width: Length::pct(100.0),
                flex_shrink: 0.0,
            }
        }
    }

    stylesheet! {
        pub SparkCard<IdeaThemeRef> {
            base(t) {
                flex_direction: FlexDirection::Column,
                flex_grow: 1.0,
                flex_basis: Length::Px(0.0),
                gap: t.spacing.xs(),
                padding: t.spacing.sm(),
                background: t.color.surface(),
                border_radius: t.radius.md(),
            }
        }
    }

    stylesheet! {
        pub SparkLabel<IdeaThemeRef> {
            base(t) {
                font_size: Length::Px(11.0),
                color: t.color.text_muted(),
            }
        }
    }

    stylesheet! {
        pub SparkValue<IdeaThemeRef> {
            base(t) {
                font_size: Length::Px(18.0),
                color: t.color.text(),
            }
        }
    }

    // A sparkline needs a definite height like any other chart — it is just
    // a small one. 40px is enough to read a trend and not enough to invite
    // an axis.
    stylesheet! {
        pub SparkPlot<IdeaThemeRef> {
            base(_t) {
                height: Length::Px(40.0),
                width: Length::pct(100.0),
            }
        }
    }

    stylesheet! {
        pub Readout<IdeaThemeRef> {
            base(t) {
                font_size: Length::Px(13.0),
                color: t.color.text(),
            }
        }
    }
}
