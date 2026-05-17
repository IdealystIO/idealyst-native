//! Loading spinner / activity indicator.
//!
//! Passive widget — no state, no methods. Authors typically render
//! it conditionally (`if loading.get() { ... }`) inside a `when()`.
//! Backends:
//!   - Web: a `<span>` with a CSS keyframe rotation. The rule is
//!     injected into the framework's stylesheet once on first use.
//!   - iOS: `UIActivityIndicatorView` (`startAnimating()` on mount).
//!   - Android: indeterminate `ProgressBar`.

use crate::{Bound, Color, Primitive, Ref, RefFill};
use std::any::Any;
use std::rc::Rc;

/// Two sizes matching RN's API. Maps to native sizes per-platform
/// and to fixed px diameters on web (16px for Small, 36px for Large).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityIndicatorSize {
    Small,
    Large,
}

impl Default for ActivityIndicatorSize {
    fn default() -> Self {
        ActivityIndicatorSize::Small
    }
}

#[derive(Clone)]
pub struct ActivityIndicatorHandle {
    #[allow(dead_code)]
    node: Rc<dyn Any>,
    #[allow(dead_code)]
    ops: &'static dyn ActivityIndicatorOps,
}

impl ActivityIndicatorHandle {
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn ActivityIndicatorOps) -> Self {
        Self { node, ops }
    }
}

pub trait ActivityIndicatorOps {
    // Reserved.
}

/// Construct an indicator with default size (`Small`) and no color
/// override (uses native default tint or theme on web).
pub fn activity_indicator() -> Bound<ActivityIndicatorHandle> {
    Bound::new(Primitive::ActivityIndicator {
        size: ActivityIndicatorSize::default(),
        color: None,
        style: None,
        ref_fill: None,
    })
}

impl Bound<ActivityIndicatorHandle> {
    pub fn size(mut self, s: ActivityIndicatorSize) -> Self {
        if let Primitive::ActivityIndicator { size, .. } = &mut self.primitive {
            *size = s;
        }
        self
    }

    pub fn color(mut self, c: Color) -> Self {
        if let Primitive::ActivityIndicator { color, .. } = &mut self.primitive {
            *color = Some(c);
        }
        self
    }

    pub fn bind(mut self, r: Ref<ActivityIndicatorHandle>) -> Self {
        if let Primitive::ActivityIndicator { ref_fill, .. } = &mut self.primitive {
            *ref_fill = Some(RefFill::ActivityIndicator(Box::new(move |h| r.fill(h))));
        }
        self
    }
}
