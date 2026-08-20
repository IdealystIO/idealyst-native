//! `charts` — reactive charting for idealyst.
//!
//! The author-facing layer over [`charts_core`]: it renders that crate's
//! mark IR onto a `Canvas`, materializes axis labels as real `text`
//! primitives, and drives a tooltip from the hit index.
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

pub use adapt::marks_into_scene;
pub use chart::{
    Chart, ChartHover, ChartProps, HoverCallback, TooltipRenderer, DEFAULT_X_AXIS_HEIGHT,
    DEFAULT_Y_AXIS_WIDTH,
};

#[doc(hidden)]
pub mod __test_support {
    //! Pure helpers the integration suite exercises directly. Not a public
    //! API — exposed only so the tests can reach the logic without a
    //! backend or a synthetic event stream.
    pub use crate::chart::{hover_at, visual_state};
}

// The spec/scale/hit surface is re-exported so a screen imports one crate.
pub use charts_core::{
    datum, render, render_with, AreaFill, AreaStyle, Axis, AxisKind, BarLayout, BarStyle,
    ChartOutput, ChartSpec, Color, Datum, DatumRef, Domain, Emphasis, Gutters, Highlight, HitIndex,
    HitResult, LineStyle, MarkContext, MarkOverride, PointShape, PointStyle, Ring, Series,
    SeriesKind, StyleFn, Tick,
};

/// The small namespace a screen imports.
pub mod prelude {
    pub use super::{Chart, ChartHover, ChartProps};
    pub use charts_core::{
        datum, AreaFill, AreaStyle, Axis, AxisKind, BarLayout, BarStyle, ChartSpec, Color, Datum,
        DatumRef, Domain, Highlight, LineStyle, MarkContext, MarkOverride, PointShape, PointStyle,
        Ring, Series, SeriesKind, StyleFn,
    };
}
