//! `charts-core` — renderer-agnostic charting.
//!
//! Takes a spec and a pixel rectangle, returns a [`ChartScene`]:
//! a flat list of vector marks, a list of text *placements* (not glyphs),
//! and a [`HitIndex`] for pointer interaction. It draws nothing itself and
//! knows about no UI toolkit, so the same render feeds a GPU canvas, an SVG
//! string, or a native component tree.
//!
//! Two spec families, one output. [`ChartSpec`] is cartesian — line, area,
//! bar, scatter, heatmap — and goes through [`render`]. [`PieSpec`] and
//! [`RadialSpec`] are polar, and go through [`render_pie`] /
//! [`render_radial`]. They are separate types because a pie inherits none of
//! the cartesian machinery (two axes, a domain each, tick selection, gutters,
//! bar-slot math, column hit-testing); see [`polar`] for the full argument.
//! What they share is everything below the spec, so every renderer and every
//! host serves both without knowing which it is drawing.
//!
//! # Why the seam is here
//!
//! The obvious alternative is a drawing-backend trait, as plotters uses.
//! We deliberately rejected that shape:
//!
//! - Its coordinates are `(i32, i32)`, and the quantization lives in the
//!   scale trait rather than the backend, so marks snap to whole logical
//!   pixels and jitter under animation on high-DPR displays.
//! - `draw_pixel` is required, and the default path/rect/circle impls fall
//!   back to per-pixel rasterization — a forgotten override degrades
//!   silently instead of failing.
//! - Paths are polyline-only. Smooth lines, rounded bars, and arcs are all
//!   inexpressible.
//! - Text arrives as a per-pixel callback with a font *name* and no font
//!   handle, which forces every consumer to own a font stack and rules out
//!   native text nodes entirely.
//!
//! Emitting data instead of driving a trait avoids all four, and makes the
//! entire output of a render comparable with `==` — so the tests here are
//! exact goldens rather than pixel diffs.
//!
//! # Example
//!
//! ```
//! use charts_core::{render, ChartSpec, Series, SeriesKind, Color, Rect, datum};
//!
//! let spec = ChartSpec::new(vec![Series::new(
//!     "revenue",
//!     SeriesKind::line(),
//!     Color::rgb(0x4c, 0x8d, 0xff),
//!     vec![datum(0.0, 10.0), datum(1.0, 30.0), datum(2.0, 20.0)],
//! )]);
//!
//! let out = render(&spec, Rect::new(0.0, 0.0, 400.0, 300.0));
//! assert!(!out.scene.marks.is_empty());
//! assert!(!out.scene.labels.is_empty());
//! ```
#![deny(missing_debug_implementations)]

pub mod hit;
pub mod pie;
pub mod polar;
pub mod radial;
pub mod render;
pub mod scale;
pub mod scene;
pub mod spec;
pub mod svg;
pub mod tween;

pub use hit::{HitIndex, HitResult, MarkBounds};
pub use pie::{
    lerp_pie, render_pie, render_pie_tween, PieLabels, PieSpec, Slice, SliceContext, SliceOverride,
    SliceStyleFn,
};
pub use polar::{PolarOutput, SliceHighlight};
pub use radial::{lerp_radial, render_radial, render_radial_tween, RadialBar, RadialSpec};
pub use render::{
    render, render_tween, render_with, ChartOutput, Gutters, LabelMetrics, Padding,
};
pub use tween::{ease_in_out, lerp_color, lerp_data, same_shape, TweenAt};
pub use scale::{ResolvedAxis, Tick};
pub use scene::{
    pt, ChartScene, Color, FillRule, GradientStop, HAlign, LabelPlacement, LabelRole, Layer,
    LineCap, LineJoin, Mark, Paint, Path, PathSeg, Point, PointInstance, Rect, Stroke, VAlign,
};
pub use spec::{
    cell, datum, Annotation, AnnotationAt, AreaFill, AreaStyle, Axis, AxisKind, BarLayout,
    BarStyle, ChartSpec, ColorRamp, Datum, DatumRef, Domain, Emphasis, HeatmapStyle, Highlight,
    Interpolation, LineStyle, MarkContext, MarkOverride, PointShape, PointStyle, Ring, Series,
    SeriesKind, StepAt, StyleFn,
};
pub use polar::{arc_to, point_on, wedge_path};
pub use svg::{scene_to_svg, to_svg, ApproxMetrics};
