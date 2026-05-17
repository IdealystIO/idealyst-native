//! `Field` — a labeled text input with optional helper / error text.
//!
//! Wraps `TextInput` with the [`Field`](crate::stylesheets::Field)
//! styling, an optional label above, and helper text below. Error
//! state is driven by `error: Option<String>` — when set, the input
//! gets the `error` tone and the helper line renders in red.
//!
//! ```ignore
//! let email = signal!("".to_string());
//! ui! {
//!     Field(
//!         label = "Email",
//!         value = email,
//!         on_change = move |v: String| email.set(v),
//!         placeholder = "you@example.com",
//!         help = "We'll never share your email."
//!     )
//! }
//! ```

use framework_core::{ui, Primitive, Signal};
use std::rc::Rc;

use crate::stylesheets::{Field, FieldGroup, FieldHelp, FieldHelpTone, FieldLabel};
pub use crate::stylesheets::{FieldSize, FieldTone};

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct FieldProps {
    /// Optional label rendered above the input.
    pub label: Option<String>,
    /// Controlled signal — source of truth for the input's value.
    pub value: Signal<String>,
    /// Change callback. Same shape as the underlying `TextInput`.
    pub on_change: Rc<dyn Fn(String)>,
    /// Placeholder shown when the input is empty.
    pub placeholder: Option<String>,
    /// Helper text rendered below the input.
    pub help: Option<String>,
    /// If set, switches the field into its error state and replaces
    /// the helper text with the error message.
    pub error: Option<String>,
    pub size: FieldSize,
}

impl Default for FieldProps {
    fn default() -> Self {
        Self {
            label: None,
            // Caller must override `value` — a freshly-created signal
            // is fine for the default since it'll never be observed.
            value: Signal::new(String::new()),
            on_change: Rc::new(|_| {}),
            placeholder: None,
            help: None,
            error: None,
            size: FieldSize::default(),
        }
    }
}

pub fn field(props: &FieldProps) -> Primitive {
    let value = props.value;
    let on_change = props.on_change.clone();
    let placeholder = props.placeholder.clone();
    let size = props.size;
    let has_error = props.error.is_some();
    let tone = if has_error { FieldTone::Error } else { FieldTone::Default };

    let input_style = Field().size(size).tone(tone);
    let label_text = props.label.clone();
    let help_text = props
        .error
        .clone()
        .or_else(|| props.help.clone());
    let help_tone = if has_error { FieldHelpTone::Error } else { FieldHelpTone::Default };
    let help_style = FieldHelp().tone(help_tone);
    let label_style = FieldLabel();
    let group_style = FieldGroup();

    // Build the TextInput primitive (with or without placeholder).
    let input_node: Primitive = if let Some(p) = placeholder {
        ui! {
            TextInput(
                value = value,
                on_change = move |v: String| (on_change)(v),
                placeholder = p,
                style = input_style
            )
        }
    } else {
        ui! {
            TextInput(
                value = value,
                on_change = move |v: String| (on_change)(v),
                style = input_style
            )
        }
    };

    // Compose: label? + input + help?
    let mut children: Vec<Primitive> = Vec::with_capacity(3);
    if let Some(l) = label_text {
        children.push(ui! { Text(style = label_style) { l } });
    }
    children.push(input_node);
    if let Some(h) = help_text {
        children.push(ui! { Text(style = help_style) { h } });
    }

    ui! { View(style = group_style) { children } }
}
