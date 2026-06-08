//! The retained 2D scene model — the renderer-agnostic abstraction.
//!
//! A [`Scene`] is a flat, serializable list of [`DrawOp`]s. Authors
//! build one with pure-Rust calls (zero FFI); a renderer then replays
//! the ops onto its surface. The same `Scene` drives every renderer
//! (`canvas-native`'s per-platform engines, `canvas-vello`'s GPU
//! pipeline), which is what makes them interchangeable *and* makes
//! benchmarking one against the other apples-to-apples: feed the
//! identical scene to each and time the flush.
//!
//! # Why retained, not immediate-mode
//!
//! An immediate-mode API (`path.move_to()` calling straight through to
//! the backend) would issue one FFI/JNI crossing per segment — fine for
//! CoreGraphics (a C API) but pathological on Android, where every
//! `Path.lineTo` is a JVM round-trip. A retained `Scene` decouples
//! authoring (pure Rust) from the flush (one batched crossing per
//! frame), so the boundary cost is `O(frames)`, not `O(draw calls)`.
//!
//! # Coordinate space
//!
//! Logical pixels, origin top-left, x right / y down — the Canvas2D /
//! CoreGraphics / `android.graphics` convention. Renderers scale to the
//! surface's device-pixel ratio at flush time.

use std::sync::Arc;

use runtime_core::color::Rgba;
use serde::{Deserialize, Serialize};

/// A drawing color. Re-exported from the framework's canonical
/// [`Rgba`](runtime_core::color::Rgba) so canvas colors interoperate
/// with theme colors and backend color packing without a second color
/// type. Parse a CSS string with [`color`].
pub type Color = Rgba;

/// Parse a CSS color string (`#rgb`, `#rrggbb`, `#rrggbbaa`,
/// `rgb(...)`, `rgba(...)`) into a [`Color`], falling back to opaque
/// black on an unparseable input. Delegates to the framework's
/// canonical color parser — canvas does not ship its own.
pub fn color(s: &str) -> Color {
    runtime_core::color::parse_or(s, Rgba::BLACK)
}

// ============================================================================
// Path geometry
// ============================================================================

/// One segment of a [`Path`]. Coordinates are absolute logical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum PathSeg {
    /// Start a new subpath at `(x, y)`.
    MoveTo {
        /// Subpath start x.
        x: f32,
        /// Subpath start y.
        y: f32,
    },
    /// Straight line from the current point to `(x, y)`.
    LineTo {
        /// Endpoint x.
        x: f32,
        /// Endpoint y.
        y: f32,
    },
    /// Quadratic Bézier from the current point to `(x, y)` with control
    /// point `(cx, cy)`.
    QuadTo {
        /// Control x.
        cx: f32,
        /// Control y.
        cy: f32,
        /// Endpoint x.
        x: f32,
        /// Endpoint y.
        y: f32,
    },
    /// Cubic Bézier from the current point to `(x, y)` with control
    /// points `(c1x, c1y)` and `(c2x, c2y)`.
    CubicTo {
        /// First control x.
        c1x: f32,
        /// First control y.
        c1y: f32,
        /// Second control x.
        c2x: f32,
        /// Second control y.
        c2y: f32,
        /// Endpoint x.
        x: f32,
        /// Endpoint y.
        y: f32,
    },
    /// Close the current subpath back to its start point.
    Close,
}

/// An ordered list of [`PathSeg`]s. Build one inline on a [`Scene`]
/// (`scene.path().move_to(..).line_to(..)`) or standalone with the
/// shape constructors ([`Path::rect`], [`Path::circle`], …) for reuse
/// across multiple fills/strokes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Path {
    /// The segments, in draw order.
    pub segs: Vec<PathSeg>,
}

impl Path {
    /// An empty path.
    pub fn new() -> Self {
        Self { segs: Vec::new() }
    }

    /// Append a `MoveTo`.
    pub fn move_to(mut self, x: f32, y: f32) -> Self {
        self.segs.push(PathSeg::MoveTo { x, y });
        self
    }

    /// Append a `LineTo`.
    pub fn line_to(mut self, x: f32, y: f32) -> Self {
        self.segs.push(PathSeg::LineTo { x, y });
        self
    }

    /// Append a `QuadTo`.
    pub fn quad_to(mut self, cx: f32, cy: f32, x: f32, y: f32) -> Self {
        self.segs.push(PathSeg::QuadTo { cx, cy, x, y });
        self
    }

    /// Append a `CubicTo`.
    pub fn cubic_to(mut self, c1x: f32, c1y: f32, c2x: f32, c2y: f32, x: f32, y: f32) -> Self {
        self.segs.push(PathSeg::CubicTo { c1x, c1y, c2x, c2y, x, y });
        self
    }

    /// Append a `Close`.
    pub fn close(mut self) -> Self {
        self.segs.push(PathSeg::Close);
        self
    }

    /// An axis-aligned rectangle subpath.
    pub fn rect(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self::new()
            .move_to(x, y)
            .line_to(x + w, y)
            .line_to(x + w, y + h)
            .line_to(x, y + h)
            .close()
    }

    /// A rounded rectangle subpath with uniform corner radius `r`
    /// (clamped to half the shorter side).
    pub fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Self {
        let r = r.min(w * 0.5).min(h * 0.5).max(0.0);
        // 4 quadratic corners; control points at the box corners.
        Self::new()
            .move_to(x + r, y)
            .line_to(x + w - r, y)
            .quad_to(x + w, y, x + w, y + r)
            .line_to(x + w, y + h - r)
            .quad_to(x + w, y + h, x + w - r, y + h)
            .line_to(x + r, y + h)
            .quad_to(x, y + h, x, y + h - r)
            .line_to(x, y + r)
            .quad_to(x, y, x + r, y)
            .close()
    }

    /// A circle subpath, approximated with four cubic Béziers.
    pub fn circle(cx: f32, cy: f32, r: f32) -> Self {
        Self::ellipse(cx, cy, r, r)
    }

    /// An axis-aligned ellipse subpath, approximated with four cubic
    /// Béziers. `K` is the standard circle-to-cubic magic constant.
    pub fn ellipse(cx: f32, cy: f32, rx: f32, ry: f32) -> Self {
        const K: f32 = 0.552_284_8; // 4/3 * (sqrt(2) - 1)
        let (ox, oy) = (rx * K, ry * K);
        Self::new()
            .move_to(cx + rx, cy)
            .cubic_to(cx + rx, cy + oy, cx + ox, cy + ry, cx, cy + ry)
            .cubic_to(cx - ox, cy + ry, cx - rx, cy + oy, cx - rx, cy)
            .cubic_to(cx - rx, cy - oy, cx - ox, cy - ry, cx, cy - ry)
            .cubic_to(cx + ox, cy - ry, cx + rx, cy - oy, cx + rx, cy)
            .close()
    }

    /// `true` if the path has no segments.
    pub fn is_empty(&self) -> bool {
        self.segs.is_empty()
    }
}

