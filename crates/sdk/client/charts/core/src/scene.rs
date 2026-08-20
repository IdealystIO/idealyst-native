//! The mark IR: what a chart render produces.
//!
//! This is deliberately a *data* type, not a trait. Plotters puts its seam
//! at a `DrawingBackend` trait and pays for it — every consumer implements
//! ~10 methods, forgotten overrides silently fall back to per-pixel
//! rasterization, and the trait's shape (integer coords, no beziers) leaks
//! into everything upstream. A plain enum has none of those failure modes:
//! a consumer pattern-matches, the compiler enforces exhaustiveness, and
//! the whole output of a render is comparable with `==` so tests are exact
//! goldens rather than pixel diffs.
//!
//! Everything here is `f32`. That is a deliberate departure from plotters'
//! `BackendCoord = (i32, i32)`: integer marks visibly snap between whole
//! logical pixels on a 3x-DPR display and jitter during a resize or pan
//! animation, and no amount of care at the backend fixes it because the
//! quantization happens upstream in the scale.

/// RGBA, 8 bits per channel.
///
/// Field-for-field identical to `runtime_core::color::Rgba` so the idealyst
/// adapter is a plain copy rather than a conversion with rounding behavior
/// of its own. Kept `Eq`/`Hash` (rather than storing float channels) so
/// `ChartScene` compares exactly in golden tests.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const TRANSPARENT: Color = Color { r: 0, g: 0, b: 0, a: 0 };

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Same color at a different opacity — the common case for area fills
    /// under a line, and for de-emphasizing non-hovered series.
    pub const fn with_alpha(self, a: u8) -> Self {
        Self { a, ..self }
    }

    /// Parse `#rgb`, `#rrggbb`, or `#rrggbbaa`. Returns `None` on anything
    /// else; callers decide whether that is a fallback or a panic.
    pub fn parse(s: &str) -> Option<Color> {
        let h = s.strip_prefix('#')?;
        let b = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
        match h.len() {
            3 => {
                let d = |i: usize| {
                    u8::from_str_radix(&h[i..i + 1], 16).ok().map(|v| v * 17)
                };
                Some(Color::rgb(d(0)?, d(1)?, d(2)?))
            }
            6 => Some(Color::rgb(b(0)?, b(2)?, b(4)?)),
            8 => Some(Color::rgba(b(0)?, b(2)?, b(4)?, b(6)?)),
            _ => None,
        }
    }
}

/// A point in plot-local pixel space.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

pub const fn pt(x: f32, y: f32) -> Point {
    Point { x, y }
}

/// An axis-aligned rectangle in pixel space. `y` grows downward, matching
/// every 2D drawing surface we target.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x <= self.right() && p.y >= self.y && p.y <= self.bottom()
    }

    /// Shrink by per-edge insets, clamping to zero so an over-large inset
    /// yields an empty rect rather than a negative-size one that would make
    /// downstream scale math produce infinities.
    pub fn inset(&self, left: f32, top: f32, right: f32, bottom: f32) -> Rect {
        Rect {
            x: self.x + left,
            y: self.y + top,
            w: (self.w - left - right).max(0.0),
            h: (self.h - top - bottom).max(0.0),
        }
    }
}

/// One segment of a path. Mirrors `canvas_core::PathSeg` exactly.
///
/// Cubics are the reason this IR exists rather than reusing plotters'
/// polyline-only `draw_path`: smooth line interpolation, rounded bar caps,
/// and donut arcs are all unrepresentable without them, and pre-flattening
/// to line segments throws away resolution the GPU renderer could have used.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PathSeg {
    MoveTo(Point),
    LineTo(Point),
    QuadTo(Point, Point),
    CubicTo(Point, Point, Point),
    Close,
}

/// A vector path.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Path {
    pub segs: Vec<PathSeg>,
}

/// Magic constant for approximating a quarter circle with a cubic bezier.
/// `4/3 * (sqrt(2) - 1)`. Used by the rounded-rect and circle constructors.
const KAPPA: f32 = 0.552_284_8;

