//! Slider primitive (controlled, single-value f32).
//!
//! Backed by `<input type="range">` on web, `UISlider` on iOS, and
//! Material `Slider` (or `SeekBar`) on Android. Controlled in the
//! same shape as `TextInput`/`Toggle` — parent owns the value
//! signal; the framework snaps the incoming on_change to `step` (if
//! set) before passing to the user's callback, so all three
//! platforms behave identically regardless of native step support.

use crate::{Bound, Element, Ref, RefFill, Signal};
use std::rc::Rc;

// The data/handle/Ops types of this primitive moved to `runtime-shared`
// (the walker-free half); this file keeps the Element/Bound builder
// surface (and its tests). The wildcard re-export preserves every old
// path.
pub use runtime_shared::primitives::slider::*;

/// Construct a controlled slider with default range 0.0..=1.0 and
/// no step (continuous). Use `.range(min, max)` and `.step(step)` to
/// customize.
#[cfg(feature = "prim-slider")]
pub fn slider<F: Fn(f32) + 'static>(
    value: Signal<f32>,
    on_change: F,
) -> Bound<SliderHandle> {
    Bound::new(Element::Slider {
        value,
        // Born batched — see `reactive::cycle`.
        on_change: Rc::new(move |v: f32| crate::cycle(|| on_change(v))),
        min: 0.0,
        max: 1.0,
        step: None,
        style: None,
        ref_fill: None,
        accessibility: crate::accessibility::AccessibilityProps::default(),
        #[cfg(feature = "robot")]
        test_id: None,
    })
}

impl Bound<SliderHandle> {
    /// Set the slider's min and max. Both inclusive.
    pub fn range(mut self, min: f32, max: f32) -> Self {
        if let Element::Slider { min: a, max: b, .. } = &mut self.primitive {
            *a = min;
            *b = max;
        }
        self
    }

    /// Set the step increment. If `None`, the slider is continuous.
    /// When set, the framework snaps incoming values to the nearest
    /// step in the on_change pipeline (relative to `min`) so all
    /// backends produce identical values.
    pub fn step(mut self, step: f32) -> Self {
        if let Element::Slider { step: slot, .. } = &mut self.primitive {
            *slot = Some(step);
        }
        self
    }

    pub fn bind(mut self, r: Ref<SliderHandle>) -> Self {
        if let Element::Slider { ref_fill, .. } = &mut self.primitive {
            *ref_fill = Some(RefFill::Slider(Box::new(move |h| r.fill(h))));
        }
        self
    }
}
