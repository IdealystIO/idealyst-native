//! The chart specification: what to draw, independent of where.
//!
//! A `ChartSpec` is plain owned data with no callbacks and no borrows, so a
//! host can hold it in a signal, diff it, and re-render on change without
//! lifetime gymnastics. That is a deliberate contrast with plotters, whose
//! `ChartContext<'a, DB, CT>` threads the backend and a lifetime through
//! every builder call and cannot outlive the drawing area it was built
//! against.

use std::rc::Rc;

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

/// Marker shape for a data point.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PointShape {
    #[default]
    Circle,
    Square,
    RoundedSquare,
}

/// An outline drawn around a point marker.
///
/// The usual reason to want one is a marker sitting on top of the line that
/// produced it: a ring in the surface color separates the two so the marker
/// reads as a distinct node rather than a bulge in the stroke.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Ring {
    pub color: Color,
    pub width: f32,
}

/// How point markers are drawn — for a scatter series, and for the optional
/// markers on a line.
///
/// The three radii are separate values rather than a base plus a multiplier
/// because emphasis is not uniformly proportional: a 3px scatter dot wants a
/// large relative jump to be noticeable, while a 10px marker wants a small
/// one. Setting them independently also lets a series opt out of emphasis
/// entirely by making them equal.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PointStyle {
    /// Radius at rest.
    pub radius: f32,
    /// Radius while the point is hovered (see [`Highlight::column`]).
    pub hover_radius: f32,
    /// Radius while the point is selected (see [`Highlight::points`]).
    pub selected_radius: f32,
    pub shape: PointShape,
    /// Marker fill. `None` uses the series color.
    pub fill: Option<Color>,
    /// Optional outline.
    pub ring: Option<Ring>,
}

impl Default for PointStyle {
    fn default() -> Self {
        Self {
            radius: 3.0,
            hover_radius: 5.5,
            selected_radius: 6.5,
            shape: PointShape::Circle,
            fill: None,
            ring: None,
        }
    }
}

impl PointStyle {
    /// A marker of `radius`, with emphasis radii scaled from it.
    pub fn new(radius: f32) -> Self {
        Self {
            radius,
            hover_radius: radius * 1.8,
            selected_radius: radius * 2.1,
            ..Default::default()
        }
    }

    pub fn hover(mut self, radius: f32) -> Self {
        self.hover_radius = radius;
        self
    }

    pub fn selected(mut self, radius: f32) -> Self {
        self.selected_radius = radius;
        self
    }

    pub fn shape(mut self, shape: PointShape) -> Self {
        self.shape = shape;
        self
    }

    pub fn fill(mut self, color: Color) -> Self {
        self.fill = Some(color);
        self
    }

    pub fn ring(mut self, color: Color, width: f32) -> Self {
        self.ring = Some(Ring { color, width });
        self
    }

    /// Radius for a given emphasis level.
    pub fn radius_for(&self, emphasis: Emphasis) -> f32 {
        match emphasis {
            Emphasis::None => self.radius,
            Emphasis::Hovered => self.hover_radius,
            Emphasis::Selected => self.selected_radius,
        }
    }
}

/// How a line is stroked.
#[derive(Clone, PartialEq, Debug)]
pub struct LineStyle {
    pub width: f32,
    /// Stroke width while any point in the series is emphasised. Equal to
    /// `width` means "do not thicken".
    pub hover_width: f32,
    /// Monotone-cubic interpolation instead of straight segments.
    pub smooth: bool,
    /// Dash pattern; empty is solid. Used for projections and targets.
    pub dash: Vec<f32>,
    /// Draw a marker at each datum.
    pub points: Option<PointStyle>,
}

impl Default for LineStyle {
    fn default() -> Self {
        Self {
            width: 2.0,
            hover_width: 3.0,
            smooth: false,
            dash: Vec::new(),
            points: None,
        }
    }
}

impl LineStyle {
    pub fn new(width: f32) -> Self {
        Self { width, hover_width: width * 1.5, ..Default::default() }
    }