impl Path {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.segs.is_empty()
    }

    pub fn move_to(mut self, x: f32, y: f32) -> Self {
        self.segs.push(PathSeg::MoveTo(pt(x, y)));
        self
    }

    pub fn line_to(mut self, x: f32, y: f32) -> Self {
        self.segs.push(PathSeg::LineTo(pt(x, y)));
        self
    }

    pub fn quad_to(mut self, cx: f32, cy: f32, x: f32, y: f32) -> Self {
        self.segs.push(PathSeg::QuadTo(pt(cx, cy), pt(x, y)));
        self
    }

    pub fn cubic_to(mut self, c1x: f32, c1y: f32, c2x: f32, c2y: f32, x: f32, y: f32) -> Self {
        self.segs
            .push(PathSeg::CubicTo(pt(c1x, c1y), pt(c2x, c2y), pt(x, y)));
        self
    }

    pub fn close(mut self) -> Self {
        self.segs.push(PathSeg::Close);
        self
    }

    pub fn rect(r: Rect) -> Self {
        Path::new()
            .move_to(r.x, r.y)
            .line_to(r.right(), r.y)
            .line_to(r.right(), r.bottom())
            .line_to(r.x, r.bottom())
            .close()
    }

    /// A rounded rectangle with per-corner radii, ordered top-left,
    /// top-right, bottom-right, bottom-left.
    ///
    /// Per-corner rather than uniform because that is exactly what a bar
    /// chart needs: columns round only their top two corners, stacked
    /// segments round only the outermost ones, and a uniform-radius API
    /// forces every caller to hand-build the path instead.
    ///
    /// Radii are clamped to half the shorter side, so an over-large radius
    /// degrades to a pill rather than producing self-intersecting curves.
    pub fn rounded_rect(r: Rect, radii: [f32; 4]) -> Self {
        let lim = (r.w.min(r.h) / 2.0).max(0.0);
        let [tl, tr, br, bl] = [
            radii[0].clamp(0.0, lim),
            radii[1].clamp(0.0, lim),
            radii[2].clamp(0.0, lim),
            radii[3].clamp(0.0, lim),
        ];
        let (l, t, rt, b) = (r.x, r.y, r.right(), r.bottom());
        let mut p = Path::new().move_to(l + tl, t);
        p = p.line_to(rt - tr, t);
        if tr > 0.0 {
            p = p.cubic_to(rt - tr + tr * KAPPA, t, rt, t + tr - tr * KAPPA, rt, t + tr);
        }
        p = p.line_to(rt, b - br);
        if br > 0.0 {
            p = p.cubic_to(rt, b - br + br * KAPPA, rt - br + br * KAPPA, b, rt - br, b);
        }
        p = p.line_to(l + bl, b);
        if bl > 0.0 {
            p = p.cubic_to(l + bl - bl * KAPPA, b, l, b - bl + bl * KAPPA, l, b - bl);
        }
        p = p.line_to(l, t + tl);
        if tl > 0.0 {
            p = p.cubic_to(l, t + tl - tl * KAPPA, l + tl - tl * KAPPA, t, l + tl, t);
        }
        p.close()
    }

    pub fn circle(c: Point, r: f32) -> Self {
        let k = r * KAPPA;
        Path::new()
            .move_to(c.x, c.y - r)
            .cubic_to(c.x + k, c.y - r, c.x + r, c.y - k, c.x + r, c.y)
            .cubic_to(c.x + r, c.y + k, c.x + k, c.y + r, c.x, c.y + r)
            .cubic_to(c.x - k, c.y + r, c.x - r, c.y + k, c.x - r, c.y)
            .cubic_to(c.x - r, c.y - k, c.x - k, c.y - r, c.x, c.y - r)
            .close()
    }
}

/// A gradient color stop. `offset` is 0..=1 along the gradient axis.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct GradientStop {
    pub offset: f32,
    pub color: Color,
}

/// How a shape is colored.
#[derive(Clone, PartialEq, Debug)]
pub enum Paint {
    Solid(Color),
    /// Linear gradient from `(x0,y0)` to `(x1,y1)` in pixel space. The
    /// canonical use is an area fill that fades toward the baseline.
    Linear {
        from: Point,
        to: Point,
        stops: Vec<GradientStop>,
    },
}

impl Paint {
    pub const fn solid(c: Color) -> Self {
        Paint::Solid(c)
    }