// ============================================================================
// Paint
// ============================================================================

/// How a fill or stroke is colored: a flat color or a gradient, plus how
/// it composites against what's already on the canvas.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Paint {
    /// The paint source.
    pub kind: PaintKind,
    /// How this paint composites against existing canvas content.
    /// Defaults to [`BlendMode::Normal`] (source-over). `#[serde(default)]`
    /// keeps older wire scenes (recorded before blend existed) decoding —
    /// a missing field deserializes to `Normal`.
    #[serde(default)]
    pub blend: BlendMode,
}

/// How a paint (or image blit) composites against the pixels already on
/// the canvas. The canvas surface is transparent, so the composite is
/// against the strokes drawn earlier *in the same frame* (or, for a
/// persistent layer, earlier frames too).
///
/// `#[non_exhaustive]` so more Porter-Duff / separable blend modes can be
/// added without breaking renderer match arms — a renderer that doesn't
/// recognize a mode falls back to [`Normal`](BlendMode::Normal).
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlendMode {
    /// Source-over — the Canvas2D default. Paint is laid over the
    /// destination with straight alpha.
    #[default]
    Normal,
    /// Destination-out — the paint's *coverage* erases the destination,
    /// leaving transparency (`result = dst · (1 − src_alpha)`). This is
    /// the basis for a pixel eraser: stroke with any opaque color and the
    /// coverage punches a hole through everything drawn before it,
    /// revealing whatever sits behind the transparent canvas. The color
    /// channels are ignored; only the alpha matters.
    DestinationOut,
    /// Multiply the source and destination colors — darkens. Useful for
    /// highlighter-style ink that tints what it crosses.
    Multiply,
    /// Screen blend — lightens (inverse-multiply of the inverses).
    Screen,
}

/// The paint source variants. `#[non_exhaustive]` so image/pattern
/// paints can be added later without breaking match arms in renderers
/// they don't yet support (those fall back to transparent).
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PaintKind {
    /// A single flat color.
    Solid(#[serde(with = "rgba_argb")] Color),
    /// A linear gradient between two points.
    Linear(LinearGradient),
    /// A radial gradient.
    Radial(RadialGradient),
}

impl Paint {
    /// A flat-color paint.
    pub fn solid(c: impl Into<Color>) -> Self {
        Self { kind: PaintKind::Solid(c.into()), blend: BlendMode::Normal }
    }

    /// A linear-gradient paint from `(x0, y0)` to `(x1, y1)`.
    pub fn linear(x0: f32, y0: f32, x1: f32, y1: f32, stops: Vec<GradientStop>) -> Self {
        Self {
            kind: PaintKind::Linear(LinearGradient { x0, y0, x1, y1, stops }),
            blend: BlendMode::Normal,
        }
    }

    /// A radial-gradient paint centered at `(cx, cy)` with radius `r`.
    pub fn radial(cx: f32, cy: f32, r: f32, stops: Vec<GradientStop>) -> Self {
        Self {
            kind: PaintKind::Radial(RadialGradient { cx, cy, r, stops }),
            blend: BlendMode::Normal,
        }
    }

    /// Set the [`BlendMode`] for this paint. Chains onto any constructor:
    /// `Paint::solid(c).blend(BlendMode::DestinationOut)` makes an
    /// eraser stroke.
    pub fn blend(mut self, blend: BlendMode) -> Self {
        self.blend = blend;
        self
    }

    /// An eraser paint — opaque coverage in [`BlendMode::DestinationOut`].
    /// The color is irrelevant (only alpha drives the erase); opaque white
    /// gives full erase. Stroke a path with this to cut a hole through
    /// everything drawn before it this frame.
    pub fn eraser() -> Self {
        Paint::solid(Color::new(255, 255, 255, 255)).blend(BlendMode::DestinationOut)
    }
}

impl From<Color> for Paint {
    fn from(c: Color) -> Self {
        Paint::solid(c)
    }
}

/// One color stop in a gradient.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GradientStop {
    /// Position along the gradient axis, `0.0..=1.0`.
    pub offset: f32,
    /// Color at this stop.
    #[serde(with = "rgba_argb")]
    pub color: Color,
}

impl GradientStop {
    /// A stop at `offset` with `color`.
    pub fn new(offset: f32, color: impl Into<Color>) -> Self {
        Self { offset, color: color.into() }
    }
}

/// A linear gradient between `(x0, y0)` and `(x1, y1)`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LinearGradient {
    /// Axis start x.
    pub x0: f32,
    /// Axis start y.
    pub y0: f32,
    /// Axis end x.
    pub x1: f32,
    /// Axis end y.
    pub y1: f32,
    /// Color stops, expected sorted by `offset`.
    pub stops: Vec<GradientStop>,
}

/// A radial gradient centered at `(cx, cy)` with radius `r`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RadialGradient {
    /// Center x.
    pub cx: f32,
    /// Center y.
    pub cy: f32,
    /// Outer radius.
    pub r: f32,
    /// Color stops, expected sorted by `offset`.
    pub stops: Vec<GradientStop>,
}

// ============================================================================
// Stroke style
// ============================================================================

/// How a path's outline is stroked.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    /// Line width in logical pixels.
    pub width: f32,
    /// End-cap style for open subpaths.
    pub cap: LineCap,
    /// Join style between segments.
    pub join: LineJoin,
    /// Miter limit (ratio) before a miter join falls back to bevel.
    pub miter_limit: f32,
}

impl Stroke {
    /// A stroke of the given `width` with default butt caps / miter
    /// joins.
    pub fn width(width: f32) -> Self {
        Self { width, cap: LineCap::Butt, join: LineJoin::Miter, miter_limit: 4.0 }
    }

    /// Set the end-cap style.
    pub fn cap(mut self, cap: LineCap) -> Self {
        self.cap = cap;
        self
    }

    /// Set the join style.
    pub fn join(mut self, join: LineJoin) -> Self {
        self.join = join;
        self
    }

    /// Set the miter limit.
    pub fn miter_limit(mut self, limit: f32) -> Self {
        self.miter_limit = limit;
        self
    }
}

impl Default for Stroke {
    fn default() -> Self {
        Self::width(1.0)
    }
}

/// Line end-cap style.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineCap {
    /// Squared off at the endpoint.
    #[default]
    Butt,
    /// Rounded past the endpoint by half the width.
    Round,
    /// Squared off past the endpoint by half the width.
    Square,
}

/// Line join style between two segments.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineJoin {
    /// Sharp corner, limited by [`Stroke::miter_limit`].
    #[default]
    Miter,
    /// Rounded corner.
    Round,
    /// Flattened corner.
    Bevel,
}

/// Fill winding rule for self-intersecting paths.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FillRule {
    /// Non-zero winding (the common default).
    #[default]
    NonZero,
    /// Even-odd winding.
    EvenOdd,
}

// ============================================================================
// Transform
// ============================================================================

