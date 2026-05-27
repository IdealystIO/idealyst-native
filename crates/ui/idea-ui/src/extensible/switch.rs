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

use runtime_core::{ui, Primitive, Signal};

use crate::stylesheets::{FieldLabel, SwitchRow};

pub struct SwitchProps {
    pub label: Option<String>,
    pub value: Signal<bool>,
    pub on_change: Rc<dyn Fn(bool)>,
}

impl Default for SwitchProps {
    fn default() -> Self {
        Self {
            label: None,
            value: Signal::new(false),
            on_change: Rc::new(|_| {}),
        }
    }
}

pub fn switch(props: &SwitchProps) -> Primitive {
    let value = props.value;
    let on_change = props.on_change.clone();
    let label_text = props.label.clone();

    if let Some(l) = label_text {
        ui! {
            View(style = SwitchRow()) {
                Text(style = FieldLabel()) { l }
                Toggle(value = value, on_change = move |v: bool| (on_change)(v))
            }
        }
    } else {
        ui! { Toggle(value = value, on_change = move |v: bool| (on_change)(v)) }
    }
}
