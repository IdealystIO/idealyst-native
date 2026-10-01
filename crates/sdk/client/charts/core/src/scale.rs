//! Domain resolution, tick selection, and value <-> pixel mapping.
//!
//! Tick VALUES and labels come from [`crate::ticks`]; this module settles
//! the domain (auto-fit, outward rounding, log clamping) and owns the
//! value <-> pixel mapping. Mapping is `f64` in, `f32` out, with no
//! quantization: snapping marks to whole logical pixels would show as
//! jitter on high-DPR displays and during pan/zoom animation.

use crate::spec::{Axis, AxisKind, Domain};
use crate::ticks;

/// One tick: where it sits in data space, and its rendered label.
#[derive(Clone, PartialEq, Debug)]
pub struct Tick {
    pub value: f64,
    pub label: String,
}

/// How data values are distributed across the axis.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Transform {
    Linear,
    Log10,
}

impl Transform {
    fn fwd(self, v: f64) -> f64 {
        match self {
            Transform::Linear => v,
            // Guard the domain: log of a non-positive value is undefined, and
            // letting -inf/NaN through would poison every downstream pixel
            // coordinate. Such points are clipped by the renderer instead.
            Transform::Log10 => {
                if v > 0.0 {
                    v.log10()
                } else {
                    f64::NAN
                }
            }
        }
    }

    fn inv(self, v: f64) -> f64 {
        match self {
            Transform::Linear => v,
            Transform::Log10 => 10f64.powf(v),
        }
    }
}

/// An axis with its domain pinned down and its ticks chosen.
#[derive(Clone, PartialEq, Debug)]
pub struct ResolvedAxis {
    pub min: f64,
    pub max: f64,
    pub ticks: Vec<Tick>,
    /// Number of category slots, when this is a category axis. Bar width
    /// math needs it and would otherwise have to re-inspect the spec.
    pub categories: Option<usize>,
    transform: Transform,
}

impl ResolvedAxis {
    /// Map a data value to a pixel between `lo_px` and `hi_px`.
    ///
    /// `f32` throughout. Callers pass the two ends in the order the pixel
    /// axis runs, so a y-axis is mapped by passing `(bottom, top)` and the
    /// downward-growing screen y falls out with no special case here.
    pub fn map(&self, v: f64, lo_px: f32, hi_px: f32) -> f32 {
        let (a, b) = (self.transform.fwd(self.min), self.transform.fwd(self.max));
        let t = self.transform.fwd(v);
        if !t.is_finite() || (b - a).abs() < f64::EPSILON {
            return lo_px;
        }
        let frac = ((t - a) / (b - a)) as f32;
        lo_px + (hi_px - lo_px) * frac
    }

    /// Inverse of [`map`](Self::map) — the basis of pointer hit-testing and
    /// of anchoring a zoom about the cursor.
    pub fn unmap(&self, px: f32, lo_px: f32, hi_px: f32) -> f64 {
        if (hi_px - lo_px).abs() < f32::EPSILON {
            return self.min;
        }
        let frac = ((px - lo_px) / (hi_px - lo_px)) as f64;
        let (a, b) = (self.transform.fwd(self.min), self.transform.fwd(self.max));
        self.transform.inv(a + (b - a) * frac)
    }

    /// The resolved window, as a `Domain` ready to be handed to
    /// [`Domain::translate`](crate::spec::Domain::translate) or
    /// [`Domain::zoom`](crate::spec::Domain::zoom). This is the seam a
    /// pan/zoom addon writes through: resolve, transform, store back on the
    /// axis, re-render.
    pub fn domain(&self) -> Domain {
        Domain::Fixed { min: self.min, max: self.max }
    }

    /// True when the value cannot be positioned on this axis (non-positive
    /// on a log scale). The renderer skips such points rather than drawing
    /// them at a bogus coordinate.
    pub fn is_plottable(&self, v: f64) -> bool {
        self.transform.fwd(v).is_finite()
    }
}

