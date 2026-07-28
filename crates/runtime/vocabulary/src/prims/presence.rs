//! Animated-lifecycle payload: `presence` — declarative mount/unmount
//! animation (`walker/presence.rs`'s payload shape, re-expressed for the
//! scene's Dyn-driver + retire-hook contract).

use std::rc::Rc;

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives::presence::{PresenceAnim, PresenceHandle};
use runtime_scene::Element;

/// The `presence` primitive. `child` is called once per real mount
/// (each `false → true` flip of `present`); `enter`/`exit` carry the
/// animation halves (`None` = mount/unmount without animation, exactly
/// like the old core).
pub struct PresencePrim {
    /// Deferred child constructor — built fresh on every mount, inside
    /// the subtree's own scope (the old walker's `untrack`ed child
    /// build, now the guarded Dyn build).
    pub child: Box<dyn Fn() -> Element>,
    /// The presence predicate. `Rc` because the handler installs it in
    /// two tracked closures (the structural hole's guard + the
    /// enter-animation effect) that must share one dependency source.
    pub present: Rc<dyn Fn() -> bool>,
    pub enter: Option<PresenceAnim>,
    pub exit: Option<PresenceAnim>,
    pub a11y: AccessibilityProps,
    pub ref_fill: Option<Box<dyn FnOnce(PresenceHandle)>>,
}
