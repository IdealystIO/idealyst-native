//! Toggle primitive (controlled switch / checkbox).
//!
//! Backed by `<input type="checkbox">` (with `role="switch"`) on
//! web, `UISwitch` on iOS, `Switch` on Android. Controlled: parent
//! owns a `Signal<bool>` that the framework reads to set the native
//! widget's state; native toggle events fire `on_change`.
//!
//! Same controlled rationale as `TextInput`: single source of truth
//! lives in the parent's signal.

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


