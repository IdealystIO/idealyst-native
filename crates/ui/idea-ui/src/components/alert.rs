//! `Alert` — banner with title + optional body and dismiss button,
//! built on the extensible Tone + Variant trait surface.
//!
//! ```ignore
//! use std::rc::Rc;
//! use idea_ui::extensible::alert::{alert, AlertProps};
//! use idea_theme::extensible::{tone, variant};
//!
//! ui! {
//!     Alert(
//!         title = "Couldn't save",
//!         body = Some("Server returned 503.".to_string()),
//!         tone = tone::Danger,
//!         variant = variant::Soft,
//!         on_dismiss = Some(Rc::new(move || hide_alert())),
//!     )
//! }
//! ```
//!
//! Same Tone + Variant axes as [`badge`](super::badge::badge). Alert
//! has its own padding/font/radius in the base stylesheet, so no
//! Size/Shape axis — adding one would imply a continuous range of
//! banner densities which we don't have a use for yet.

use std::rc::Rc;

use runtime_core::{component, ui, IntoElement, Element, Reactive, StyleApplication};

use idea_theme::extensible::{installed_alert_sheet, tone, variant, ToneRef, VariantRef};

use crate::stylesheets::{AlertBody, AlertTitle, TagClose};

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct AlertProps {
    /// Alert title. `Reactive<String>` — static or live (signal/`rx!`).
    pub title: Reactive<String>,
    /// Optional second-line detail text, beneath the title.
    /// `Reactive<Option<String>>` — static or live.
    pub body: Reactive<Option<String>>,
    pub tone: ToneRef,
    pub variant: VariantRef,
    /// When `Some`, a close affordance appears in the top-right.
    pub on_dismiss: Option<Rc<dyn Fn()>>,
}

impl Default for AlertProps {
    fn default() -> Self {
        Self {
            title: Reactive::Static(String::new()),
            body: Reactive::Static(None),
            // Info/Soft = the common informational alert. Use Danger/Filled
            // for breaking news, Warning/Soft for cautionary, etc.
            tone: tone::Info.into(),
            variant: variant::Soft.into(),
            on_dismiss: None,
        }
    }
}

#[component]
pub fn Alert(props: &AlertProps) -> Element {
    let title = props.title.clone();
    let tone = props.tone.clone();
    let variant = props.variant.clone();

    let appearance_key = format!("{}_{}", tone.key(), variant.key());

    // Static style — build-time apply, no flicker (see Button).
    let container_style =
        StyleApplication::new(installed_alert_sheet()).with("appearance", appearance_key);

    let title_style = AlertTitle();
    let body_style = AlertBody();
    let close_style = TagClose();

    let title_node: Element = ui! { Text(style = title_style) { title } };
    let body_node: Option<Element> =
        crate::components::optional_reactive_text(props.body.clone(), body_style);

    let close_node: Option<Element> = props.on_dismiss.clone().map(|on_dismiss| {
        let close_text = runtime_core::text("×".to_string()).into_element();
        runtime_core::pressable(vec![close_text], move || (on_dismiss)())
            .with_style(close_style)
            .into_element()
    });

    let mut children: Vec<Element> = Vec::with_capacity(2);
    let mut text_column: Vec<Element> = Vec::with_capacity(2);
    text_column.push(title_node);
    if let Some(b) = body_node {
        text_column.push(b);
    }
    children.push(ui! { View { text_column } });
    if let Some(c) = close_node {
        children.push(c);
    }

    ui! { View(style = container_style) { children } }
}