    pub fn smooth(mut self) -> Self {
        self.smooth = true;
        self
    }

    pub fn hover_width(mut self, width: f32) -> Self {
        self.hover_width = width;
        self
    }

    pub fn dashed(mut self, pattern: impl Into<Vec<f32>>) -> Self {
        self.dash = pattern.into();
        self
    }

    pub fn with_points(mut self, points: PointStyle) -> Self {
        self.points = Some(points);
        self
    }

    pub fn width_for(&self, emphasis: Emphasis) -> f32 {
        match emphasis {
            Emphasis::None => self.width,
            _ => self.hover_width,
        }
    }
}

/// How the region between an area's line and its baseline is painted.
#[derive(Clone, PartialEq, Debug)]
pub enum AreaFill {
    /// One flat color at `opacity` (0..=1) of the series color.
    Flat { opacity: f32 },
    /// A vertical fade from the line down to the baseline. Both ends are
    /// opacities (0..=1) of the series color, so a fill always stays in the
    /// series' hue without the caller restating it.
    Gradient { top_opacity: f32, bottom_opacity: f32 },
    /// Fully explicit stops, for a fill that is not a simple fade — a
    /// threshold band, a two-tone gradient, a brand ramp.
    Stops(Vec<crate::scene::GradientStop>),
    /// No fill; the area renders as a bare line.
    None,
}

impl Default for AreaFill {
    fn default() -> Self {
        AreaFill::Gradient { top_opacity: 0.43, bottom_opacity: 0.0 }
    }
}

/// An area series: a line plus the fill beneath it.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct AreaStyle {
    pub line: LineStyle,
    pub fill: AreaFill,
}

impl AreaStyle {
    pub fn new(line: LineStyle, fill: AreaFill) -> Self {
        Self { line, fill }
    }

    pub fn fill(mut self, fill: AreaFill) -> Self {
        self.fill = fill;
        self
    }

    pub fn line(mut self, line: LineStyle) -> Self {
        self.line = line;
        self
    }
}

/// How bars are drawn.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct BarStyle {
    /// Corner radius on the outer end. In a stack only the outermost
    /// segment is rounded, so an interior segment never shows a seam.
    pub radius: f32,
    /// Fill while emphasised. `None` leaves the bar's own color.
    pub hover_color: Option<Color>,
    /// Fraction of its slot the bar (or group) occupies, 0..=1. `None`
    /// defers to [`ChartSpec::bar_group_padding`], so the common case of
    /// "same spacing everywhere" stays a single setting.
    pub width_fraction: Option<f32>,
}

impl Default for BarStyle {
    fn default() -> Self {
        Self { radius: 4.0, hover_color: None, width_fraction: None }
    }
}

impl BarStyle {
    pub fn new(radius: f32) -> Self {
        Self { radius, ..Default::default() }
    }

    pub fn hover_color(mut self, color: Color) -> Self {
        self.hover_color = Some(color);
        self
    }

    pub fn width_fraction(mut self, f: f32) -> Self {
        self.width_fraction = Some(f);
        self
    }
}

/// How a series is drawn.
///
/// Each variant carries a style struct rather than loose fields, so a new
/// knob is one field on one struct instead of a change to every match arm
/// and every call site that spelled the variant out.
#[derive(Clone, PartialEq, Debug)]
pub enum SeriesKind {
    Line(LineStyle),
    Area(AreaStyle),
    Bar(BarStyle),
    Scatter(PointStyle),
}

impl SeriesKind {
    pub fn line() -> Self {
        SeriesKind::Line(LineStyle::default())
    }

    pub fn smooth_line() -> Self {
        SeriesKind::Line(LineStyle::default().smooth())
    }

    pub fn area() -> Self {
        SeriesKind::Area(AreaStyle::default())
    }

    pub fn bar() -> Self {
        SeriesKind::Bar(BarStyle::default())
    }

    pub fn scatter() -> Self {
        SeriesKind::Scatter(PointStyle::default())
    }

    pub(crate) fn is_bar(&self) -> bool {
        matches!(self, SeriesKind::Bar(_))
    }
}

