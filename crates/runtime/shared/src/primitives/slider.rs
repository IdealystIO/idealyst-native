//! Slider primitive (controlled, single-value f32).
//!
//! Backed by `<input type="range">` on web, `UISlider` on iOS, and
//! Material `Slider` (or `SeekBar`) on Android. Controlled in the
//! same shape as `TextInput`/`Toggle` — parent owns the value
//! signal; the framework snaps the incoming on_change to `step` (if
//! set) before passing to the user's callback, so all three
//! platforms behave identically regardless of native step support.

use std::any::Any;
use std::rc::Rc;

#[derive(Clone)]
pub struct SliderHandle {
    #[allow(dead_code)]
    node: Rc<dyn Any>,
    #[allow(dead_code)]
    ops: &'static dyn SliderOps,
}

impl SliderHandle {
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn SliderOps) -> Self {
        Self { node, ops }
    }
}

pub trait SliderOps {
    // No methods yet — slider value is fully reactive via the
    // controlled signal.
}


