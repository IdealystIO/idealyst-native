//! `charts` — reactive charting for idealyst.
//!
//! The author-facing layer over [`charts_core`]: it renders that crate's
//! mark IR onto a `Canvas`, materializes labels as real `text` primitives,
//! and drives a tooltip from the hit index.
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
//! three components share a canvas, an adapter, and a tooltip.
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
//! Pick a canvas renderer at app bootstrap exactly as any other canvas
//! consumer does — `canvas_native::register` or `canvas_vello::register`.
//! A chart is a canvas author; it installs nothing of its own.
#![deny(missing_debug_implementations)]

pub(crate) mod adapt;
mod chart;
mod polar_chart;

pub use adapt::marks_into_scene;
pub use chart::{
    Chart, ChartHover, ChartProps, HoverCallback, TooltipRenderer, DEFAULT_X_AXIS_HEIGHT,
    DEFAULT_Y_AXIS_WIDTH,
};
pub use polar_chart::{
    polar_hover_at, PieChart, PieChartProps, PolarHover, PolarHoverCallback, PolarTooltipRenderer,
    RadialChart, RadialChartProps,
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
    HitResult, Interpolation, LineStyle, MarkContext, MarkOverride, PieLabels, PieSpec, PointShape,
    PointStyle, PolarOutput, RadialBar, RadialSpec, Ring, Series, SeriesKind, Slice, SliceContext,
    SliceHighlight, SliceOverride, SliceStyleFn, StepAt, StyleFn, Tick,
};

/// The small namespace a screen imports.
pub mod prelude {
    pub use super::{
        Chart, ChartHover, ChartProps, PieChart, PieChartProps, PolarHover, RadialChart,
        RadialChartProps,
    };
    pub use charts_core::{
        cell, datum, Annotation, AnnotationAt, AreaFill, AreaStyle, Axis, AxisKind, BarLayout,
        BarStyle, ChartSpec, Color, ColorRamp, Datum, DatumRef, Domain, HeatmapStyle, Highlight,
        Interpolation, LineStyle, MarkContext, MarkOverride, PieLabels, PieSpec, PointShape,
        PointStyle, RadialBar, RadialSpec, Ring, Series, SeriesKind, Slice, SliceContext,
        SliceHighlight, SliceOverride, SliceStyleFn, StepAt, StyleFn,
    };
}