/// The min/max of a set of values, or `None` if there were none.
fn extent(values: impl Iterator<Item = f64>) -> Option<(f64, f64)> {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    let mut any = false;
    for v in values.filter(|v| v.is_finite()) {
        lo = lo.min(v);
        hi = hi.max(v);
        any = true;
    }
    any.then_some((lo, hi))
}

/// Widen a degenerate or empty extent into something with nonzero width.
///
/// A single data point, or a series where every y is identical, otherwise
/// produces `min == max` and a division by zero in `map`. Expanding by 1
/// (or by 5% for a nonzero constant) keeps the mark centered rather than
/// pinned to an edge.
fn ensure_width(lo: f64, hi: f64) -> (f64, f64) {
    if (hi - lo).abs() > f64::EPSILON {
        return (lo, hi);
    }
    if lo.abs() < f64::EPSILON {
        (-1.0, 1.0)
    } else {
        let pad = lo.abs() * 0.05;
        (lo - pad, hi + pad)
    }
}

/// Choose ticks for a numeric range.
fn numeric_ticks(min: f64, max: f64, want: usize) -> Vec<Tick> {
    ticks::linear(min, max, want)
}

/// The widest log span an axis may have, in decades.
///
/// 12 decades is already far past any real chart (a picoamp-to-amp axis is
/// 12). Without a cap, an all-non-positive data set or an absurd hand-set
/// `Domain::Fixed` produces a ~308-decade domain, for which `end / start`
/// overflows f64 — the log tick walk used to hang outright on it (plotters
/// 0.3.7 saturated `inf as usize` into a ~3.7e18-iteration loop; the
/// in-house [`ticks::log`] returns no ticks instead). Clamping keeps the
/// domain meaningful AND the ticks present.
const MAX_LOG_DECADES: f64 = 12.0;

/// Constrain a log range to at most [`MAX_LOG_DECADES`], keeping the top
/// of the range (the large values are the ones a reader is looking at).
fn clamp_log_range(min: f64, max: f64) -> (f64, f64) {
    let hi = if max > 0.0 { max } else { 1.0 };
    let floor = hi / 10f64.powf(MAX_LOG_DECADES);
    let lo = if min > 0.0 { min.max(floor) } else { floor };
    // Still guarantee a nonzero span after clamping.
    if hi > lo {
        (lo, hi)
    } else {
        (hi / 10.0, hi)
    }
}

/// Choose ticks for a log range: decade points, plus in-decade points when
/// the tick budget allows.
fn log_ticks(min: f64, max: f64, want: usize) -> Vec<Tick> {
    let (lo, hi) = clamp_log_range(min, max);
    ticks::log(lo, hi, want)
}

/// Choose ticks for a time range (milliseconds since the Unix epoch).
fn time_ticks(min_ms: f64, max_ms: f64, want: usize) -> Vec<Tick> {
    ticks::time(min_ms, max_ms, want)
}

/// Resolve one axis: settle its domain, then choose ticks within it.
///
/// `values` supplies the data extent, used only when the domain is `Auto`.
/// A `Fixed` domain is taken verbatim — that is what makes pan/zoom work
/// without the data pulling the viewport back.
pub fn resolve(axis: &Axis, values: impl Iterator<Item = f64>) -> ResolvedAxis {
    resolve_with_ticks(axis, values, None)
}

