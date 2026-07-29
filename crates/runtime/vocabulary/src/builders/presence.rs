//! Animated-lifecycle builder: `presence()`.

use std::rc::Rc;

use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::primitives::presence::{PresenceAnim, PresenceHandle};
use runtime_scene::{item, Element};
use runtime_world::{IntoValue, Value};

use crate::prims::{PresencePrim, PrimCell};

use super::IntoSceneElement;

/// Start a presence-controlled subtree. `child` is called once per real
/// mount — when `present` first flips true, and again only after a
/// complete unmount (exit finished, subtree dropped) precedes a new
/// flip-on.
///
/// Default: always present (the child mounts immediately and never
/// exits) — same as the old core; real call sites chain `.present(…)`.
pub fn presence<E: IntoSceneElement>(child: impl Fn() -> E + 'static) -> PresenceBuilder {
    PresenceBuilder {
        prim: PresencePrim {
            test_id: None,
            child: Box::new(move || child().into_scene_element()),
            present: Rc::new(|| true),
            enter: None,
            exit: None,
            a11y: AccessibilityProps::default(),
            ref_fill: None,
        },
    }
}

pub struct PresenceBuilder {
    prim: PresencePrim,
}

impl PresenceBuilder {
    /// The presence predicate — a bool literal (`Const`, fixed) or a
    /// signal/closure (`Dyn`, re-evaluated; flips drive mount/unmount).
    pub fn present(mut self, present: impl IntoValue<bool>) -> Self {
        self.prim.present = match present.into_value() {
            Value::Const(b) => Rc::new(move || b),
            Value::Dyn(f) => Rc::from(f),
        };
        self
    }

    pub fn enter(mut self, anim: PresenceAnim) -> Self {
        self.prim.enter = Some(anim);
        self
    }

    pub fn exit(mut self, anim: PresenceAnim) -> Self {
        self.prim.exit = Some(anim);
        self
    }

    pub fn a11y(mut self, a11y: AccessibilityProps) -> Self {
        self.prim.a11y = a11y;
        self
    }

    /// Receive the imperative-ref handle at mount (P2 form of `.bind`).
    /// Robot/automation anchor (`test_id = …`): the mount handler
    /// registers this node under `id` in the vocabulary robot registry
    /// (`robot` feature; the slot is inert otherwise).
    pub fn test_id(mut self, id: &'static str) -> Self {
        self.prim.test_id = Some(id);
        self
    }

    pub fn on_handle(mut self, fill: impl FnOnce(PresenceHandle) + 'static) -> Self {
        self.prim.ref_fill = Some(Box::new(fill));
        self
    }

    pub fn build(self) -> Element {
        item(PrimCell::new(self.prim), Vec::new())
    }
}
