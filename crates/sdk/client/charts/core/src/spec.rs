//! The chart specification: what to draw, independent of where.
//!
//! A `ChartSpec` is plain owned data with no callbacks and no borrows, so a
//! host can hold it in a signal, diff it, and re-render on change without
//! lifetime gymnastics. That is a deliberate contrast with plotters, whose
//! `ChartContext<'a, DB, CT>` threads the backend and a lifetime through
//! every builder call and cannot outlive the drawing area it was built
//! against.

use crate::scene::Color;

/// One data point. Categorical axes carry the category index in `x`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Datum {
    pub x: f64,
    pub y: f64,
}

pub const fn datum(x: f64, y: f64) -> Datum {
    Datum { x, y }
}

/// How a series is drawn.
#[derive(Clone, PartialEq, Debug)]
pub enum SeriesKind {
    Line {
        width: f32,
        /// Monotone-cubic interpolation instead of straight segments.
        smooth: bool,
        /// Dash pattern; empty is solid. Used for projections/targets.
        dash: Vec<f32>,
        /// Draw a marker at each datum.
        points: bool,
    },
    /// A line with the region between it and the baseline filled.
    Area {
        width: f32,
        smooth: bool,
        /// Fade the fill toward the baseline rather than using flat color.
        gradient: bool,
    },
    Bar {
        /// Corner radius applied to the two outer corners only.
        radius: f32,
    },
    Scatter {
        radius: f32,
    },
}

impl SeriesKind {
    pub fn line() -> Self {
        SeriesKind::Line { width: 2.0, smooth: false, dash: Vec::new(), points: false }
    }

    pub fn smooth_line() -> Self {
        SeriesKind::Line { width: 2.0, smooth: true, dash: Vec::new(), points: false }
    }

    pub fn area() -> Self {
        SeriesKind::Area { width: 2.0, smooth: false, gradient: true }
    }

    pub fn bar() -> Self {
        SeriesKind::Bar { radius: 4.0 }
    }

    pub fn scatter() -> Self {
        SeriesKind::Scatter { radius: 3.0 }
    }
}

/// How multiple bar series share the x axis.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum BarLayout {
    /// Side-by-side within each category slot.
    #[default]
    Grouped,
    /// Summed, each series stacked on the previous.
    Stacked,
}

/// One named, colored series.
#[derive(Clone, PartialEq, Debug)]
pub struct Series {
    pub name: String,
    pub kind: SeriesKind,
    pub color: Color,
    pub data: Vec<Datum>,
    /// Hidden series keep their color slot and legend entry but draw
    /// nothing — that is what a legend toggle needs, and dropping the
    /// series from the vec instead would reshuffle every other color.
    pub visible: bool,
}

impl Series {
    pub fn new(name: impl Into<String>, kind: SeriesKind, color: Color, data: Vec<Datum>) -> Self {
        Self { name: name.into(), kind, color, data, visible: true }
    }
}

/// The visible range of an axis.
///
/// Split from the data extent on purpose. Pan and zoom are viewport
/// operations, not data operations: they set `Fixed` and everything
/// downstream — tick selection, mark positions, the hit index — follows
/// with no other change anywhere in the pipeline. Keeping the viewport
/// implicit in the data would make those additions a rewrite.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum Domain {
    /// Fit the data, rounded outward to tick boundaries.
    #[default]
    Auto,
    /// An explicit window. This is what pan/zoom writes.
    Fixed { min: f64, max: f64 },
}

impl Domain {
    pub const fn fixed(min: f64, max: f64) -> Self {
        Domain::Fixed { min, max }
    }

    /// Slide the window by a fraction of its own width. `Auto` is returned
    /// unchanged — panning a not-yet-resolved domain is meaningless, so the
    /// caller must resolve first (see `ResolvedAxis::domain`).
    pub fn translate(self, by_fraction: f64) -> Domain {
        match self {
            Domain::Auto => Domain::Auto,
            Domain::Fixed { min, max } => {
                let d = (max - min) * by_fraction;
                Domain::Fixed { min: min + d, max: max + d }
            }
        }
    }

    /// Scale the window about a focus expressed as a 0..=1 fraction across
    /// it. `factor < 1` zooms in. The focus point stays put, which is what
    /// makes pinch and scroll-wheel zoom feel anchored.
    pub fn zoom(self, factor: f64, focus_fraction: f64) -> Domain {
        match self {
            Domain::Auto => Domain::Auto,
            Domain::Fixed { min, max } => {
                let focus = min + (max - min) * focus_fraction;
                Domain::Fixed {
                    min: focus - (focus - min) * factor,
                    max: focus + (max - focus) * factor,
                }
            }
        }
    }
}

