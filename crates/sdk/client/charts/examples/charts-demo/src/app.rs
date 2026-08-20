//! The demo screen: a chart, a row of controls, and a live hover readout.

use std::rc::Rc;

use charts::prelude::*;
use charts::{ChartHover, DatumRef, MarkContext, MarkOverride, PolarHover, StyleFn};
use idea_ui::{install_idea_theme, light_theme, tone, variant, Button, VariantRef};
use runtime_core::{component, rx, signal, switch, ui, Element, IntoElement, Signal};

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
                Chart(spec = spark, tooltip = false, transition_ms = 420)
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
    // What the pointer is over. Driven by the chart's `on_hover`, which
    // fires whether or not the built-in tooltip is showing — the point being
    // that an app can render its own readout from the same data.
    let readout = signal(String::from("Hover the chart…"));

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
        }
        None => {
            // Deliberately does NOT clear `last_hover`. Moving the pointer
            // toward the Pin button leaves the chart and fires this — so
            // clearing here would mean the button always found nothing.
            readout.set(String::from("Hover the chart…"));
        }
    });

    let on_polar_hover: Rc<dyn Fn(Option<PolarHover>)> = Rc::new(move |h| match h {
        Some(h) => readout.set(format!("{}   ·   {:.1}", h.label, h.value)),
        None => readout.set(String::from("Hover the chart…")),
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
                        transition_ms = 420,
                        on_hover = on_hover.clone(),
                    )
                },
                Family::Pie => ui! {
                    PieChart(
                        spec = rx!(build_pie(kind.get(), seed.get(), slice_selected.get())),
                        dim_others = rx!(dim.get()),
                        transition_ms = 420,
                        on_hover = on_polar_hover.clone(),
                    )
                },
                Family::Radial => ui! {
                    RadialChart(
                        spec = rx!(build_radial(kind.get(), seed.get(), slice_selected.get())),
                        dim_others = rx!(dim.get()),
                        transition_ms = 420,
                        on_hover = on_polar_hover.clone(),
                    )
                },
            }
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
