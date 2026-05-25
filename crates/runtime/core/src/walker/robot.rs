//! Robot-feature metadata extraction. The build walker calls
//! [`robot_extract_meta`] *before* destructuring a `Primitive`, so the
//! returned `RobotMeta` can be pre-registered with the robot registry
//! while the build proceeds.
//!
//! Entire module is gated on the `robot` Cargo feature.

#![cfg(feature = "robot")]

use crate::primitive::Primitive;
use crate::primitives;
use crate::sources::TextSource;

pub(super) struct RobotMeta {
    pub(super) kind: crate::robot::ElementKind,
    pub(super) test_id: Option<&'static str>,
    pub(super) label: Option<String>,
    pub(super) actions: crate::robot::ElementActions,
}

/// Extract robot-relevant metadata from a primitive *before* the move
/// match destructures it. Only interactive primitives (buttons,
/// inputs, etc.) produce a `Some`; structural primitives that aren't
/// useful to query (When, Switch, Repeat) produce `None`.
pub(super) fn robot_extract_meta(node: &Primitive) -> Option<RobotMeta> {
    use crate::robot::{ElementActions, ElementKind};

    match node {
        Primitive::View { test_id, .. } => Some(RobotMeta {
            kind: ElementKind::View,
            test_id: *test_id,
            label: None,
            actions: ElementActions::empty(),
        }),
        Primitive::Text { source, test_id, .. } => {
            let label = match source {
                TextSource::Static(s) => Some(s.clone()),
                TextSource::Bound(d) => Some((d.compute)()),
                TextSource::JsBinding(spec) => Some((spec.compute_fallback)()),
            };
            Some(RobotMeta {
                kind: ElementKind::Text,
                test_id: *test_id,
                label,
                actions: ElementActions::empty(),
            })
        }
        Primitive::Button { label, on_click, test_id, .. } => {
            let label_text = match label {
                TextSource::Static(s) => Some(s.clone()),
                TextSource::Bound(d) => Some((d.compute)()),
                TextSource::JsBinding(spec) => Some((spec.compute_fallback)()),
            };
            // `on_click` is an `Action` (not a bare `Rc<dyn Fn()>`)
            // since the generator migration. The robot's
            // `ElementActions.click` still wants the underlying
            // callable, so pull `Action::fire` (which is the
            // `Rc<dyn Fn()>` runtime backends invoke on tap).
            let click = on_click.fire.clone();
            Some(RobotMeta {
                kind: ElementKind::Button,
                test_id: *test_id,
                label: label_text,
                actions: ElementActions {
                    click: Some(click),
                    ..ElementActions::empty()
                },
            })
        }
        Primitive::Pressable { on_click, test_id, .. } => {
            let click = on_click.clone();
            Some(RobotMeta {
                kind: ElementKind::Pressable,
                test_id: *test_id,
                label: None,
                actions: ElementActions {
                    click: Some(click),
                    ..ElementActions::empty()
                },
            })
        }
        Primitive::Image { test_id, .. } => Some(RobotMeta {
            kind: ElementKind::Image,
            test_id: *test_id,
            label: None,
            actions: ElementActions::empty(),
        }),
        Primitive::TextInput { on_change, test_id, .. } => {
            let set_text = on_change.clone();
            Some(RobotMeta {
                kind: ElementKind::TextInput,
                test_id: *test_id,
                label: None,
                actions: ElementActions {
                    set_text: Some(set_text),
                    ..ElementActions::empty()
                },
            })
        }
        Primitive::TextArea { on_change, test_id, .. } => {
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
                actions: ElementActions {
                    set_text: Some(set_text),
                    ..ElementActions::empty()
                },
            })
        }
        Primitive::Toggle { on_change, test_id, .. } => {
            let set_toggle = on_change.clone();
            Some(RobotMeta {
                kind: ElementKind::Toggle,
                test_id: *test_id,
                label: None,
                actions: ElementActions {
                    set_toggle: Some(set_toggle),
                    ..ElementActions::empty()
                },
            })
        }
        Primitive::Slider { on_change, test_id, .. } => {
            let set_slider = on_change.clone();
            Some(RobotMeta {
                kind: ElementKind::Slider,
                test_id: *test_id,
                label: None,
                actions: ElementActions {
                    set_slider: Some(set_slider),
                    ..ElementActions::empty()
                },
            })
        }
        Primitive::Link { route, url, make_params, kind, target, .. } => {
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
                actions: ElementActions {
                    click: Some(click),
                    ..ElementActions::empty()
                },
            })
        }
        Primitive::Navigator { .. } => Some(RobotMeta {
            kind: ElementKind::Navigator,
            test_id: None,
            label: None,
            actions: ElementActions::empty(),
        }),
        // Structural/reactive primitives don't get registered.
        _ => None,
    }
}
