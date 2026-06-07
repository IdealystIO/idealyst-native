//! Icon primitive.
//!
//! Renders vector icon data as an inline SVG on web, `CAShapeLayer` on
//! iOS, `VectorDrawable` on Android. Icon data is `&'static` so only
//! icons actually referenced by application code end up in the binary —
//! the linker (with LTO) drops unreferenced `IconData` constants.
//!
//! ## Stroke animation
//!
//! Icons support stroke-draw animations: the path progressively draws
//! itself from 0% to 100% (or any range). This works natively on all
//! platforms:
//! - Web: `stroke-dasharray` + `stroke-dashoffset` with CSS transition
//! - iOS: `CAShapeLayer.strokeEnd` with `CABasicAnimation`
//! - Android: `ObjectAnimator` on `trimPathEnd`
//!
//! Two modes:
//! - **Reactive stroke progress** — `icon(X).stroke(|| signal.get())`
//!   gives programmatic control over how much of the path is drawn.
//! - **Animate-in on mount** — `icon(X).draw_in(500, Easing::EaseOut)`
//!   plays the draw-on effect when the icon first mounts.
//!
//! Platforms that don't support stroke animation ignore it — the icon
//! still renders fully drawn.

use crate::style::Easing;
use crate::{Bound, Element, Ref, RefFill};
use std::any::Any;
use std::rc::Rc;

// ---------------------------------------------------------------------------
// IconData — the static, const-constructible icon definition
// ---------------------------------------------------------------------------

/// Fill rule for SVG path rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillRule {
    /// Non-zero winding rule (SVG default).
    NonZero,
    /// Even-odd rule.
    EvenOdd,
}

/// A single icon's vector data. Designed to be `const`-constructible
/// so icon packs are zero-runtime-cost static data living in `.rodata`.
///
/// # Example
///
/// ```ignore
/// pub const SEARCH: IconData = IconData {
///     view_box: (24, 24),
///     paths: &["M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z"],
///     fill_rule: FillRule::NonZero,
///     filled: false,
/// };
/// ```
#[derive(Debug, Clone, Copy)]
pub struct IconData {
    /// viewBox dimensions `(width, height)`.
    pub view_box: (u16, u16),
    /// One or more SVG path `d` attribute strings. Multiple paths
    /// support multi-part icons (e.g. outlined + filled regions).
    pub paths: &'static [&'static str],
    /// Default fill rule applied to all paths.
    pub fill_rule: FillRule,
    /// When `false` (the default, matching Lucide's outlined style), the
    /// icon's paths are stroked with the icon color and the interior is
    /// left transparent. When `true`, the paths are *filled* with the
    /// icon color (using `fill_rule`) and the stroke is disabled — for
    /// solid/silhouette glyphs (brand marks, sparkles, solid play/pause).
    pub filled: bool,
}

// ---------------------------------------------------------------------------
// Stroke animation config
// ---------------------------------------------------------------------------

/// Configuration for icon stroke animation. Constructed via builder:
///
/// ```ignore
/// StrokeAnimation::new(600, Easing::EaseOut)          // 0→1, once
/// StrokeAnimation::new(800, Easing::EaseInOut)
///     .range(0.2, 0.8)                                // custom range
///     .looping()                                      // infinite
///     .reverse()                                      // autoreverse
/// ```
#[derive(Debug, Clone, Copy)]
pub struct StrokeAnimation {
    /// Duration in milliseconds.
    pub duration_ms: u32,
    /// Easing curve.
    pub easing: Easing,
    /// Starting progress (0.0 = nothing drawn). Default: 0.0.
    pub from: f32,
    /// Ending progress (1.0 = fully drawn). Default: 1.0.
    pub to: f32,
    /// When true, the animation loops indefinitely.
    pub infinite: bool,
    /// When true (and looping), the animation autoreverses
    /// (from→to→from→to) instead of snapping back (from→to, from→to).
    pub autoreverses: bool,
}

impl StrokeAnimation {
    /// Create a stroke animation with duration and easing.
    /// Defaults to drawing from 0→1, single pass, no reverse.
    pub fn new(duration_ms: u32, easing: Easing) -> Self {
        Self {
            duration_ms,
            easing,
            from: 0.0,
            to: 1.0,
            infinite: false,
            autoreverses: false,
        }
    }

    /// Set the from/to range.
    pub fn range(mut self, from: f32, to: f32) -> Self {
        self.from = from;
        self.to = to;
        self
    }

    /// Make the animation loop infinitely.
    pub fn looping(mut self) -> Self {
        self.infinite = true;
        self
    }

    /// Autoreverse when looping (from→to→from instead of from→to→from→to snap).
    /// Implies `.looping()`.
    pub fn reverse(mut self) -> Self {
        self.infinite = true;
        self.autoreverses = true;
        self
    }
}

// ---------------------------------------------------------------------------
// IconHandle + IconOps
// ---------------------------------------------------------------------------

/// Handle exposed to a parent via `Ref<IconHandle>`.
#[derive(Clone)]
pub struct IconHandle {
    node: Rc<dyn Any>,
    ops: &'static dyn IconOps,
}

