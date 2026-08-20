//! Interpolating between two chart states.
//!
//! Pure, like everything else in this crate: `t` is an input, not something
//! read from a clock. That is what keeps a mid-transition frame as testable
//! as any other render — and it is why the driver (a frame loop) lives in the
//! host rather than here.
//!
//! # Why the DATA, not the marks
//!
//! Interpolating the mark IR sounds more general — it could morph a bar
//! chart into a line — but it does not work. Two renders' mark lists differ
//! in count and in kind, matching them up is ill-defined, and a mismatched
//! lerp produces geometry that corresponds to no real chart.
//!
//! Interpolating the data instead means every frame is a genuine render, so
//! axes, gridlines, labels and the hit index are all correct by construction,
//! and every series kind animates with no per-kind code.

use crate::spec::{ChartSpec, Datum};

/// Linear interpolation, clamped so an overshooting `t` cannot produce
/// values outside the endpoints.
pub fn lerp_f64(a: f64, b: f64, t: f32) -> f64 {
    a + (b - a) * t.clamp(0.0, 1.0) as f64
}

/// Cubic ease-in-out. The default shape for a chart transition: data should
/// leave and arrive at rest, not start and stop abruptly.
pub fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// Whether two specs have the same shape, and can therefore be interpolated
/// point-for-point.
///
/// Series are matched by POSITION, and a mismatch anywhere makes the pair
/// un-interpolable. Interpolating across a different shape would mean pairing
/// unrelated points — a bar sliding to a value from a series it has nothing
/// to do with — which reads worse than not animating at all.
pub fn same_shape(a: &ChartSpec, b: &ChartSpec) -> bool {
    a.series.len() == b.series.len()
        && a.series
            .iter()
            .zip(&b.series)
            .all(|(x, y)| x.data.len() == y.data.len() && x.kind == y.kind)
}

/// Interpolate `from` toward `to`, returning `None` when the two cannot be
/// paired (see [`same_shape`]) — the caller then snaps.
///
/// Everything that is not a data value comes from `to`: colors, visibility,
/// highlight, axis configuration. A transition animates VALUES; a series
/// changing color or a point becoming selected should take effect at once,
/// not fade through an intermediate that was never asked for.
pub fn lerp_data(from: &ChartSpec, to: &ChartSpec, t: f32) -> Option<ChartSpec> {
    if !same_shape(from, to) {
        return None;
    }
    let e = ease_in_out(t);
    let mut out = to.clone();
    for (i, s) in out.series.iter_mut().enumerate() {
        let src = &from.series[i].data;
        for (j, d) in s.data.iter_mut().enumerate() {
            let a = src[j];
            *d = Datum {
                x: lerp_f64(a.x, d.x, e),
                y: lerp_f64(a.y, d.y, e),
                // Heatmap intensity animates like any other value; for every
                // other kind this channel is zero at both ends.
                w: lerp_f64(a.w, d.w, e),
            };
        }
    }
    Some(out)
}