/// A 2×3 affine transform, Canvas2D `setTransform(a, b, c, d, e, f)`
/// order. Maps `(x, y)` to `(a·x + c·y + e, b·x + d·y + f)`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    /// Row-0 scale-x component.
    pub a: f32,
    /// Row-1 shear-x (skew-y) component.
    pub b: f32,
    /// Row-0 shear-y (skew-x) component.
    pub c: f32,
    /// Row-1 scale-y component.
    pub d: f32,
    /// Translate-x.
    pub e: f32,
    /// Translate-y.
    pub f: f32,
}

impl Transform {
    /// The identity transform.
    pub const IDENTITY: Transform = Transform { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };

    /// A translation by `(dx, dy)`.
    pub fn translate(dx: f32, dy: f32) -> Self {
        Self { e: dx, f: dy, ..Self::IDENTITY }
    }

    /// A scale by `(sx, sy)`.
    pub fn scale(sx: f32, sy: f32) -> Self {
        Self { a: sx, d: sy, ..Self::IDENTITY }
    }

    /// A rotation by `radians` (clockwise in the y-down space).
    pub fn rotate(radians: f32) -> Self {
        let (s, co) = radians.sin_cos();
        Self { a: co, b: s, c: -s, d: co, e: 0.0, f: 0.0 }
    }

    /// Compose: apply `self` first, then `next`. Equivalent to the
    /// matrix product `next · self`.
    pub fn then(self, next: Transform) -> Transform {
        let (m, n) = (self, next);
        Transform {
            a: n.a * m.a + n.c * m.b,
            b: n.b * m.a + n.d * m.b,
            c: n.a * m.c + n.c * m.d,
            d: n.b * m.c + n.d * m.d,
            e: n.a * m.e + n.c * m.f + n.e,
            f: n.b * m.e + n.d * m.f + n.f,
        }
    }
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

// ============================================================================
// Images
// ============================================================================

/// An axis-aligned rectangle in logical pixels. Used as the destination of
/// an image blit ([`DrawOp::Image`]).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

impl Rect {
    /// A rectangle at `(x, y)` of size `w × h`.
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
}

/// Decoded image pixels for a [`DrawOp::Image`] blit: straight
/// (non-premultiplied) RGBA8, row-major, exactly `width * height * 4`
/// bytes.
///
/// **Why raw RGBA, not encoded bytes.** Every renderer (Canvas2D,
/// CoreGraphics, `android.graphics`, vello) can upload raw RGBA
/// synchronously; encoded-container decoding is async on the web and
/// pulls a decoder dep on native. Decoding to RGBA once, in author code
/// (see [`ImageSource::decode`] behind the `decode` feature), keeps the
/// renderer contract uniform and the per-frame replay allocation-free past
/// the first upload.
///
/// **`id` is a decode-cache key.** Renderers cache the uploaded native
/// image (a `CGImage`, `Bitmap`, `<canvas>`, or `peniko::Image`) keyed by
/// `id`, so re-emitting the same image every frame doesn't re-upload. Two
/// `ImageSource`s that share an `id` MUST have identical pixels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImageSource {
    /// Stable identity for renderer-side upload caching.
    pub id: u64,
    /// Pixel width.
    pub width: u32,
    /// Pixel height.
    pub height: u32,
    /// `width * height * 4` bytes of straight RGBA8, row-major.
    pub rgba: Vec<u8>,
}

impl ImageSource {
    /// Build an image from raw straight-RGBA8 pixels. `rgba.len()` must be
    /// `width * height * 4`; a mismatch is the caller's bug (renderers may
    /// draw nothing rather than read out of bounds).
    pub fn from_rgba8(id: u64, width: u32, height: u32, rgba: Vec<u8>) -> Self {
        Self { id, width, height, rgba }
    }

    /// `true` if the pixel buffer length matches `width * height * 4`.
    /// Renderers check this before uploading.
    pub fn is_valid(&self) -> bool {
        self.rgba.len() == (self.width as usize) * (self.height as usize) * 4
    }

    /// Decode encoded image bytes (PNG/JPEG/…) into straight RGBA8. The
    /// container format is auto-detected. Requires the `decode` feature.
    ///
    /// `id` should be stable for identical bytes (e.g. a hash) so renderers
    /// can cache the upload across frames.
    #[cfg(feature = "decode")]
    pub fn decode(id: u64, bytes: &[u8]) -> Result<Self, image::ImageError> {
        let img = image::load_from_memory(bytes)?.into_rgba8();
        let (width, height) = img.dimensions();
        Ok(Self { id, width, height, rgba: img.into_raw() })
    }
}

// ============================================================================
// Shape batch
// ============================================================================

/// One flat-colored analytic shape in a [`DrawOp::Shapes`] batch: a **rounded
/// box** defined by center, half-extents, corner radius, and a solid color, in
/// logical pixels.
///
/// A rounded box is deliberately general — with the right radius it is a
/// rectangle (`radius = 0`), a rounded rectangle, a circle (`hw == hh`,
/// `radius = hw`), or a pill (`radius = min(hw, hh)`). The named constructors
/// ([`circle`](Self::circle), [`rect`](Self::rect),
/// [`rounded_rect`](Self::rounded_rect), [`pill`](Self::pill)) cover these
/// cases, and a single GPU SDF (`sd_round_box`) rasterizes all of them, so one
/// instanced pass draws a mixed batch. (More SDF families — triangle, polygon —
/// can be added later behind a shape-type discriminant without changing the
/// batch's pipeline or wire shape.)
///
/// A batch exists so a renderer can draw a grid/scatter of many shapes in a
/// single GPU-instanced, analytic pass instead of tessellating one path per
/// shape (the per-path flatten/bin cost is what makes a large grid of
/// [`Path::circle`]/[`Path::rounded_rect`] fills slow). The trade is per-shape
/// paint flexibility: the batch carries only a solid color. For gradient-filled
/// or stroked shapes, emit individual [`Fill`](DrawOp::Fill) /
/// [`Stroke`](DrawOp::Stroke) ops instead.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShapeInstance {
    /// Center x.
    pub cx: f32,
    /// Center y.
    pub cy: f32,
    /// Half-width — the box spans `cx - hw ..= cx + hw`.
    pub hw: f32,
    /// Half-height — the box spans `cy - hh ..= cy + hh`.
    pub hh: f32,
    /// Corner radius. `0` = sharp rectangle; `min(hw, hh)` = fully rounded
    /// (a circle when `hw == hh`). Clamped to `0..=min(hw, hh)` at rasterization
    /// (see [`effective_radius`](Self::effective_radius)).
    pub radius: f32,
    /// Flat fill color. Packed as a single ARGB u32 on the wire (same shim as
    /// every other canvas color).
    #[serde(with = "rgba_argb")]
    pub color: Color,
}

impl ShapeInstance {
    /// A rounded box centered at `(cx, cy)` with the given half-extents and
    /// corner `radius`, filled `color`.
    pub fn new(cx: f32, cy: f32, hw: f32, hh: f32, radius: f32, color: impl Into<Color>) -> Self {
        Self { cx, cy, hw, hh, radius, color: color.into() }
    }

