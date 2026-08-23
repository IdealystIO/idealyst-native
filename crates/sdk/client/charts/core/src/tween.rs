//! Interpolating between two chart states.
//!
//! Pure, like everything else in this crate: `t` is an input, not something
//! read from a clock. That is what keeps a mid-transition frame as testable
//! as any other render — and it is why the driver (a frame loop) lives in the
//! host rather than here.
//!
//! # `t` arrives ALREADY EASED
//!
//! This crate applies no curve of its own. Every `t` here is a fraction of
//! the way through a transition *after* its easing function has been applied,
//! so `0.5` means "half way there", not "half way through the clock".
//!
//! That boundary is deliberate. The host declares transitions with the
//! framework's own `Transition { duration_ms, easing }` vocabulary, and
//! `Easing` is a runtime-shared type this crate cannot see — `charts-core`
//! depends on no runtime crate, which is what lets it render to an SVG string
//! with no toolkit present. Owning a duplicate easing enum here would fork
//! that vocabulary in two; taking a pre-eased scalar keeps one vocabulary and
//! leaves this crate doing nothing but geometry.
//!
//! # Two channels, two clocks
//!
//! Values and colors transition independently — see [`TweenAt`]. A chart
//! whose bars glide over 420 ms while a threshold recolor fades over 150 ms
//! is one render per frame with two fractions, not two renders.
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

use crate::scene::Color;
use crate::spec::{ChartSpec, Datum};

/// How far through each transition channel a frame is. Both are ALREADY
/// EASED fractions in `0..=1` — see the module docs.
///
/// Separate fields rather than one `t` because the two channels are declared
/// with their own `Transition` and therefore have their own duration and
/// curve. `1.0` means "settled", which is what a channel with no transition
/// declared always passes.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct TweenAt {
    /// Datum values (x / y / heatmap intensity) and the axis domain.
    ///
    /// The domain rides the value clock rather than owning one: an axis
    /// settling on a different beat from the marks it measures reads as a
    /// glitch, not as a flourish.
    pub value: f32,
    /// Series colors and whatever a [`StyleFn`](crate::spec::StyleFn)
    /// resolves to.
    pub color: f32,
}

impl TweenAt {
    /// Both channels settled — a plain, un-animated render.
    pub const SETTLED: Self = Self { value: 1.0, color: 1.0 };

    /// One fraction driving both channels.
    pub const fn uniform(t: f32) -> Self {
        Self { value: t, color: t }
    }
}

impl Default for TweenAt {
    fn default() -> Self {
        Self::SETTLED
    }
}

/// Interpolate two colors channel-wise, including alpha.
///
/// Plain sRGB component lerp, not OKLab or premultiplied. Chart transitions
/// are short and usually run between two colors of similar lightness (a
/// threshold recolor, a palette swap), where the muddy midpoint a naive lerp
/// is criticised for does not have time to be visible. A perceptual space
/// would mean a color-science dependency in a crate that deliberately has
/// none.
pub fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let ch = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    Color { r: ch(a.r, b.r), g: ch(a.g, b.g), b: ch(a.b, b.b), a: ch(a.a, b.a) }
}

/// Linear interpolation, clamped so an overshooting `t` cannot produce
/// values outside the endpoints.
pub fn lerp_f64(a: f64, b: f64, t: f32) -> f64 {
    a + (b - a) * t.clamp(0.0, 1.0) as f64
}

/// Cubic ease-in-out, exposed for a host that wants the old default curve
/// without pulling in the framework's `Easing`.
///
/// NOT applied anywhere in this crate — see the module docs on pre-eased `t`.
/// It is here because an SVG-only consumer with no runtime has no other
/// source for a curve.
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
/// Both `at.value` and `at.color` are pre-eased fractions (module docs).
///
/// # What animates, and what does not
///
/// - **Values** ride `at.value`: each datum's `x` / `y` / `w`.
/// - **Series colors** ride `at.color`.
/// - **Everything else comes from `to`**: visibility, highlight, axis
///   configuration, `style_fn` identity. Highlight in particular is instant
///   on purpose — a point becoming selected is a state change the user just
///   caused, and fading into it makes the UI feel unresponsive rather than
///   smooth.
///
/// A [`StyleFn`](crate::spec::StyleFn)'s own answer is NOT resolved here. It
/// depends on the datum, so interpolating its input would make a threshold
/// flip at whatever frame the tweened value crosses it — a hard switch in the
/// middle of a smooth transition. It is instead resolved at BOTH ends during
/// the render and the two results interpolated, the same way
/// [`render_tween`](crate::render::render_tween) already treats the axis
/// domain.
pub fn lerp_data(from: &ChartSpec, to: &ChartSpec, at: TweenAt) -> Option<ChartSpec> {
    if !same_shape(from, to) {
        return None;
    }
    let mut out = to.clone();
    for (i, s) in out.series.iter_mut().enumerate() {
        let src = &from.series[i];
        s.color = lerp_color(src.color, s.color, at.color);
        for (j, d) in s.data.iter_mut().enumerate() {
            let a = src.data[j];
            *d = Datum {
                x: lerp_f64(a.x, d.x, at.value),
                y: lerp_f64(a.y, d.y, at.value),
                // Heatmap intensity animates like any other value; for every
                // other kind this channel is zero at both ends.
                w: lerp_f64(a.w, d.w, at.value),
            };
        }
    }
    Some(out)
}