/// How strongly one datum is emphasised.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Emphasis {
    #[default]
    None,
    Hovered,
    Selected,
}

/// A reference to one plotted datum.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DatumRef {
    /// Index into [`ChartSpec::series`] — the full list, including hidden
    /// series, so a reference stays valid across a visibility toggle.
    pub series: usize,
    pub index: usize,
}

/// Which data points are currently emphasised, and how the rest respond.
///
/// Lives on the spec rather than being a separate render argument, so the
/// whole input to a render is still ONE comparable value: goldens stay exact,
/// and a host memoising on the spec picks up emphasis changes for free.
///
/// It is also author-settable, not just hover-driven — pinning a value,
/// highlighting an anomaly, or linking two charts all work by writing this
/// directly, with no pointer involved.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Highlight {
    /// Emphasise every datum at this data-x — the hovered column. Keyed on
    /// the DATA x, not a pixel, so it changes only when the pointer crosses
    /// into a new column and a host can memoise on it safely.
    pub column: Option<f64>,
    /// Emphasise specific points — selection, pins, annotations.
    pub points: Vec<DatumRef>,
    /// Fade series that contain no emphasised datum, so the highlighted one
    /// stands out instead of merely being slightly bigger.
    pub dim_others: bool,
    /// Opacity multiplier applied by `dim_others`, 0..=1.
    pub dim_opacity: f32,
}

impl Highlight {
    /// Emphasise a whole column, the shape a tooltip hover produces.
    pub fn column(x: f64) -> Self {
        Self { column: Some(x), dim_opacity: 0.35, ..Default::default() }
    }

    pub fn with_points(mut self, points: Vec<DatumRef>) -> Self {
        self.points = points;
        self
    }

    pub fn dim_others(mut self, on: bool) -> Self {
        self.dim_others = on;
        if self.dim_opacity <= 0.0 {
            self.dim_opacity = 0.35;
        }
        self
    }

    pub fn is_empty(&self) -> bool {
        self.column.is_none() && self.points.is_empty()
    }

    /// Emphasis level for one datum.
    pub fn of(&self, series: usize, index: usize, x: f64) -> Emphasis {
        if self.points.iter().any(|p| p.series == series && p.index == index) {
            return Emphasis::Selected;
        }
        match self.column {
            Some(cx) if cx == x => Emphasis::Hovered,
            _ => Emphasis::None,
        }
    }

    /// Whether a series contains any emphasised datum — what decides
    /// line thickening and whether `dim_others` fades it.
    pub fn touches_series(&self, series: usize, data: &[Datum]) -> bool {
        if self.points.iter().any(|p| p.series == series) {
            return true;
        }
        match self.column {
            Some(cx) => data.iter().any(|d| d.x == cx),
            None => false,
        }
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

/// What a [`StyleFn`] is told about the mark it is styling.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct MarkContext {
    /// Index into [`ChartSpec::series`].
    pub series: usize,
    /// Index into that series' `data`.
    pub index: usize,
    pub datum: Datum,
    /// Emphasis this mark already resolved to, so a callback can build ON
    /// the hover/selection state rather than having to reimplement it.
    pub emphasis: Emphasis,
    /// The color the mark would have used, after emphasis and dimming.
    pub base_color: Color,
}

/// Per-mark overrides returned by a [`StyleFn`]. Every field is optional;
/// `None` keeps what the series style already resolved.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct MarkOverride {
    pub color: Option<Color>,
    /// Point/marker radius. Ignored by bars.
    pub radius: Option<f32>,
    /// Multiplier on the resolved alpha, 0..=1.
    pub opacity: Option<f32>,
}

impl MarkOverride {
    pub fn color(c: Color) -> Self {
        Self { color: Some(c), ..Default::default() }
    }

    pub fn radius(r: f32) -> Self {
        Self { radius: Some(r), ..Default::default() }
    }

    pub fn opacity(o: f32) -> Self {
        Self { opacity: Some(o), ..Default::default() }
    }

