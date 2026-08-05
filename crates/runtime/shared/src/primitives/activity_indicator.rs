//! Loading spinner / activity indicator.
//!
//! Passive widget — no state, no methods. Authors typically render
//! it conditionally (`if loading.get() { ... }`) inside a `when()`.
//! Backends:
//!   - Web: a `<span>` with a CSS keyframe rotation. The rule is
//!     injected into the framework's stylesheet once on first use.
//!   - iOS: `UIActivityIndicatorView` (`startAnimating()` on mount).
//!   - Android: indeterminate `ProgressBar`.

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


