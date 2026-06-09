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
use std::any::Any;
use std::rc::Rc;

#[derive(Clone)]
pub struct ToggleHandle {
    #[allow(dead_code)]
    node: Rc<dyn Any>,
    #[allow(dead_code)]
    ops: &'static dyn ToggleOps,
}

impl ToggleHandle {
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn ToggleOps) -> Self {
        Self { node, ops }
    }
}

pub trait ToggleOps {
    // No methods yet. Toggle state is fully reactive via the
    // controlled signal; nothing imperative is needed.
}

/// Construct a controlled toggle. `value` is the source of truth;
/// `on_change` is called with the new value on every native flip.
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
