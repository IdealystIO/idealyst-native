//! `Switch` — passthrough to the framework's `Toggle` primitive with
//! an optional inline label.
//!
//! ```ignore
//! ui! {
//!     Switch(
//!         label = "Notifications",
//!         value = on,
//!         on_change = move |v: bool| on.set(v),
//!     )
//! }
//! ```
//!
//! No tone/variant/size axes today — the framework `Toggle`'s
//! appearance is platform-native. When the framework primitive
//! grows a style hook, this is the place a Tone axis would land.
//! Until then this lives in the `extensible` namespace for
//! consistent layout — the API matches the closed-enum
//! [`crate::components::switch`].

use std::rc::Rc;

use runtime_core::{component, ui, Element, Reactive, Signal};

use crate::stylesheets::{FieldLabel, SwitchRow};

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct SwitchProps {
    /// Optional inline label. `Reactive<Option<String>>` — `None` /
    /// `Some("…")` are static; a `Signal<Option<String>>` or
    /// `rx!(Some(…))` makes the label text live.
    pub label: Reactive<Option<String>>,
    pub value: Signal<bool>,
    pub on_change: Rc<dyn Fn(bool)>,
}

impl Default for SwitchProps {
    fn default() -> Self {
        Self {
            label: Reactive::Static(None),
            value: Signal::new(false),
            on_change: Rc::new(|_| {}),
        }
    }
}

#[component]
pub fn Switch(props: &SwitchProps) -> Element {
    let value = props.value;
    let on_change = props.on_change.clone();

    let label_node = crate::components::optional_reactive_text(props.label.clone(), FieldLabel());

    match label_node {
        Some(label) => ui! {
            view(style = SwitchRow()) {
                label
                toggle(value = value, on_change = move |v: bool| (on_change)(v))
            }
        },
        None => ui! { toggle(value = value, on_change = move |v: bool| (on_change)(v)) },
    }
}