    pub fn with_color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }

    pub fn with_radius(mut self, r: f32) -> Self {
        self.radius = Some(r);
        self
    }
}

/// A callback that styles individual marks — conditional formatting.
///
/// Applies to PER-DATUM marks: bars and point markers. A line is one stroke
/// for the whole series, so there is no per-datum question to answer there;
/// its color and its emphasis width live on the series and [`LineStyle`].
///
/// # Identity, and the trap
///
/// [`ChartSpec`] is `PartialEq` so a host can memoise on it, and a closure
/// has no meaningful equality — so two specs' style functions compare by
/// `Rc::ptr_eq`. That makes IDENTITY the thing that matters: build the `Rc`
/// ONCE and clone it into each spec. Constructing a fresh closure inside the
/// expression that rebuilds the spec produces a new pointer every time, the
/// specs never compare equal, and the chart re-renders on every reactive
/// tick instead of only when something changed.
pub type StyleFn = Rc<dyn Fn(&MarkContext) -> MarkOverride>;

/// One named, colored series.
#[derive(Clone)]
pub struct Series {
    pub name: String,
    pub kind: SeriesKind,
    pub color: Color,
    pub data: Vec<Datum>,
    /// Optional per-mark styling. See [`StyleFn`] — note the identity rule.
    pub style_fn: Option<StyleFn>,
    /// Hidden series keep their color slot and legend entry but draw
    /// nothing — that is what a legend toggle needs, and dropping the
    /// series from the vec instead would reshuffle every other color.
    pub visible: bool,
}

// Hand-written: `style_fn` is a closure, which has no structural equality.
// Comparing by pointer keeps `ChartSpec: PartialEq` — and therefore host
// memoisation and exact goldens — while still noticing when the callback is
// swapped for a different one. See [`StyleFn`] for why identity matters.
impl PartialEq for Series {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.kind == other.kind
            && self.color == other.color
            && self.data == other.data
            && self.visible == other.visible
            && match (&self.style_fn, &other.style_fn) {
                (None, None) => true,
                (Some(a), Some(b)) => Rc::ptr_eq(a, b),
                _ => false,
            }
    }
}

impl std::fmt::Debug for Series {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Series")
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("color", &self.color)
            .field("data", &self.data)
            .field("visible", &self.visible)
            .field("style_fn", &self.style_fn.is_some())
            .finish()
    }
}

impl Series {
    pub fn new(name: impl Into<String>, kind: SeriesKind, color: Color, data: Vec<Datum>) -> Self {
        Self { name: name.into(), kind, color, data, style_fn: None, visible: true }
    }

    /// Attach per-mark styling. Hoist the `Rc` — see [`StyleFn`].
    pub fn styled(mut self, f: StyleFn) -> Self {
        self.style_fn = Some(f);
        self
    }

    /// Resolve the override for one mark, or the default when there is no
    /// callback.
    pub(crate) fn override_for(&self, ctx: &MarkContext) -> MarkOverride {
        match &self.style_fn {
            Some(f) => f(ctx),
            None => MarkOverride::default(),
        }
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
    /// Which points are emphasised. See [`Highlight`].
    pub highlight: Highlight,
    /// Translucent band painted behind the highlighted column, spanning the
    /// plot's full height.
    ///
    /// This is the affordance a bar chart actually wants on hover: emphasising
    /// the bar alone tells you which bar, but the band tells you which SLOT —
    /// it covers the whole category including the gaps between grouped bars,
    /// so the pointer does not have to be over a bar for the column to read as
    /// active. `None` draws no band.
    pub highlight_band: Option<Color>,
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
            highlight: Highlight::default(),
            highlight_band: None,
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

    pub fn highlight(mut self, h: Highlight) -> Self {
        self.highlight = h;
        self
    }

    /// Paint a band behind the highlighted column. See
    /// [`ChartSpec::highlight_band`].
    pub fn highlight_band(mut self, color: Color) -> Self {
        self.highlight_band = Some(color);
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
            .any(|s| s.visible && s.kind.is_bar())
    }
}
