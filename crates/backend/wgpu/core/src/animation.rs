//! Tween engine for native-widget rendering.
//!
//! Widgets in this backend (`Toggle`, `Slider`, future `Button`
//! press scale, focus rings, presence transitions, …) often want
//! smooth value transitions that the framework's style-driven
//! transition system can't drive — because those widgets paint
//! themselves rather than expose every visual property as a
//! styleable rule.
//!
//! [`Animator`] is a small per-backend side-table of active
//! tweens keyed by `(LayoutNode, AnimProperty)`. The widget paint
//! code samples it on every frame; the backend ticks it forward
//! before each frame and reports whether anything is still
//! animating so the event loop knows to schedule another redraw.
//!
//! # Lifecycle
//!
//! 1. Some backend event (`update_toggle_value`, press state flip,
//!    …) calls [`Animator::animate`] to start a tween. If a tween
//!    for the same key already exists, the new tween starts from
//!    the current interpolated value — flipping the toggle
//!    mid-slide is smooth.
//! 2. On every render, the renderer calls
//!    [`Animator::sample`] to get the current value, falling back
//!    to a static value if no tween exists.
//! 3. After rendering, the event loop calls [`Animator::tick`] to
//!    purge completed tweens. If it returns `true`, the loop
//!    requests another redraw.
//! 4. When a node drops (via `clear_children`), call
//!    [`Animator::drop_node`] to clear any tweens against that
//!    node so they don't keep firing redraws for dead nodes.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use framework_core::Easing;
use native_layout::LayoutNode;

/// Which property of a node a tween targets.
///
/// Variants split into two families by storage backing inside
/// [`Animator`]:
/// - **Scalar (`f32`)** — widget animations driven by a single
///   normalized parameter. Toggle thumb, slider thumb, press
///   scale. Stored in `Animator::scalars`.
/// - **Color (`[f32; 4]`)** — style-driven color transitions
///   (background, text, per-side borders). Stored in
///   `Animator::colors`.
///
/// The animator routes via the variant. Callers reach for the
/// matching `animate_f32` / `animate_color` (and `sample_f32` /
/// `sample_color`) method; passing the wrong variant to the
/// wrong method is a no-op (sample returns fallback, animate
/// silently does the wrong thing). Keeping one enum across both
/// families simplifies the `drop_node` cleanup path.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum AnimProperty {
    // --- Scalar (f32) ---
    /// Toggle thumb position. `0.0` = OFF (thumb at left), `1.0` =
    /// ON (thumb at right). Track color interpolates with the
    /// same `t`.
    ToggleThumb,
    /// Slider thumb position. Reserved for future programmatic
    /// value changes.
    SliderThumb,
    /// Button / Pressable press scale (1.0 = at rest, < 1.0 =
    /// pressed). Reserved.
    PressScale,

    // --- Color ([f32; 4]) — style transitions ---
    /// `background` color crossfade.
    BackgroundColor,
    /// `color` (text) crossfade.
    TextColor,
    /// Per-side border colors. Even though only the top side is
    /// drawn in the current shader, all four are tweenable so the
    /// transition specs the author wrote keep working when we
    /// add per-side rendering later.
    BorderTopColor,
    BorderRightColor,
    BorderBottomColor,
    BorderLeftColor,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct TweenKey {
    pub node: LayoutNode,
    pub property: AnimProperty,
}

impl TweenKey {
    pub fn new(node: LayoutNode, property: AnimProperty) -> Self {
        Self { node, property }
    }
}

#[derive(Copy, Clone, Debug)]
struct Tween {
    from: f32,
    to: f32,
    started: Instant,
    duration: Duration,
    easing: Easing,
}

impl Tween {
    fn sample(&self, now: Instant) -> f32 {
        let elapsed = now.saturating_duration_since(self.started);
        if self.duration.is_zero() || elapsed >= self.duration {
            return self.to;
        }
        let t = elapsed.as_secs_f32() / self.duration.as_secs_f32();
        let eased = apply_easing(t, self.easing);
        self.from + (self.to - self.from) * eased
    }