impl IconHandle {
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn IconOps) -> Self {
        Self { node, ops }
    }

    /// Imperatively animate the stroke from `from` to `to` over
    /// `duration_ms` with the given easing. Platforms that don't
    /// support stroke animation no-op.
    pub fn animate_stroke(&self, from: f32, to: f32, duration_ms: u32, easing: Easing) {
        self.ops.animate_stroke(&*self.node, from, to, duration_ms, easing);
    }

    /// Set stroke progress immediately (no animation). 0.0 = hidden,
    /// 1.0 = fully drawn.
    pub fn set_stroke_progress(&self, progress: f32) {
        self.ops.set_stroke_progress(&*self.node, progress);
    }

    /// Replay the icon's draw-in animation from the beginning.
    pub fn replay(&self, from: f32, to: f32, duration_ms: u32, easing: Easing) {
        self.ops.set_stroke_progress(&*self.node, from);
        self.ops.animate_stroke(&*self.node, from, to, duration_ms, easing);
    }

    /// Play the stroke animation in reverse (1→0 by default).
    /// The icon "erases" itself.
    pub fn reverse(&self, duration_ms: u32, easing: Easing) {
        self.ops.animate_stroke(&*self.node, 1.0, 0.0, duration_ms, easing);
    }
}

pub trait IconOps {
    /// Animate stroke from→to over duration with easing.
    fn animate_stroke(
        &self,
        _node: &dyn Any,
        _from: f32,
        _to: f32,
        _duration_ms: u32,
        _easing: Easing,
    ) {
    }

    /// Set stroke progress immediately (no animation).
    fn set_stroke_progress(&self, _node: &dyn Any, _progress: f32) {}
}

// ---------------------------------------------------------------------------
// Constructor + builder
// ---------------------------------------------------------------------------

/// Construct an `Icon` primitive from icon data.
///
/// ```ignore
/// use icons_lucide::SEARCH;
///
/// // Basic usage
/// icon(SEARCH)
///
/// // With draw-in animation on mount
/// icon(SEARCH).draw_in(500, Easing::EaseOut)
///
/// // With reactive stroke progress (e.g. tied to scroll)
/// icon(SEARCH).stroke(|| scroll_progress.get())
/// ```
pub fn icon(data: IconData) -> Bound<IconHandle> {
    Bound::new(Element::Icon {
        data,
        color: None,
        stroke: None,
        draw_in: None,
        style: None,
        ref_fill: None,
        accessibility: crate::accessibility::AccessibilityProps::default(),
        #[cfg(feature = "robot")]
        test_id: None,
    })
}

impl Bound<IconHandle> {
    /// Set a reactive color for the icon. When `None` (the default),
    /// the icon inherits `currentColor` on web or the nearest text
    /// color on native platforms.
    pub fn color<F: Fn() -> crate::style::Color + 'static>(mut self, f: F) -> Self {
        if let Element::Icon { color, .. } = &mut self.primitive {
            *color = Some(Box::new(f));
        }
        self
    }

    /// Reactive stroke progress (0.0 to 1.0). Controls how much of the
    /// icon's path is visibly drawn. Useful for binding to scroll
    /// position, loading progress, or gesture state.
    ///
    /// When set, the icon mounts at the initial value of the closure
    /// and updates reactively as signals change.
    pub fn stroke<F: Fn() -> f32 + 'static>(mut self, f: F) -> Self {
        if let Element::Icon { stroke, .. } = &mut self.primitive {
            *stroke = Some(Box::new(f));
        }
        self
    }

    /// Configure a stroke animation that plays on mount.
    ///
    /// ```ignore
    /// icon(SEARCH).animate(StrokeAnimation::new(600, Easing::EaseOut))
    /// icon(MENU).animate(StrokeAnimation::new(800, Easing::EaseInOut).looping())
    /// icon(X).animate(StrokeAnimation::new(1000, Easing::Linear).range(0.2, 0.8).reverse())
    /// ```
    ///
    /// For ongoing programmatic control, use `.stroke()` with a
    /// reactive signal, or `.bind()` and call handle methods.
    pub fn animate(mut self, anim: StrokeAnimation) -> Self {
        if let Element::Icon { draw_in, .. } = &mut self.primitive {
            *draw_in = Some(anim);
        }
        self
    }

    /// Shorthand for `.animate(StrokeAnimation::new(duration_ms, easing))`.
    pub fn draw_in(self, duration_ms: u32, easing: Easing) -> Self {
        self.animate(StrokeAnimation::new(duration_ms, easing))
    }

    /// Bind to a `Ref<IconHandle>` so the parent can call
    /// `animate_stroke()`, `set_stroke_progress()`, or `replay()`
    /// imperatively.
    pub fn bind(mut self, r: Ref<IconHandle>) -> Self {
        if let Element::Icon { ref_fill, .. } = &mut self.primitive {
            *ref_fill = Some(RefFill::Icon(Box::new(move |h| r.fill(h))));
        }
        self
    }
}
