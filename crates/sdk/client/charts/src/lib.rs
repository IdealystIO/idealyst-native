//! `charts` — reactive charting for idealyst.
//!
//! The author-facing layer over [`charts_core`]: it renders that crate's
//! mark IR onto a `Canvas`, materializes labels as real `text` primitives,
//! and reports hover from the hit index.
//!
//! Three components, one per spec type:
//!
//! - [`Chart`] — cartesian: line, area, bar, scatter, heatmap.
//! - [`PieChart`] — pie and donut.
//! - [`RadialChart`] — radial bars and gauges.
//!
//! They are separate because the specs are: a pie has no axes, no domain and
//! no columns, so `charts_core::polar` gives it its own type rather than
//! leaving half of `ChartSpec` meaningless. Everything below the spec — the
//! mark IR, the hit index, the label protocol — is shared, which is why the
//! three components share a canvas, an adapter, and a [`PointerFrame`].
//!
//! ```ignore
//! use charts::prelude::*;
//!
//! let data = signal(vec![datum(0.0, 3.0), datum(1.0, 7.0), datum(2.0, 5.0)]);
//!
//! ui! {
//!     view() {
//!         Chart(spec = rx!(ChartSpec::new(vec![Series::new(
//!             "revenue",
//!             SeriesKind::smooth_line(),
//!             Color::rgb(0x4c, 0x8d, 0xff),
//!             data.get(),
//!         )])))
//!     }
//! }
//! ```
//!
//! # No built-in tooltip
//!
//! These components render **no hover surface**. [`ChartProps::on_hover`] is
//! the whole mechanism; an app renders its own tooltip, outside the chart's
//! tree. A tooltip is composable from a callback plus a surface, so it
//! belongs to the app — and owning one here meant clipping it against the
//! plot's `overflow: hidden`, hardcoding colors that are wrong in half the
//! themes it lands in, and privileging cursor-following over snap-to-mark
//! and pinned-axis placements that real charts use just as often.
//!
//! What is provided instead is enough to place a surface anywhere:
//! [`PointerFrame`] carries the pointer in plot-local AND window space plus
//! the plot's viewport rect, and each [`HitResult`] carries its anchor
//! ([`HitResult::position`]) and its drawn geometry
//! ([`MarkBounds`]). `examples/charts-demo` implements
//! cursor-following, snap-to-mark and track-x placements on one callback.
//!
//! # Transitions
//!
//! Declared with the framework's own [`Transition`] and [`Easing`] — one
//! vocabulary, so `Transition::new(420, Easing::EaseInOut)` means here what it
//! means on a stylesheet's `background_transition`. Two channels, because one
//! duration cannot suit both:
//!
//! - [`ChartProps::value_transition`] — datum values and the axis domain.
//! - [`ChartProps::color_transition`] — series colours and whatever a
//!   [`StyleFn`] resolves to, so a threshold recolor fades rather than
//!   flipping at the frame the tweened value crosses it.
//!
//! `None` on either means snap. Highlight never animates. Only the mechanism
//! differs from a style transition: marks are painted into a canvas, so the
//! chart drives its own frame loop instead of handing the backend a CSS
//! transition.
//!
//! Pick a canvas renderer at app bootstrap exactly as any other canvas
//! consumer does — `canvas_native::register` or `canvas_vello::register`.
//! A chart is a canvas author; it installs nothing of its own.
#![deny(missing_debug_implementations)]

pub(crate) mod adapt;
mod chart;
mod polar_chart;

pub use adapt::marks_into_scene;
pub use chart::{
    channel_at, transition_span_ms, tween_at, Chart, ChartHover, ChartProps, HoverCallback,
    PointerFrame, DEFAULT_X_AXIS_HEIGHT, DEFAULT_Y_AXIS_WIDTH,
};
pub use polar_chart::{
    polar_hover_at, PieChart, PieChartProps, PolarHover, PolarHoverCallback, RadialChart,
    RadialChartProps,
};

#[doc(hidden)]
pub mod __test_support {
    //! Pure helpers the integration suite exercises directly. Not a public
    //! API — exposed only so the tests can reach the logic without a
    //! backend or a synthetic event stream.
    pub use crate::chart::{gutters_for, hover_at, visual_state};
    pub use crate::polar_chart::polar_hover_at;
}

// The spec/scale/hit surface is re-exported so a screen imports one crate.
pub use charts_core::{
    cell, datum, render, render_pie, render_radial, render_with, Annotation, AnnotationAt,
    AreaFill, AreaStyle, Axis, AxisKind, BarLayout, BarStyle, ChartOutput, ChartSpec, Color,
    ColorRamp, Datum, DatumRef, Domain, Emphasis, Gutters, HeatmapStyle, Highlight, HitIndex,
    HitResult, Interpolation, LineStyle, MarkBounds, MarkContext, MarkOverride, PieLabels, PieSpec,
    PointShape,
    PointStyle, PolarOutput, Point, RadialBar, RadialSpec, Ring, Series, SeriesKind, Slice,
    SliceContext, SliceHighlight, SliceOverride, SliceStyleFn, StepAt, StyleFn, Tick, TweenAt,
};

// `PointerFrame::plot` is a `ViewportRect`, so a caller placing a surface has
// to be able to name the type. Re-exported here rather than left to a
// `runtime_core` import, for the same "a screen imports one crate" reason the
// core surface above is re-exported.
pub use runtime_core::ViewportRect;

// Transitions are declared with the framework's OWN vocabulary rather than a
// charting dialect: `Transition::new(420, Easing::EaseInOut)` means the same
// thing on `ChartProps::value_transition` as it does on a stylesheet's
// `background_transition`. Only the mechanism differs — marks are painted
// into a canvas, so the chart drives its own frame loop instead of handing
// the backend a CSS transition — and that is an implementation detail the
// author never spells.
pub use runtime_core::{Easing, Transition};

/// The small namespace a screen imports.
pub mod prelude {
    pub use super::{
        Chart, ChartHover, ChartProps, Easing, PieChart, PieChartProps, PointerFrame, PolarHover,
        RadialChart, RadialChartProps, Transition, ViewportRect,
    };
    pub use charts_core::{
        cell, datum, Annotation, AnnotationAt, AreaFill, AreaStyle, Axis, AxisKind, BarLayout,
        BarStyle, ChartSpec, Color, ColorRamp, Datum, DatumRef, Domain, HeatmapStyle, Highlight,
        Interpolation, LineStyle, MarkBounds, MarkContext, MarkOverride, PieLabels, PieSpec,
        Point, PointShape, PointStyle, RadialBar, RadialSpec, Ring, Series, SeriesKind, Slice,
        SliceContext, SliceHighlight, SliceOverride, SliceStyleFn, StepAt, StyleFn,
    };
}