    fn done(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.started) >= self.duration
    }
}

#[derive(Copy, Clone, Debug)]
struct ColorTween {
    from: [f32; 4],
    to: [f32; 4],
    started: Instant,
    duration: Duration,
    easing: Easing,
}

impl ColorTween {
    fn sample(&self, now: Instant) -> [f32; 4] {
        let elapsed = now.saturating_duration_since(self.started);
        if self.duration.is_zero() || elapsed >= self.duration {
            return self.to;
        }
        let t = elapsed.as_secs_f32() / self.duration.as_secs_f32();
        let eased = apply_easing(t, self.easing);
        lerp_color(self.from, self.to, eased)
    }

    fn done(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.started) >= self.duration
    }
}

pub struct Animator {
    scalars: HashMap<TweenKey, Tween>,
    colors: HashMap<TweenKey, ColorTween>,
}

impl Animator {
    pub fn new() -> Self {
        Self {
            scalars: HashMap::new(),
            colors: HashMap::new(),
        }
    }

    /// Start or restart a scalar tween to `target`.
    ///
    /// - If a tween already exists for `key`, its current sampled
    ///   value becomes the new tween's `from` — preserving visual
    ///   continuity when the target flips mid-animation.
    /// - If no tween exists, `fallback_from` is used as the
    ///   starting value. This is the caller's responsibility
    ///   because the engine doesn't know the widget's "rest"
    ///   value.
    ///
    /// Self-tweens (`from == to`) clear any existing tween and
    /// otherwise no-op.
    pub fn animate(
        &mut self,
        key: TweenKey,
        target: f32,
        fallback_from: f32,
        duration_ms: u32,
        easing: Easing,
        now: Instant,
    ) {
        let from = self
            .scalars
            .get(&key)
            .map(|t| t.sample(now))
            .unwrap_or(fallback_from);
        if (from - target).abs() < f32::EPSILON {
            self.scalars.remove(&key);
            return;
        }
        self.scalars.insert(
            key,
            Tween {
                from,
                to: target,
                started: now,
                duration: Duration::from_millis(duration_ms.max(1) as u64),
                easing,
            },
        );
    }

    /// Color counterpart to [`Animator::animate`]. Same shape and
    /// contract; sRGB linear-RGB-friendly component-wise lerp via
    /// [`lerp_color`] inside the tween.
    pub fn animate_color(
        &mut self,
        key: TweenKey,
        target: [f32; 4],
        fallback_from: [f32; 4],
        duration_ms: u32,
        easing: Easing,
        now: Instant,
    ) {
        let from = self
            .colors
            .get(&key)
            .map(|t| t.sample(now))
            .unwrap_or(fallback_from);
        if color_close(from, target) {
            self.colors.remove(&key);
            return;
        }
        self.colors.insert(
            key,
            ColorTween {
                from,
                to: target,
                started: now,
                duration: Duration::from_millis(duration_ms.max(1) as u64),
                easing,
            },
        );
    }

    /// Sample the current scalar value for `key`, or `fallback`.
    pub fn sample(&self, key: TweenKey, fallback: f32, now: Instant) -> f32 {
        self.scalars
            .get(&key)
            .map(|t| t.sample(now))
            .unwrap_or(fallback)
    }

    /// Sample the current color value for `key`, or `fallback`.
    pub fn sample_color(&self, key: TweenKey, fallback: [f32; 4], now: Instant) -> [f32; 4] {
        self.colors
            .get(&key)
            .map(|t| t.sample(now))
            .unwrap_or(fallback)
    }