    /// A vertical fade from `top` to fully transparent at `bottom_y` — the
    /// standard area-chart gradient, spelled once so every series builds it
    /// identically.
    pub fn vertical_fade(top: Color, top_y: f32, bottom_y: f32) -> Self {
        Paint::Linear {
            from: pt(0.0, top_y),
            to: pt(0.0, bottom_y),
            stops: vec![
                GradientStop { offset: 0.0, color: top },
                GradientStop { offset: 1.0, color: top.with_alpha(0) },
            ],
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum LineCap {
    #[default]
    Butt,
    Round,
    Square,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum LineJoin {
    #[default]
    Miter,
    Round,
    Bevel,
}

/// Stroke parameters. `dash` is an on/off length pattern; empty means solid.
#[derive(Clone, PartialEq, Debug)]
pub struct Stroke {
    pub width: f32,
    pub cap: LineCap,
    pub join: LineJoin,
    pub dash: Vec<f32>,
    pub dash_offset: f32,
}

impl Default for Stroke {
    fn default() -> Self {
        Self {
            width: 1.0,
            cap: LineCap::default(),
            join: LineJoin::default(),
            dash: Vec::new(),
            dash_offset: 0.0,
        }
    }
}

impl Stroke {
    pub fn width(width: f32) -> Self {
        Self { width, ..Default::default() }
    }

    pub fn dashed(width: f32, pattern: impl Into<Vec<f32>>) -> Self {
        Self { width, dash: pattern.into(), ..Default::default() }
    }

    pub fn cap(mut self, cap: LineCap) -> Self {
        self.cap = cap;
        self
    }

    pub fn join(mut self, join: LineJoin) -> Self {
        self.join = join;
        self
    }
}

/// Fill winding rule.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FillRule {
    #[default]
    NonZero,
    EvenOdd,
}

/// One instance in a [`Mark::Points`] batch.
///
/// Split out from the general fill path because a scatter plot is the one
/// chart type where mark count is unbounded — tens of thousands of points is
/// ordinary. Emitting a full `Path` per point would allocate per datum and
/// defeat the instanced batch (`DrawOp::Shapes`) the GPU renderer already
/// supports, so points get their own flat, copyable representation.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PointInstance {
    pub center: Point,
    /// Half-width and half-height, so squares and circles share one shape.
    pub half: Point,
    /// Corner radius. Equal to `half.x`/`half.y` yields a circle.
    pub radius: f32,
    pub color: Color,
}

/// Which layer a mark belongs to.
///
/// Charts have a fixed and meaningful paint order — gridlines behind data,
/// data behind the hover crosshair — and getting it wrong is a classic bug
/// (gridlines painted over the series). Rather than depend on emission order
/// and hope every renderer is stable, marks carry their layer and the scene
/// sorts. The sort is stable, so within a layer emission order still wins.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Layer {
    /// Plot background fill / banding.
    Background,
    /// Gridlines and the zero rule.
    Grid,
    /// Area fills — beneath their own lines.
    AreaFill,
    /// Bars, lines, and scatter points.
    Series,
    /// Axis spines and tick ticks.
    Axis,
    /// Crosshair, hover highlight, focus ring.
    Overlay,
}

/// A single drawing instruction.
///
/// Every variant maps 1:1 onto a `canvas_core::DrawOp`, which is not a
/// coincidence — the IR was designed against what that `Scene` can already
/// express so the idealyst adapter is lossless and mechanical. It is
/// nonetheless our own type, because `canvas-core` depends on four runtime
/// crates and could not be a dependency of a standalone crate.
#[derive(Clone, PartialEq, Debug)]
pub enum Mark {
    Fill {
        layer: Layer,
        path: Path,
        paint: Paint,
        rule: FillRule,
    },
    Stroke {
        layer: Layer,
        path: Path,
        stroke: Stroke,
        paint: Paint,
    },
    /// An instanced batch of points. See [`PointInstance`].
    Points {
        layer: Layer,
        instances: Vec<PointInstance>,
    },
}

impl Mark {
    pub fn layer(&self) -> Layer {
        match self {
            Mark::Fill { layer, .. } | Mark::Stroke { layer, .. } | Mark::Points { layer, .. } => {
                *layer
            }
        }
    }
}

/// Horizontal alignment of a label relative to its anchor point.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HAlign {
    Left,
    Center,
    Right,
}

/// Vertical alignment of a label relative to its anchor point.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VAlign {
    Top,
    Middle,
    Baseline,
    Bottom,
}

/// What a label is for. The host uses this to pick a text style and to
/// decide placement strategy — an idealyst host puts axis labels into
/// flex-laid-out gutters, while a standalone SVG host draws them directly.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LabelRole {
    AxisX,
    AxisY,
    AxisTitleX,
    AxisTitleY,
    Title,
    Legend,
    DataLabel,
}

/// A piece of text the chart wants rendered, and where.
///
/// Crucially NOT glyphs. Plotters bakes glyph rasterization into its
/// backend seam, which forces every consumer to own a font stack and makes
/// native text impossible. Emitting a placement instead lets the host
/// choose: draw it (standalone SVG/raster hosts) or materialize it as a
/// real native text node (the idealyst SDK), where it picks up the app's
/// fonts, theme colors, and accessibility text scaling for free.
///
/// It also means this crate never needs to measure text for the idealyst
/// path — labels go into flex gutters that the framework's own layout sizes
/// to the widest child.
#[derive(Clone, PartialEq, Debug)]
pub struct LabelPlacement {
    pub text: String,
    pub anchor: Point,
    pub h_align: HAlign,
    pub v_align: VAlign,
    pub role: LabelRole,
    /// Clockwise rotation in degrees. Non-zero only for crowded category
    /// axes and for the y-axis title.
    pub rotation: f32,
    /// Set when the host should tint this label to match a series, e.g.
    /// legend entries. `None` means "use the theme's normal text color".
    pub color: Option<Color>,
}

/// The complete output of a render: everything to draw, everything to
/// label, and the geometry needed to interpret a pointer position.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct ChartScene {
    pub marks: Vec<Mark>,
    pub labels: Vec<LabelPlacement>,
    /// The data area, excluding axis gutters. Hosts clip to this.
    pub plot: Rect,
}

impl ChartScene {
    pub fn push(&mut self, m: Mark) {
        self.marks.push(m);
    }

    pub fn label(&mut self, l: LabelPlacement) {
        self.labels.push(l);
    }

    /// Sort marks into paint order. Stable, so emission order is preserved
    /// within a layer. Called once at the end of a render.
    pub fn sort_layers(&mut self) {
        self.marks.sort_by_key(|m| m.layer());
    }
}
