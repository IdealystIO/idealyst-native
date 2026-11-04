//! Loading spinner / activity indicator.
//!
//! Passive widget — no state, no methods. Authors typically render
//! it conditionally (`if loading.get() { ... }`) inside a `when()`.
//! A reactive `size` resizes it in place on every backend that has a
//! spinner, and the layout follows.
//! Backends:
//!   - Web: a `<span>` with a CSS keyframe rotation. The rule is
//!     injected into the framework's stylesheet once on first use.
//!   - iOS: `UIActivityIndicatorView` (`startAnimating()` on mount).
//!   - Android: indeterminate `ProgressBar`, at iOS's diameters.
//!   - Windows: a marquee progress bar (comctl32 has no spinner).
//!   - CPU: none; the primitive is a placeholder there.

use std::any::Any;
use std::rc::Rc;

/// Two sizes matching RN's API. Maps to native sizes on iOS and macOS, to
/// iOS's diameters on Android (20dp for Small, 37dp for Large), and to fixed
/// px diameters on web (16px for Small, 36px for Large).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "remote-serde", derive(serde::Serialize, serde::Deserialize))]
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