    /// A circle: equal half-extents `r` with a full corner radius.
    pub fn circle(cx: f32, cy: f32, r: f32, color: impl Into<Color>) -> Self {
        Self::new(cx, cy, r, r, r, color)
    }

    /// A sharp-cornered rectangle from its top-left `(x, y)` and size `w × h`.
    pub fn rect(x: f32, y: f32, w: f32, h: f32, color: impl Into<Color>) -> Self {
        Self::new(x + w * 0.5, y + h * 0.5, w * 0.5, h * 0.5, 0.0, color)
    }

    /// A rounded rectangle from its top-left `(x, y)`, size `w × h`, and corner
    /// `radius`.
    pub fn rounded_rect(
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        color: impl Into<Color>,
    ) -> Self {
        Self::new(x + w * 0.5, y + h * 0.5, w * 0.5, h * 0.5, radius, color)
    }

    /// A pill / capsule from its top-left `(x, y)` and size `w × h` — fully
    /// rounded on the short axis.
    pub fn pill(x: f32, y: f32, w: f32, h: f32, color: impl Into<Color>) -> Self {
        let (hw, hh) = (w * 0.5, h * 0.5);
        Self::new(x + hw, y + hh, hw, hh, hw.min(hh), color)
    }

    /// The corner radius actually used when rasterizing: `radius` clamped to
    /// `0..=min(hw, hh)`. Both the GPU SDF pass and the fill-expansion fallback
    /// use this so they converge on the same geometry.
    pub fn effective_radius(&self) -> f32 {
        self.radius.clamp(0.0, self.hw.min(self.hh).max(0.0))
    }

    /// The equivalent individual [`DrawOp::Fill`] — a path matching this shape,
    /// filled solid with its color, non-zero winding, composited with `blend`.
    ///
    /// Renderers without an instanced fast path expand a [`DrawOp::Shapes`]
    /// batch into one of these per shape, **in array order**, so the observable
    /// output is identical to authoring the fills by hand — and so every backend
    /// (instanced or not) converges on the same pixels (CLAUDE.md §7). A renderer
    /// *with* an instanced path is free to draw the batch directly, as long as it
    /// matches this expansion's result.
    pub fn to_fill_op(&self, blend: BlendMode) -> DrawOp {
        let r = self.effective_radius();
        let (x, y, w, h) = (self.cx - self.hw, self.cy - self.hh, self.hw * 2.0, self.hh * 2.0);
        let path = if r <= 0.0 {
            Path::rect(x, y, w, h)
        } else if self.hw == self.hh && r >= self.hw {
            // Exact circle — the dedicated cubic-Bézier circle is the canonical
            // geometry (a full-radius rounded rect approximates it with quads).
            Path::circle(self.cx, self.cy, self.hw)
        } else {
            Path::rounded_rect(x, y, w, h, r)
        };
        DrawOp::Fill { path, paint: Paint::solid(self.color).blend(blend), fill_rule: FillRule::NonZero }
    }
}

// ============================================================================
// Draw ops + Scene
// ============================================================================

/// A single retained drawing instruction. A [`Scene`] is a `Vec` of
/// these; renderers replay them in order. `#[non_exhaustive]` so new
/// ops (text, image blit) can be added without breaking renderer match
/// arms.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DrawOp {
    /// Fill `path` with `paint` using `fill_rule`.
    Fill {
        /// Geometry to fill.
        path: Path,
        /// Fill paint.
        paint: Paint,
        /// Winding rule.
        fill_rule: FillRule,
    },
    /// Stroke `path`'s outline with `paint` and `stroke` style.
    Stroke {
        /// Geometry to stroke.
        path: Path,
        /// Stroke paint.
        paint: Paint,
        /// Stroke style.
        stroke: Stroke,
    },
    /// Push the current transform/clip state (Canvas2D `save()`).
    Save,
    /// Pop the last saved state (Canvas2D `restore()`).
    Restore,
    /// Concatenate `Transform` onto the current transform.
    Transform(Transform),
    /// Intersect the clip region with `path`.
    Clip {
        /// Clip geometry.
        path: Path,
        /// Winding rule for the clip.
        fill_rule: FillRule,
    },
    /// Blit `image` into the `dst` rectangle (scaled to fit), at `alpha`
    /// opacity and composited with `blend`. The renderer caches the
    /// uploaded native image by [`ImageSource::id`]. This is the
    /// "place an image, then draw over it" primitive — author a transparent
    /// canvas above it and the strokes land on top.
    Image {
        /// Source pixels (+ decode-cache id). `Arc` so a static image placed
        /// every frame (e.g. dragging it) costs a refcount bump per repaint, not
        /// a full pixel-buffer clone — renderers cache the decoded upload by
        /// [`ImageSource::id`] and never touch the buffer on a cache hit.
        image: Arc<ImageSource>,
        /// Destination rectangle in logical pixels.
        dst: Rect,
        /// Opacity, `0.0..=1.0`.
        alpha: f32,
        /// Composite mode against existing canvas content.
        blend: BlendMode,
    },
    /// Draw `ops` into a **persistent** raster layer identified by `id`,
    /// then composite that layer into the current target at `alpha` opacity
    /// with `blend`.
    ///
    /// The layer's contents survive across frames — the renderer keeps a
    /// surface (an offscreen raster on the CPU backends, a retained op-log on
    /// the GPU backend) alive between flushes. This is the "bake" primitive:
    ///
    /// - **`clear: false`** — `ops` *accumulate* onto whatever the layer
    ///   already held. Emit only the newly-drawn shapes each frame and the
    ///   layer keeps the rest, so a 10k-stroke drawing doesn't replay 10k ops
    ///   every frame. An eraser stroke ([`BlendMode::DestinationOut`]) inside
    ///   `ops` removes baked pixels *permanently* (a true pixel eraser, not a
    ///   within-frame one).
    /// - **`clear: true`** — wipe the layer to transparent before replaying
    ///   `ops` (a full repaint of the layer's content this frame).
    ///
    /// Renderers diverge in mechanism but converge in output (CLAUDE.md §7):
    /// the observable pixels — accumulation and persistent erase — are
    /// identical on every backend.
    Layer {
        /// Stable identity of the persistent layer surface.
        id: u32,
        /// Wipe the layer before replaying `ops`.
        clear: bool,
        /// Ops drawn into the layer this frame.
        ops: Vec<DrawOp>,
        /// Opacity of the whole layer when composited, `0.0..=1.0`.
        alpha: f32,
        /// Composite mode of the layer against the current target.
        blend: BlendMode,
    },
    /// Draw a batch of flat-colored analytic shapes ([`ShapeInstance`], each a
    /// rounded box) in one operation.
    ///
    /// Semantically equivalent to one [`Fill`](DrawOp::Fill) per entry, **in
    /// array order** (see [`ShapeInstance::to_fill_op`]) — the batch is purely a
    /// throughput hint. A renderer with a GPU-instanced, analytic-SDF path draws
    /// them all in a single pass (the fast path for a grid/scatter of many
    /// shapes); one without it expands the batch into the equivalent per-shape
    /// fills. Either way the pixels match, so the batch never changes *what* is
    /// drawn, only how fast. The whole batch composites with `blend` against
    /// existing content.
    Shapes {
        /// The shapes, in draw order (earlier entries are drawn under later
        /// ones where they overlap, matching per-shape fill order).
        shapes: Vec<ShapeInstance>,
        /// Composite mode of the batch against existing canvas content.
        blend: BlendMode,
    },
}

