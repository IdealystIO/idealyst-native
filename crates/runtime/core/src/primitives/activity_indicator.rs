//! Loading spinner / activity indicator.
//!
//! Passive widget — no state, no methods. Authors typically render
//! it conditionally (`if loading.get() { ... }`) inside a `when()`.
//! Backends:
//!   - Web: a `<span>` with a CSS keyframe rotation. The rule is
//!     injected into the framework's stylesheet once on first use.
//!   - iOS: `UIActivityIndicatorView` (`startAnimating()` on mount).
//!   - Android: indeterminate `ProgressBar`.

use crate::{Bound, Color, Element, Ref, RefFill};
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
    Bound::new(Element::ActivityIndicator {
        size: ActivityIndicatorSize::default(),
        size_fn: None,
        color: None,
        style: None,
        ref_fill: None,
        accessibility: crate::accessibility::AccessibilityProps::default(),
        #[cfg(feature = "robot")]
        test_id: None,
    })
}

impl Bound<ActivityIndicatorHandle> {
    pub fn size(mut self, s: ActivityIndicatorSize) -> Self {
        if let Element::ActivityIndicator { size, .. } = &mut self.primitive {
            *size = s;
        }
        self
    }

    /// Set a reactive `size`. When the closure's signals change, the
    /// spinner resizes in place (no node rebuild) via
    /// `Backend::update_activity_indicator_size`. The indicator mounts at
    /// the closure's initial value. A fixed size uses [`size`](Self::size)
    /// and skips this. Mirrors `Icon::data` (closure-source).
    ///
    /// Note: `ActivityIndicatorSize` is a discrete `Small`/`Large` enum.
    /// Web re-applies the CSS diameter in place; native spinners
    /// (`UIActivityIndicatorView`, `ProgressBar`) fix their style at
    /// construction and inherit the backend no-op (see
    /// `Backend::update_activity_indicator_size`).
    pub fn size_reactive<F: Fn() -> ActivityIndicatorSize + 'static>(mut self, f: F) -> Self {
        if let Element::ActivityIndicator { size, size_fn, .. } = &mut self.primitive {
            *size = f();
            *size_fn = Some(Box::new(f));
        }
        self
    }

    pub fn color(mut self, c: Color) -> Self {
        if let Element::ActivityIndicator { color, .. } = &mut self.primitive {
            *color = Some(c);
        }
        self
    }

    pub fn bind(mut self, r: Ref<ActivityIndicatorHandle>) -> Self {
        if let Element::ActivityIndicator { ref_fill, .. } = &mut self.primitive {
            *ref_fill = Some(RefFill::ActivityIndicator(Box::new(move |h| r.fill(h))));
        }
        self
    }
}
