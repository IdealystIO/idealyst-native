//! The demo screen: a chart, a row of controls, and a live hover readout.

use std::rc::Rc;

use charts::prelude::*;
use charts::{ChartHover, DatumRef, MarkContext, MarkOverride, StyleFn};
use idea_ui::{install_idea_theme, light_theme, tone, variant, Button, VariantRef};
use runtime_core::{component, rx, signal, ui, Element, Signal};

/// Which chart the demo is showing. One enum drives both the button row and
/// the spec builder, so adding a kind is a single edit in each.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Line,
    Smooth,
    Area,
    Bars,
    Stacked,
    Scatter,
}

impl Default for Kind {
    fn default() -> Self {
        Kind::Line
    }
}

impl Kind {
    const ALL: [Kind; 6] =
        [Kind::Line, Kind::Smooth, Kind::Area, Kind::Bars, Kind::Stacked, Kind::Scatter];

    fn label(self) -> &'static str {
        match self {
            Kind::Line => "Line",
            Kind::Smooth => "Smooth",
            Kind::Area => "Area",
            Kind::Bars => "Bars",
            Kind::Stacked => "Stacked",
            Kind::Scatter => "Scatter",
        }
    }

    /// Bar kinds share the categorical x axis; the rest are continuous.
    fn is_bar(self) -> bool {
        matches!(self, Kind::Bars | Kind::Stacked)
    }
}

const BLUE: Color = Color::rgb(0x4c, 0x8d, 0xff);
const PINK: Color = Color::rgb(0xff, 0x6b, 0x9d);
const MONTHS: [&str; 8] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug"];

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
fn build_spec(
    kind: Kind,
    seed: u64,
    show_costs: bool,
    fill: AreaFill,
    dim: bool,
    selected: Vec<DatumRef>,
    style_fn: Option<StyleFn>,
) -> ChartSpec {
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

    let x_axis = if kind.is_bar() { Axis::category(MONTHS) } else { Axis::linear() };

    let mut highlight = Highlight::default().with_points(selected);
    highlight.dim_others = dim;

    ChartSpec::new(vec![revenue, costs])
        .x(x_axis)
        .y(Axis::linear().include_zero(true).title("USD (k)"))
        .bars(if kind == Kind::Stacked { BarLayout::Stacked } else { BarLayout::Grouped })
        .legend(true)
        .highlight(highlight)
        // Band behind the hovered column, so the whole category reads as
        // active even when the pointer is in the gap between grouped bars.
        .highlight_band(Color::rgba(0x33, 0x33, 0x33, 18))
}

/// The area fill modes the demo cycles through.
const FILLS: [(&str, fn() -> AreaFill); 4] = [
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
                    )),
                    dim_others = rx!(dim.get()),
                    transition_ms = 420,
                    on_hover = on_hover.clone(),
                )
            }

            text(style = styles::Readout()) { readout }
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
        pub Readout<IdeaThemeRef> {
            base(t) {
                font_size: Length::Px(13.0),
                color: t.color.text(),
            }
        }
    }
}