/// A retained list of draw operations — the renderer-agnostic unit a
/// canvas renderer consumes. Build it imperatively; the current-path
/// cursor mirrors Canvas2D semantics (`path()` begins a path,
/// `fill()`/`stroke()` consume the current path).
///
/// # The current path
///
/// Geometry builders ([`move_to`](Self::move_to),
/// [`line_to`](Self::line_to), [`add_path`](Self::add_path), …) append to
/// a single **current path**; [`fill`](Self::fill) /
/// [`stroke`](Self::stroke) then paint *that* accumulated path. This is
/// the Canvas2D model, and it has one classic footgun: a loop that draws
/// many shapes without resetting between them appends them all into one
/// path, so each `fill`/`stroke` repaints every prior shape with the
/// latest paint (the "my color applied to all strokes" bug).
///
/// The `Scene` defuses it: once a draw consumes the current path, the
/// next geometry call **auto-begins a fresh path**, so a forgotten
/// [`path`](Self::path) can't accumulate. You still *may* call
/// [`path`](Self::path) to start a new path explicitly, and the
/// legitimate fill-then-stroke-the-same-shape pattern still works (no
/// geometry is added between the two draws, so no reset happens).
///
/// When a shape is self-contained, prefer the one-shot
/// [`fill_path`](Self::fill_path) / [`stroke_path`](Self::stroke_path),
/// which paint a freestanding [`Path`] without touching the current-path
/// cursor at all — no ordering to remember.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Scene {
    /// The recorded ops, in draw order.
    ops: Vec<DrawOp>,
    /// The in-progress path that `fill`/`stroke`/`clip` consume. Not
    /// serialized — it's a build-time cursor, fully captured into ops
    /// by the time a scene crosses the wire.
    #[serde(skip)]
    current: Path,
    /// `true` once the current path has been consumed by a draw
    /// (`fill`/`stroke`/`clip`). The next call that *adds* geometry
    /// (`move_to`, `add_path`, …) auto-begins a fresh path so accumulated
    /// segments don't bleed into a new shape.
    ///
    /// This is the fix for the classic footgun: a loop that draws many
    /// shapes but forgets `path()` between them would otherwise keep
    /// appending to one ever-growing path, and each `fill`/`stroke` would
    /// repaint *every* prior shape with the latest paint. The flag keeps
    /// the legitimate fill-then-stroke reuse (no geometry added between the
    /// two draws → no auto-reset) while making the forgot-`path()` case Just
    /// Work. See [`tests::forgotten_path_does_not_accumulate`].
    #[serde(skip)]
    current_consumed: bool,
}

impl Scene {
    /// An empty scene.
    pub fn new() -> Self {
        Self::default()
    }

    /// The recorded ops, for a renderer to replay.
    pub fn ops(&self) -> &[DrawOp] {
        &self.ops
    }

    /// `true` if nothing has been drawn yet.
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// Begin a fresh current path, discarding any in-progress one.
    /// Chain the `*_to` builders, then [`fill`](Self::fill) /
    /// [`stroke`](Self::stroke) to consume it.
    pub fn path(&mut self) -> &mut Self {
        self.current = Path::new();
        self.current_consumed = false;
        self
    }

    /// If the current path was already consumed by a draw, begin a fresh
    /// one before adding new geometry. Called by every geometry-adding
    /// method so a forgotten [`path`](Self::path) can't accumulate shapes.
    fn begin_if_consumed(&mut self) {
        if self.current_consumed {
            self.current = Path::new();
            self.current_consumed = false;
        }
    }

    /// Move the path cursor to `(x, y)`, starting a new subpath.
    pub fn move_to(&mut self, x: f32, y: f32) -> &mut Self {
        self.begin_if_consumed();
        self.current.segs.push(PathSeg::MoveTo { x, y });
        self
    }

    /// Line from the cursor to `(x, y)`.
    pub fn line_to(&mut self, x: f32, y: f32) -> &mut Self {
        self.begin_if_consumed();
        self.current.segs.push(PathSeg::LineTo { x, y });
        self
    }

