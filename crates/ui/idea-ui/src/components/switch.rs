//! `Switch` — a `Toggle` with an optional inline label.
//!
//! ```ignore
//! let on = signal!(true);
//! ui! {
//!     Switch(
//!         label = "Notifications",
//!         value = on,
//!         on_change = move |v: bool| on.set(v)
//!     )
//! }
//! ```

use runtime_core::{ui, Primitive, Signal};
use std::rc::Rc;

use crate::stylesheets::{FieldLabel, SwitchRow};

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct SwitchProps {
    /// Optional label rendered to the left of the toggle.
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
    let row_style = SwitchRow();
    let label_style = FieldLabel();

    if let Some(l) = label_text {
        ui! {
            View(style = row_style) {
                Text(style = label_style) { l }
                Toggle(value = value, on_change = move |v: bool| (on_change)(v))
            }
        }
    } else {
        ui! { Toggle(value = value, on_change = move |v: bool| (on_change)(v)) }
    }
}