/// What kind of values an axis carries, and therefore how ticks are chosen
/// and formatted.
#[derive(Clone, PartialEq, Debug, Default)]
pub enum AxisKind {
    #[default]
    Linear,
    /// Base-10 log. Non-positive data is dropped by the scale, since there
    /// is no meaningful position for it.
    Log,
    /// `x` is milliseconds since the Unix epoch.
    Time,
    /// Discrete slots; `x` is the index into this list.
    Category(Vec<String>),
}

/// Axis configuration.
#[derive(Clone, PartialEq, Debug)]
pub struct Axis {
    pub kind: AxisKind,
    pub domain: Domain,
    pub title: Option<String>,
    /// Draw gridlines at this axis's ticks.
    pub grid: bool,
    /// Desired tick count. Advisory — the underlying tick selection rounds
    /// to human-friendly intervals and may return fewer.
    pub tick_count: usize,
    /// Include zero when auto-fitting. Bars are misleading without it;
    /// line charts of a narrow range usually want it off.
    pub include_zero: bool,
}

impl Default for Axis {
    fn default() -> Self {
        Self {
            kind: AxisKind::Linear,
            domain: Domain::Auto,
            title: None,
            grid: true,
            tick_count: 5,
            include_zero: false,
        }
    }
}

impl Axis {
    pub fn linear() -> Self {
        Self::default()
    }

    pub fn time() -> Self {
        Self { kind: AxisKind::Time, ..Default::default() }
    }

    pub fn log() -> Self {
        Self { kind: AxisKind::Log, ..Default::default() }
    }

    pub fn category(labels: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            kind: AxisKind::Category(labels.into_iter().map(Into::into).collect()),
            grid: false,
            ..Default::default()
        }
    }

    pub fn title(mut self, t: impl Into<String>) -> Self {
        self.title = Some(t.into());
        self
    }

    pub fn grid(mut self, on: bool) -> Self {
        self.grid = on;
        self
    }

    pub fn domain(mut self, d: Domain) -> Self {
        self.domain = d;
        self
    }

    pub fn include_zero(mut self, on: bool) -> Self {
        self.include_zero = on;
        self
    }
}

/// A complete chart.
#[derive(Clone, PartialEq, Debug)]
pub struct ChartSpec {
    pub series: Vec<Series>,
    pub x: Axis,
    pub y: Axis,
    pub bar_layout: BarLayout,
    /// Fraction of a category slot left empty between groups, 0..1.
    pub bar_group_padding: f32,
    /// Emit legend label placements.
    pub legend: bool,
    /// Colors for the axis furniture. Text color is the host's business —
    /// labels carry no color unless they are series-tinted — but lines have
    /// to be drawn by us, so they live here.
    pub grid_color: Color,
    pub axis_color: Color,
}

impl Default for ChartSpec {
    fn default() -> Self {
        Self {
            series: Vec::new(),
            x: Axis::default(),
            y: Axis::default(),
            bar_layout: BarLayout::default(),
            bar_group_padding: 0.2,
            legend: false,
            grid_color: Color::rgba(128, 128, 128, 40),
            axis_color: Color::rgba(128, 128, 128, 90),
        }
    }
}

impl ChartSpec {
    pub fn new(series: Vec<Series>) -> Self {
        Self { series, ..Default::default() }
    }

    pub fn x(mut self, a: Axis) -> Self {
        self.x = a;
        self
    }

    pub fn y(mut self, a: Axis) -> Self {
        self.y = a;
        self
    }

    pub fn bars(mut self, l: BarLayout) -> Self {
        self.bar_layout = l;
        self
    }

    pub fn legend(mut self, on: bool) -> Self {
        self.legend = on;
        self
    }

    /// Series that actually draw, paired with their index in `series` so
    /// hit-test results can refer back to the original list.
    pub fn visible(&self) -> impl Iterator<Item = (usize, &Series)> {
        self.series.iter().enumerate().filter(|(_, s)| s.visible)
    }

    pub(crate) fn has_bars(&self) -> bool {
        self.series
            .iter()
            .any(|s| s.visible && matches!(s.kind, SeriesKind::Bar { .. }))
    }
}