    /// Quadratic Bézier from the cursor to `(x, y)` via `(cx, cy)`.
    pub fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) -> &mut Self {
        self.begin_if_consumed();
        self.current.segs.push(PathSeg::QuadTo { cx, cy, x, y });
        self
    }

    /// Cubic Bézier from the cursor to `(x, y)` via two control points.
    pub fn cubic_to(
        &mut self,
        c1x: f32,
        c1y: f32,
        c2x: f32,
        c2y: f32,
        x: f32,
        y: f32,
    ) -> &mut Self {
        self.begin_if_consumed();
        self.current.segs.push(PathSeg::CubicTo { c1x, c1y, c2x, c2y, x, y });
        self
    }

    /// Close the current subpath.
    pub fn close(&mut self) -> &mut Self {
        self.begin_if_consumed();
        self.current.segs.push(PathSeg::Close);
        self
    }

    /// Replace the current path with a prebuilt [`Path`] (e.g. from
    /// [`Path::rect`]).
    pub fn add_path(&mut self, path: Path) -> &mut Self {
        self.begin_if_consumed();
        self.current.segs.extend(path.segs);
        self
    }

    /// Fill the current path with `paint` (non-zero winding).
    pub fn fill(&mut self, paint: impl Into<Paint>) -> &mut Self {
        self.ops.push(DrawOp::Fill {
            path: self.current.clone(),
            paint: paint.into(),
            fill_rule: FillRule::NonZero,
        });
        self.current_consumed = true;
        self
    }

    /// Fill the current path with an explicit winding rule.
    pub fn fill_rule(&mut self, paint: impl Into<Paint>, fill_rule: FillRule) -> &mut Self {
        self.ops.push(DrawOp::Fill { path: self.current.clone(), paint: paint.into(), fill_rule });
        self.current_consumed = true;
        self
    }

    /// Stroke the current path's outline.
    pub fn stroke(&mut self, paint: impl Into<Paint>, stroke: Stroke) -> &mut Self {
        self.ops.push(DrawOp::Stroke { path: self.current.clone(), paint: paint.into(), stroke });
        self.current_consumed = true;
        self
    }

    /// Fill a freestanding path without disturbing the current path.
    pub fn fill_path(&mut self, path: Path, paint: impl Into<Paint>) -> &mut Self {
        self.ops.push(DrawOp::Fill { path, paint: paint.into(), fill_rule: FillRule::NonZero });
        self
    }

    /// Stroke a freestanding path without disturbing the current path.
    pub fn stroke_path(
        &mut self,
        path: Path,
        paint: impl Into<Paint>,
        stroke: Stroke,
    ) -> &mut Self {
        self.ops.push(DrawOp::Stroke { path, paint: paint.into(), stroke });
        self
    }

    /// Push the current transform/clip state.
    pub fn save(&mut self) -> &mut Self {
        self.ops.push(DrawOp::Save);
        self
    }

    /// Pop the last saved transform/clip state.
    pub fn restore(&mut self) -> &mut Self {
        self.ops.push(DrawOp::Restore);
        self
    }

    /// Concatenate an arbitrary affine transform.
    pub fn transform(&mut self, t: Transform) -> &mut Self {
        self.ops.push(DrawOp::Transform(t));
        self
    }

    /// Translate the coordinate system by `(dx, dy)`.
    pub fn translate(&mut self, dx: f32, dy: f32) -> &mut Self {
        self.transform(Transform::translate(dx, dy))
    }

    /// Scale the coordinate system by `(sx, sy)`.
    pub fn scale(&mut self, sx: f32, sy: f32) -> &mut Self {
        self.transform(Transform::scale(sx, sy))
    }

    /// Rotate the coordinate system by `radians`.
    pub fn rotate(&mut self, radians: f32) -> &mut Self {
        self.transform(Transform::rotate(radians))
    }

    /// Blit `image` into the `dst` rectangle (scaled to fit), fully opaque,
    /// source-over. Does not touch the current path. Accepts an owned
    /// [`ImageSource`] or an `Arc<ImageSource>` — pass the `Arc` to re-blit the
    /// same image every frame for a refcount bump instead of a buffer clone.
    pub fn draw_image(&mut self, image: impl Into<Arc<ImageSource>>, dst: Rect) -> &mut Self {
        self.draw_image_with(image, dst, 1.0, BlendMode::Normal)
    }

    /// Blit `image` into `dst` at `alpha` opacity with an explicit
    /// [`BlendMode`]. Does not touch the current path.
    pub fn draw_image_with(
        &mut self,
        image: impl Into<Arc<ImageSource>>,
        dst: Rect,
        alpha: f32,
        blend: BlendMode,
    ) -> &mut Self {
        self.ops
            .push(DrawOp::Image { image: image.into(), dst, alpha: alpha.clamp(0.0, 1.0), blend });
        self
    }

    /// Draw into the persistent layer `id` via the builder `f`, then
    /// composite it fully opaque, source-over. `clear` wipes the layer first;
    /// `clear = false` accumulates onto prior frames' content. See
    /// [`DrawOp::Layer`].
    pub fn layer(&mut self, id: u32, clear: bool, f: impl FnOnce(&mut Scene)) -> &mut Self {
        self.layer_with(id, clear, 1.0, BlendMode::Normal, f)
    }

    /// [`layer`](Self::layer) with explicit composite `alpha` and `blend` for
    /// the whole layer.
    pub fn layer_with(
        &mut self,
        id: u32,
        clear: bool,
        alpha: f32,
        blend: BlendMode,
        f: impl FnOnce(&mut Scene),
    ) -> &mut Self {
        let mut sub = Scene::new();
        f(&mut sub);
        self.ops.push(DrawOp::Layer {
            id,
            clear,
            ops: sub.ops,
            alpha: alpha.clamp(0.0, 1.0),
            blend,
        });
        self
    }

    /// Draw a batch of flat-colored [`ShapeInstance`]s in one op, source-over.
    /// The throughput primitive for a grid or scatter of many shapes: a GPU
    /// renderer draws them in a single instanced pass, a CPU one expands them to
    /// per-shape fills (in iteration order). Does not touch the current path.
    /// See [`DrawOp::Shapes`].
    pub fn shapes(&mut self, shapes: impl IntoIterator<Item = ShapeInstance>) -> &mut Self {
        self.shapes_with(shapes, BlendMode::Normal)
    }

    /// [`shapes`](Self::shapes) with an explicit [`BlendMode`] for the whole
    /// batch.
    pub fn shapes_with(
        &mut self,
        shapes: impl IntoIterator<Item = ShapeInstance>,
        blend: BlendMode,
    ) -> &mut Self {
        self.ops.push(DrawOp::Shapes { shapes: shapes.into_iter().collect(), blend });
        self
    }

    /// Intersect the clip region with the current path.
    pub fn clip(&mut self) -> &mut Self {
        self.ops.push(DrawOp::Clip {
            path: self.current.clone(),
            fill_rule: FillRule::NonZero,
        });
        self.current_consumed = true;
        self
    }
}

// ============================================================================
// Serde shim: pack Rgba as a single ARGB u32 on the wire.
// ============================================================================

