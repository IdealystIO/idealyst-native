//! Robot-feature metadata extraction. The build walker calls
//! [`robot_extract_meta`] *before* destructuring a `Element`, so the
//! returned `RobotMeta` can be pre-registered with the robot registry
//! while the build proceeds.
//!
//! Entire module is gated on the `robot` Cargo feature.

#![cfg(feature = "robot")]

use std::rc::Rc;

use crate::element::Element;
use crate::primitives;
use crate::sources::TextSource;

pub(super) struct RobotMeta {
    pub(super) kind: crate::robot::ElementKind,
    pub(super) test_id: Option<&'static str>,
    pub(super) label: Option<String>,
    /// Lazy recompute for reactive labels — `Some` only for
    /// `TextSource::Bound` / `JsBinding`. See `RegistryEntry::label_fn`.
    pub(super) label_fn: Option<Rc<dyn Fn() -> Option<String>>>,
    pub(super) actions: crate::robot::ElementActions,
}

/// Compute the mount-time label for any `TextSource`. Used as the
/// cached `label` (a snapshot for static sources, a starting value for
/// reactive ones that `label_recompute` then keeps fresh).
fn label_now(source: &TextSource) -> Option<String> {
    match source {
        TextSource::Static(s) => Some(s.clone()),
        TextSource::Bound(d) => Some((d.compute)()),
        TextSource::JsBinding(spec) => Some((spec.compute_fallback)()),
    }
}

/// Build a recompute closure for reactive text sources so the robot
/// registry reports the *live* label, not the value frozen at mount.
/// Returns `None` for static text (the cached `label` is authoritative).
/// The read is untracked — querying the robot must never subscribe the
/// caller's scope to the underlying signals.
fn label_recompute(source: &TextSource) -> Option<Rc<dyn Fn() -> Option<String>>> {
    match source {
        TextSource::Static(_) => None,
        TextSource::Bound(d) => {
            let compute = d.compute.clone();
            Some(Rc::new(move || Some(crate::reactive::untrack(|| (compute)()))))
        }
        TextSource::JsBinding(spec) => {
            let compute = spec.compute_fallback.clone();
            Some(Rc::new(move || Some(crate::reactive::untrack(|| (compute)()))))
        }
    }
}

/// Extract robot-relevant metadata from a primitive *before* the move
/// match destructures it. Only interactive primitives (buttons,
/// inputs, etc.) produce a `Some`; structural primitives that aren't
/// useful to query (When, Switch, Repeat) produce `None`.
pub(super) fn robot_extract_meta(node: &Element) -> Option<RobotMeta> {
    use crate::robot::{ElementActions, ElementKind};

    match node {
        Element::View { test_id, .. } => Some(RobotMeta {
            kind: ElementKind::View,
            test_id: *test_id,
            label: None,
            label_fn: None,
            actions: ElementActions::empty(),
        }),
        Element::Text { source, test_id, .. } => Some(RobotMeta {
            kind: ElementKind::Text,
            test_id: *test_id,
            label: label_now(source),
            label_fn: label_recompute(source),
            actions: ElementActions::empty(),
        }),
        Element::Button { label, on_click, test_id, .. } => {
            // `on_click` is an `Action` (not a bare `Rc<dyn Fn()>`)
            // since the generator migration. The robot's
            // `ElementActions.click` still wants the underlying
            // callable, so pull `Action::fire` (which is the
            // `Rc<dyn Fn()>` runtime backends invoke on tap).
            let click = on_click.fire.clone();
            Some(RobotMeta {
                kind: ElementKind::Button,
                test_id: *test_id,
                label: label_now(label),
                label_fn: label_recompute(label),
                actions: ElementActions {
                    click: Some(click),
                    ..ElementActions::empty()
                },
            })
        }
        Element::Pressable { on_click, test_id, .. } => {
            let click = on_click.clone();
            Some(RobotMeta {
                kind: ElementKind::Pressable,
                test_id: *test_id,
                label: None,
                label_fn: None,
                actions: ElementActions {
                    click: Some(click),
                    ..ElementActions::empty()
                },
            })
        }
        Element::Image { test_id, .. } => Some(RobotMeta {
            kind: ElementKind::Image,
            test_id: *test_id,
            label: None,
            label_fn: None,
            actions: ElementActions::empty(),
        }),
        Element::TextInput { on_change, test_id, .. } => {
            let set_text = on_change.clone();
            Some(RobotMeta {
                kind: ElementKind::TextInput,
                test_id: *test_id,
                label: None,
                label_fn: None,
                actions: ElementActions {
                    set_text: Some(set_text),
                    ..ElementActions::empty()
                },
            })
        }
        Element::TextArea { on_change, test_id, .. } => {
            // Reuse `ElementKind::TextInput` — the robot
            // surface doesn't distinguish single- vs.
            // multi-line; the `set_text` action covers both.
            // Authors who care can branch on the wrapping
            // element's test_id.
            let set_text = on_change.clone();
            Some(RobotMeta {
                kind: ElementKind::TextInput,
                test_id: *test_id,
                label: None,
                label_fn: None,
                actions: ElementActions {
                    set_text: Some(set_text),
                    ..ElementActions::empty()
                },
            })
        }
        Element::Toggle { on_change, test_id, .. } => {
            let set_toggle = on_change.clone();
            Some(RobotMeta {
                kind: ElementKind::Toggle,
                test_id: *test_id,
                label: None,
                label_fn: None,
                actions: ElementActions {
                    set_toggle: Some(set_toggle),
                    ..ElementActions::empty()
                },
            })
        }
        Element::Slider { on_change, test_id, .. } => {
            let set_slider = on_change.clone();
            Some(RobotMeta {
                kind: ElementKind::Slider,
                test_id: *test_id,
                label: None,
                label_fn: None,
                actions: ElementActions {
                    set_slider: Some(set_slider),
                    ..ElementActions::empty()
                },
            })
        }
        Element::Link { route, url, make_params, kind, target, .. } => {
            // Build the same on_activate the backend wires onto the
            // native tap target so the robot's `click` triggers the
            // navigator just like a real tap would.
            let click = primitives::link::make_on_activate(
                target.clone(),
                route,
                url.clone(),
                *kind,
                make_params.clone(),
            );
            Some(RobotMeta {
                kind: ElementKind::Link,
                test_id: None,
                label: None,
                label_fn: None,
                actions: ElementActions {
                    click: Some(click),
                    ..ElementActions::empty()
                },
            })
        }
        Element::Navigator { .. } => Some(RobotMeta {
            kind: ElementKind::Navigator,
            test_id: None,
            label: None,
            label_fn: None,
            actions: ElementActions::empty(),
        }),
        // Structural/reactive primitives don't get registered.
        _ => None,
    }
}
