//! Toggle primitive (controlled switch / checkbox).
//!
//! Backed by `<input type="checkbox">` (with `role="switch"`) on
//! web, `UISwitch` on iOS, `Switch` on Android. Controlled: parent
//! owns a `Signal<bool>` that the framework reads to set the native
//! widget's state; native toggle events fire `on_change`.
//!
//! Same controlled rationale as `TextInput`: single source of truth
//! lives in the parent's signal.

use crate::{Bound, Element, Ref, RefFill, Signal};
use std::rc::Rc;

// The data/handle/Ops types of this primitive moved to `runtime-shared`
// (the walker-free half); this file keeps the Element/Bound builder
// surface (and its tests). The wildcard re-export preserves every old
// path.
pub use runtime_shared::primitives::toggle::*;

/// Construct a controlled toggle. `value` is the source of truth;
/// `on_change` is called with the new value on every native flip.
#[cfg(feature = "prim-toggle")]
pub fn toggle<F: Fn(bool) + 'static>(
    value: Signal<bool>,
    on_change: F,
) -> Bound<ToggleHandle> {
    Bound::new(Element::Toggle {
        value,
        // Born batched — see `reactive::cycle`.
        on_change: Rc::new(move |v: bool| crate::cycle(|| on_change(v))),
        style: None,
        ref_fill: None,
        accessibility: crate::accessibility::AccessibilityProps::default(),
        #[cfg(feature = "robot")]
        test_id: None,
    })
}

impl Bound<ToggleHandle> {
    pub fn bind(mut self, r: Ref<ToggleHandle>) -> Self {
        if let Element::Toggle { ref_fill, .. } = &mut self.primitive {
            *ref_fill = Some(RefFill::Toggle(Box::new(move |h| r.fill(h))));
        }
        self
    }
}