    /// Purge completed tweens across both maps. Returns `true` if
    /// anything is still in flight — the caller should
    /// `request_redraw` so the next frame samples the next step.
    pub fn tick(&mut self, now: Instant) -> bool {
        self.scalars.retain(|_, t| !t.done(now));
        self.colors.retain(|_, t| !t.done(now));
        !self.scalars.is_empty() || !self.colors.is_empty()
    }

    /// Drop every tween targeting `node` from both maps.
    pub fn drop_node(&mut self, node: LayoutNode) {
        self.scalars.retain(|k, _| k.node != node);
        self.colors.retain(|k, _| k.node != node);
    }
}

fn color_close(a: [f32; 4], b: [f32; 4]) -> bool {
    (a[0] - b[0]).abs() < f32::EPSILON
        && (a[1] - b[1]).abs() < f32::EPSILON
        && (a[2] - b[2]).abs() < f32::EPSILON
        && (a[3] - b[3]).abs() < f32::EPSILON
}

impl Default for Animator {
    fn default() -> Self {
        Self::new()
    }
}

/// Apply an [`Easing`] curve to a linear `0..=1` time parameter.
/// CubicBezier uses an approximation suitable for UI work —
/// equivalent to a couple of Newton-Raphson iterations on the
/// inverse curve. Easings are taken from the framework's
/// transition vocabulary so backend animation feels the same as
/// the style-driven transition system.
fn apply_easing(t: f32, easing: Easing) -> f32 {
    let t = t.clamp(0.0, 1.0);
    match easing {
        Easing::Linear => t,
        Easing::Ease => cubic_bezier_y(t, 0.25, 0.1, 0.25, 1.0),
        Easing::EaseIn => cubic_bezier_y(t, 0.42, 0.0, 1.0, 1.0),
        Easing::EaseOut => cubic_bezier_y(t, 0.0, 0.0, 0.58, 1.0),
        Easing::EaseInOut => cubic_bezier_y(t, 0.42, 0.0, 0.58, 1.0),
        Easing::CubicBezier(x1, y1, x2, y2) => cubic_bezier_y(t, x1, y1, x2, y2),
    }
}

/// Approximate `y` on a cubic Bézier curve `(0,0) → (x1,y1) →
/// (x2,y2) → (1,1)` at horizontal position `x = t`.
///
/// Standard UI-grade implementation: a few Newton-Raphson
/// iterations to solve for the curve parameter that produces `x`,
/// then evaluate `y` at that parameter. Cheap (< 1 µs typical)
/// and accurate to well under a pixel for any reasonable
/// duration.
fn cubic_bezier_y(x: f32, x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    // Bezier basis coefficients along x.
    let ax = 3.0 * x1 - 3.0 * x2 + 1.0;
    let bx = -6.0 * x1 + 3.0 * x2;
    let cx = 3.0 * x1;
    let curve_x = |u: f32| ((ax * u + bx) * u + cx) * u;
    let curve_dx = |u: f32| (3.0 * ax * u + 2.0 * bx) * u + cx;

    // Bezier basis coefficients along y.
    let ay = 3.0 * y1 - 3.0 * y2 + 1.0;
    let by = -6.0 * y1 + 3.0 * y2;
    let cy = 3.0 * y1;
    let curve_y = |u: f32| ((ay * u + by) * u + cy) * u;

    // Solve curve_x(u) = x for u, then return curve_y(u).
    let mut u = x;
    for _ in 0..6 {
        let cx_u = curve_x(u);
        let dx_u = curve_dx(u);
        if dx_u.abs() < 1e-6 {
            break;
        }
        u -= (cx_u - x) / dx_u;
        u = u.clamp(0.0, 1.0);
    }
    curve_y(u)
}

/// Component-wise linear interpolation for color blending.
/// Inputs are sRGB-space `[r, g, b, a]`; we lerp in sRGB which
/// matches CSS / UIKit's `tintColor` transitions (strictly less
/// "physically correct" than linear-space lerp, but visually
/// indistinguishable for short transitions between similar
/// colors).
pub fn lerp_color(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ]
}
