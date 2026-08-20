//! The demo screen: a chart, a row of controls, and a live hover readout.

use std::rc::Rc;

use charts::prelude::*;
use charts::ChartHover;
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
fn build_spec(kind: Kind, seed: u64, show_costs: bool) -> ChartSpec {
    let a = series_values(seed, 2.0);
    let b = series_values(seed.wrapping_add(9), 1.0);

    let series_kind = match kind {
        Kind::Line => SeriesKind::line(),
        Kind::Smooth => SeriesKind::smooth_line(),
        Kind::Area => SeriesKind::area(),
        Kind::Bars | Kind::Stacked => SeriesKind::bar(),
        Kind::Scatter => SeriesKind::scatter(),
    };

    let to_data = |vals: &[f64]| -> Vec<Datum> {
        vals.iter().enumerate().map(|(i, v)| datum(i as f64, *v)).collect()
    };

    let revenue = Series::new("revenue", series_kind.clone(), BLUE, to_data(&a));
    let mut costs = Series::new("costs", series_kind, PINK, to_data(&b));
    // A hidden series keeps its color and its legend slot rather than being
    // removed from the vec — that is exactly what the toggle demonstrates,
    // and dropping it instead would reshuffle every other series' color.
    // Two overlapping gradient fills are unreadable, so an area chart always
    // shows one series.
    costs.visible = show_costs && kind != Kind::Area;

    let x_axis = if kind.is_bar() { Axis::category(MONTHS) } else { Axis::linear() };

    ChartSpec::new(vec![revenue, costs])
        .x(x_axis)
        .y(Axis::linear().include_zero(true).title("USD (k)"))
        .bars(if kind == Kind::Stacked { BarLayout::Stacked } else { BarLayout::Grouped })
        .legend(true)
}

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
            readout.set(format!("x={:.0}   ·   {}", h.entries[0].datum.x, parts.join("   ·   ")));
        }
        None => readout.set(String::from("Hover the chart…")),
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
            }

            view(style = styles::ChartBox()) {
                Chart(
                    spec = rx!(build_spec(kind.get(), seed.get(), show_costs.get())),
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