mod rgba_argb {
    use super::Color;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(c: &Color, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u32(c.to_argb_u32())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Color, D::Error> {
        Ok(Color::from_argb_u32(u32::deserialize(d)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_consumes_current_path() {
        let mut s = Scene::new();
        s.path().move_to(0.0, 0.0).line_to(10.0, 0.0).line_to(10.0, 10.0).close();
        s.fill(Paint::solid(Color::new(255, 0, 0, 255)));
        assert_eq!(s.ops().len(), 1);
        match &s.ops()[0] {
            DrawOp::Fill { path, paint, fill_rule } => {
                assert_eq!(path.segs.len(), 4);
                assert_eq!(*fill_rule, FillRule::NonZero);
                assert_eq!(paint.kind, PaintKind::Solid(Color::new(255, 0, 0, 255)));
            }
            other => panic!("expected Fill, got {other:?}"),
        }
    }

    #[test]
    fn fill_then_stroke_reuse_same_current_path() {
        let mut s = Scene::new();
        s.path().add_path(Path::rect(0.0, 0.0, 5.0, 5.0));
        s.fill(Color::new(0, 0, 0, 255));
        s.stroke(Color::new(255, 255, 255, 255), Stroke::width(2.0));
        // Both ops carry the same geometry — current path is not cleared
        // by fill/stroke, only by path().
        assert_eq!(s.ops().len(), 2);
        let geom = |op: &DrawOp| match op {
            DrawOp::Fill { path, .. } | DrawOp::Stroke { path, .. } => path.segs.len(),
            _ => 0,
        };
        assert_eq!(geom(&s.ops()[0]), geom(&s.ops()[1]));
        assert!(geom(&s.ops()[0]) > 0);
    }

    #[test]
    fn forgotten_path_does_not_accumulate() {
        // Two shapes drawn in a row WITHOUT a `path()` between them — the
        // classic footgun. The second fill must carry only its own geometry,
        // not the first shape's segments too.
        let mut s = Scene::new();
        s.move_to(0.0, 0.0).line_to(10.0, 0.0).line_to(10.0, 10.0).close();
        s.fill(Color::new(255, 0, 0, 255));
        // Author forgot `path()`: starts a brand-new shape directly.
        s.move_to(50.0, 50.0).line_to(60.0, 50.0);
        s.fill(Color::new(0, 0, 255, 255));

        assert_eq!(s.ops().len(), 2);
        let segs = |op: &DrawOp| match op {
            DrawOp::Fill { path, .. } => path.segs.len(),
            _ => 0,
        };
        assert_eq!(segs(&s.ops()[0]), 4, "first shape: move+line+line+close");
        assert_eq!(
            segs(&s.ops()[1]),
            2,
            "second shape must NOT include the first's 4 segments"
        );
    }

    #[test]
    fn explicit_path_after_draw_still_resets() {
        // The explicit, correct form keeps working unchanged.
        let mut s = Scene::new();
        s.path().add_path(Path::rect(0.0, 0.0, 5.0, 5.0));
        s.fill(Color::new(1, 2, 3, 255));
        s.path().add_path(Path::circle(20.0, 20.0, 4.0));
        s.fill(Color::new(4, 5, 6, 255));
        let segs = |op: &DrawOp| match op {
            DrawOp::Fill { path, .. } => path.segs.len(),
            _ => 0,
        };
        // circle = 6 segs (move + 4 cubics + close); critically NOT rect(5) + circle(6).
        assert_eq!(segs(&s.ops()[1]), 6);
    }

    #[test]
    fn draw_image_records_blit_op_and_clamps_alpha() {
        let img = ImageSource::from_rgba8(7, 2, 2, vec![255; 16]);
        assert!(img.is_valid());
        let mut s = Scene::new();
        s.draw_image_with(img.clone(), Rect::new(10.0, 20.0, 30.0, 40.0), 2.0, BlendMode::Multiply);
        match &s.ops()[0] {
            DrawOp::Image { image, dst, alpha, blend } => {
                assert_eq!(image.id, 7);
                assert_eq!(*dst, Rect::new(10.0, 20.0, 30.0, 40.0));
                assert_eq!(*alpha, 1.0, "alpha must be clamped to 1.0");
                assert_eq!(*blend, BlendMode::Multiply);
            }
            other => panic!("expected Image, got {other:?}"),
        }
    }

    #[test]
    fn image_op_survives_scene_round_trip() {
        let img = ImageSource::from_rgba8(42, 1, 1, vec![1, 2, 3, 4]);
        let mut s = Scene::new();
        s.draw_image(img, Rect::new(0.0, 0.0, 1.0, 1.0));
        let bytes = serde_json::to_vec(&s).expect("serialize");
        let back: Scene = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(s.ops(), back.ops());
    }

    #[test]
    fn layer_builder_captures_nested_ops() {
        let mut s = Scene::new();
        s.layer_with(3, false, 0.5, BlendMode::DestinationOut, |l| {
            l.path().add_path(Path::rect(0.0, 0.0, 4.0, 4.0));
            l.fill(Color::new(255, 0, 0, 255));
        });
        match &s.ops()[0] {
            DrawOp::Layer { id, clear, ops, alpha, blend } => {
                assert_eq!(*id, 3);
                assert!(!*clear);
                assert_eq!(*alpha, 0.5);
                assert_eq!(*blend, BlendMode::DestinationOut);
                assert_eq!(ops.len(), 1, "nested fill recorded");
                assert!(matches!(ops[0], DrawOp::Fill { .. }));
            }
            other => panic!("expected Layer, got {other:?}"),
        }
    }

    #[test]
    fn layer_op_survives_scene_round_trip() {
        let mut s = Scene::new();
        s.layer(1, true, |l| {
            l.path().add_path(Path::circle(5.0, 5.0, 3.0));
            l.stroke(Paint::eraser(), Stroke::width(2.0));
        });
        let bytes = serde_json::to_vec(&s).expect("serialize");
        let back: Scene = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(s.ops(), back.ops());
    }

    #[test]
    fn invalid_image_buffer_is_flagged() {
        // 2×2 needs 16 bytes; give it 8.
        let bad = ImageSource::from_rgba8(1, 2, 2, vec![0; 8]);
        assert!(!bad.is_valid());
    }

    #[test]
    fn color_into_paint_coercion() {
        let mut s = Scene::new();
        s.path().add_path(Path::circle(5.0, 5.0, 5.0)).fill(Color::new(1, 2, 3, 4));
        assert!(matches!(s.ops()[0], DrawOp::Fill { .. }));
    }

    #[test]
    fn scene_serde_round_trips_through_json() {
        let mut s = Scene::new();
        s.save();
        s.translate(10.0, 20.0);
        s.path().add_path(Path::rounded_rect(0.0, 0.0, 40.0, 30.0, 6.0));
        s.fill(Paint::linear(
            0.0,
            0.0,
            40.0,
            0.0,
            vec![
                GradientStop::new(0.0, Color::new(255, 0, 0, 255)),
                GradientStop::new(1.0, Color::new(0, 0, 255, 255)),
            ],
        ));
        s.stroke(Color::new(0, 0, 0, 255), Stroke::width(2.0).cap(LineCap::Round));
        s.restore();

        let bytes = serde_json::to_vec(&s).expect("serialize");
        let back: Scene = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(s.ops(), back.ops());
    }

    #[test]
    fn solid_color_packs_as_argb_u32() {
        // Verifies the rgba_argb serde shim preserves channels exactly.
        let c = Color::new(0x12, 0x34, 0x56, 0x78);
        let p = Paint::solid(c);
        let json = serde_json::to_string(&p).unwrap();
        let back: Paint = serde_json::from_str(&json).unwrap();
        assert_eq!(back.kind, PaintKind::Solid(c));
    }

    #[test]
    fn blend_builder_and_eraser_helper() {
        let p = Paint::solid(Color::new(10, 20, 30, 255)).blend(BlendMode::Multiply);
        assert_eq!(p.blend, BlendMode::Multiply);

        let e = Paint::eraser();
        assert_eq!(e.blend, BlendMode::DestinationOut);
        // Default constructors are Normal.
        assert_eq!(Paint::solid(Color::BLACK).blend, BlendMode::Normal);
        assert_eq!(
            Paint::radial(0.0, 0.0, 1.0, vec![]).blend,
            BlendMode::Normal
        );
    }

    #[test]
    fn eraser_blend_survives_scene_round_trip() {
        let mut s = Scene::new();
        s.path().add_path(Path::circle(5.0, 5.0, 3.0));
        s.stroke(Paint::eraser(), Stroke::width(4.0));
        let bytes = serde_json::to_vec(&s).expect("serialize");
        let back: Scene = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(s.ops(), back.ops());
        match &back.ops()[0] {
            DrawOp::Stroke { paint, .. } => assert_eq!(paint.blend, BlendMode::DestinationOut),
            other => panic!("expected Stroke, got {other:?}"),
        }
    }

    #[test]
    fn paint_without_blend_field_deserializes_to_normal() {
        // Wire back-compat: a scene recorded before `blend` existed has no
        // `blend` key. `#[serde(default)]` must fill it with `Normal`.
        let legacy = r#"{"kind":{"Solid":305419896}}"#;
        let p: Paint = serde_json::from_str(legacy).expect("legacy paint deserializes");
        assert_eq!(p.blend, BlendMode::Normal);
        assert_eq!(p.kind, PaintKind::Solid(Color::from_argb_u32(0x12345678)));
    }

    #[test]
    fn shapes_records_batch_in_order() {
        let mut s = Scene::new();
        s.shapes([
            ShapeInstance::circle(10.0, 10.0, 5.0, Color::new(255, 0, 0, 255)),
            ShapeInstance::rect(20.0, 20.0, 6.0, 4.0, Color::new(0, 255, 0, 255)),
        ]);
        assert_eq!(s.ops().len(), 1);
        match &s.ops()[0] {
            DrawOp::Shapes { shapes, blend } => {
                assert_eq!(shapes.len(), 2);
                assert_eq!(*blend, BlendMode::Normal);
                // Order preserved: red first (drawn under), green second (on top).
                assert_eq!(shapes[0].color, Color::new(255, 0, 0, 255));
                // rect(20,20,6,4) → center (23,22), half (3,2), sharp.
                assert_eq!((shapes[1].cx, shapes[1].cy), (23.0, 22.0));
                assert_eq!((shapes[1].hw, shapes[1].hh, shapes[1].radius), (3.0, 2.0, 0.0));
            }
            other => panic!("expected Shapes, got {other:?}"),
        }
    }

    #[test]
    fn shapes_with_blend_is_carried() {
        let mut s = Scene::new();
        s.shapes_with([ShapeInstance::circle(0.0, 0.0, 1.0, Color::BLACK)], BlendMode::Multiply);
        match &s.ops()[0] {
            DrawOp::Shapes { blend, .. } => assert_eq!(*blend, BlendMode::Multiply),
            other => panic!("expected Shapes, got {other:?}"),
        }
    }

    #[test]
    fn shape_constructors_and_effective_radius() {
        // circle: square half-extents, full radius.
        let c = ShapeInstance::circle(5.0, 6.0, 4.0, Color::BLACK);
        assert_eq!((c.cx, c.cy, c.hw, c.hh, c.radius), (5.0, 6.0, 4.0, 4.0, 4.0));
        assert_eq!(c.effective_radius(), 4.0);
        // rect from top-left: sharp corners, centered.
        let r = ShapeInstance::rect(0.0, 0.0, 10.0, 8.0, Color::BLACK);
        assert_eq!((r.cx, r.cy, r.hw, r.hh, r.radius), (5.0, 4.0, 5.0, 4.0, 0.0));
        // rounded rect: radius preserved, clamped on use.
        let rr = ShapeInstance::rounded_rect(0.0, 0.0, 10.0, 8.0, 3.0, Color::BLACK);
        assert_eq!(rr.effective_radius(), 3.0);
        // pill: fully rounded on the short axis (height here).
        let p = ShapeInstance::pill(0.0, 0.0, 20.0, 6.0, Color::BLACK);
        assert_eq!(p.radius, 3.0);
        assert_eq!(p.effective_radius(), 3.0);
        // over-large radius clamps to min(hw, hh).
        let over = ShapeInstance::new(0.0, 0.0, 5.0, 3.0, 100.0, Color::BLACK);
        assert_eq!(over.effective_radius(), 3.0);
    }

    #[test]
    fn shape_to_fill_op_matches_hand_authored_fill() {
        // The §7 convergence contract: a batched shape MUST expand to exactly the
        // fill an author would write, so the instanced GPU path and the CPU
        // expand-to-fills path produce identical geometry → identical pixels.

        // Circle case → Path::circle (canonical cubic geometry).
        match ShapeInstance::circle(7.0, 8.0, 4.0, Color::new(1, 2, 3, 255)).to_fill_op(BlendMode::Normal) {
            DrawOp::Fill { path, paint, fill_rule } => {
                assert_eq!(path, Path::circle(7.0, 8.0, 4.0));
                assert_eq!(paint.kind, PaintKind::Solid(Color::new(1, 2, 3, 255)));
                assert_eq!(paint.blend, BlendMode::Normal);
                assert_eq!(fill_rule, FillRule::NonZero);
            }
            other => panic!("expected Fill, got {other:?}"),
        }
        // Sharp rect → Path::rect.
        match ShapeInstance::rect(0.0, 0.0, 10.0, 8.0, Color::BLACK).to_fill_op(BlendMode::Normal) {
            DrawOp::Fill { path, .. } => assert_eq!(path, Path::rect(0.0, 0.0, 10.0, 8.0)),
            other => panic!("expected Fill, got {other:?}"),
        }
        // Rounded rect → Path::rounded_rect.
        match ShapeInstance::rounded_rect(0.0, 0.0, 10.0, 8.0, 3.0, Color::BLACK).to_fill_op(BlendMode::Normal) {
            DrawOp::Fill { path, .. } => assert_eq!(path, Path::rounded_rect(0.0, 0.0, 10.0, 8.0, 3.0)),
            other => panic!("expected Fill, got {other:?}"),
        }
        // Blend carries through to the expanded fill (e.g. an eraser batch).
        match ShapeInstance::circle(0.0, 0.0, 1.0, Color::new(255, 255, 255, 255)).to_fill_op(BlendMode::DestinationOut) {
            DrawOp::Fill { paint, .. } => assert_eq!(paint.blend, BlendMode::DestinationOut),
            other => panic!("expected Fill, got {other:?}"),
        }
    }

    #[test]
    fn shapes_op_survives_scene_round_trip() {
        let mut s = Scene::new();
        s.shapes_with(
            [
                ShapeInstance::circle(10.0, 10.0, 5.0, Color::new(255, 0, 0, 255)),
                ShapeInstance::rounded_rect(20.0, 20.0, 8.0, 6.0, 2.0, Color::new(0, 255, 0, 128)),
            ],
            BlendMode::Screen,
        );
        let bytes = serde_json::to_vec(&s).expect("serialize");
        let back: Scene = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(s.ops(), back.ops());
        // The per-shape color packed/unpacked through the ARGB shim exactly.
        match &back.ops()[0] {
            DrawOp::Shapes { shapes, .. } => {
                assert_eq!(shapes[1].color, Color::new(0, 255, 0, 128));
                assert_eq!(shapes[1].radius, 2.0);
            }
            other => panic!("expected Shapes, got {other:?}"),
        }
    }

    #[test]
    fn transform_then_matches_manual_point_map() {
        // self = translate(10,0), next = scale(2,2).
        // Apply self first: (1,1) -> (11,1) -> scale -> (22,2).
        let m = Transform::translate(10.0, 0.0).then(Transform::scale(2.0, 2.0));
        let (x, y) = (1.0_f32, 1.0_f32);
        let px = m.a * x + m.c * y + m.e;
        let py = m.b * x + m.d * y + m.f;
        assert_eq!((px, py), (22.0, 2.0));
    }
}