/// [`resolve`], but with the tick VALUES supplied from elsewhere.
///
/// Exists for transitions. While a chart animates, its domain is somewhere
/// between the old and the new one, and choosing ticks from that intermediate
/// range makes the labels churn through arbitrary values — `3.7`, `7.4` —
/// for the length of the animation. Passing the destination's ticks instead
/// keeps the labels stable from the first frame while the gridlines slide
/// smoothly into place. Ticks outside the current window are culled by the
/// renderer's existing range check.
pub fn resolve_with_ticks(
    axis: &Axis,
    values: impl Iterator<Item = f64>,
    forced: Option<&[Tick]>,
) -> ResolvedAxis {
    if let AxisKind::Category(cats) = &axis.kind {
        let n = cats.len();
        // Half-slot padding at each end so the first and last bars are not
        // flush against the plot edge.
        return ResolvedAxis {
            min: -0.5,
            max: n as f64 - 0.5,
            ticks: forced.map(|t| t.to_vec()).unwrap_or_else(|| cats
                .iter()
                .enumerate()
                .map(|(i, c)| Tick { value: i as f64, label: c.clone() })
                .collect()),
            categories: Some(n),
            transform: Transform::Linear,
        };
    }

    let transform = match axis.kind {
        AxisKind::Log => Transform::Log10,
        _ => Transform::Linear,
    };

    let ticks_for = |lo: f64, hi: f64| match axis.kind {
        AxisKind::Log => log_ticks(lo, hi, axis.tick_count),
        AxisKind::Time => time_ticks(lo, hi, axis.tick_count),
        _ => numeric_ticks(lo, hi, axis.tick_count),
    };

    let (min, max) = match axis.domain {
        // A Fixed domain is a viewport, honored verbatim — that is the
        // contract pan/zoom depends on. It is still range-checked for log,
        // since a hand-set domain can be nonsense.
        Domain::Fixed { min, max } => {
            if transform == Transform::Log10 {
                clamp_log_range(min, max)
            } else {
                (min, max)
            }
        }
        Domain::Auto => {
            // On a log axis only positive data can contribute to the
            // extent. Taking the raw min would put the floor at or below
            // zero, and clamping THAT back up lands on an absurd lower
            // bound (a previous cut used `f64::MIN_POSITIVE`, producing a
            // 308-decade span — see MAX_LOG_DECADES).
            let extent_of = if transform == Transform::Log10 {
                extent(values.filter(|v| *v > 0.0))
            } else {
                extent(values)
            };

            let (mut lo, mut hi) = extent_of.unwrap_or(if transform == Transform::Log10 {
                (1.0, 100.0)
            } else {
                (0.0, 1.0)
            });
            if axis.include_zero && transform == Transform::Linear {
                lo = lo.min(0.0);
                hi = hi.max(0.0);
            }
            let (lo, hi) = if transform == Transform::Log10 {
                // A single positive value gives lo == hi; widen by a decade
                // either side so the point sits mid-axis.
                if (hi - lo).abs() < f64::EPSILON {
                    (lo / 10.0, hi * 10.0)
                } else {
                    (lo, hi)
                }
            } else {
                ensure_width(lo, hi)
            };

            // Round OUTWARD to the next tick boundary, so the extremes sit
            // inside the axis rather than exactly on its edge.
            //
            // Taking `hi.max(last_tick)` is not enough and was a real bug:
            // tick selection only returns values INSIDE the range, so the
            // last tick is always <= hi and the domain stays pinned to the
            // raw data max. The topmost data point then lands exactly on the
            // plot's top edge, and since the plot clips its overflow, the
            // outer half of a line's stroke width is shaved off — a visibly
            // flat-topped peak.
            //
            // Only for Auto: a Fixed domain is a viewport and is honored
            // exactly, or pan/zoom would fight this rounding every frame.
            let probe = ticks_for(lo, hi);
            let step = match probe.as_slice() {
                [a, b, ..] => (b.value - a.value).abs(),
                _ => 0.0,
            };
            if step > 0.0 && transform == Transform::Linear {
                // `- f64::EPSILON` on the ceil guard so a value already
                // sitting exactly on a tick does not gain a whole empty
                // step above it.
                let up = (hi / step - f64::EPSILON).ceil() * step;
                let down = (lo / step + f64::EPSILON).floor() * step;
                (down.min(lo), up.max(hi))
            } else {
                match (probe.first(), probe.last()) {
                    (Some(f), Some(l)) if l.value > f.value => (lo.min(f.value), hi.max(l.value)),
                    _ => (lo, hi),
                }
            }
        }
    };

    let (min, max) = if transform == Transform::Log10 {
        clamp_log_range(min, max)
    } else {
        (min, max)
    };

    let ticks = match forced {
        Some(t) => t.to_vec(),
        None => ticks_for(min, max),
    };
    ResolvedAxis { min, max, ticks, categories: None, transform }
}
