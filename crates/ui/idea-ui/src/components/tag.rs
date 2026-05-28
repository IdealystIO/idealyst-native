//! `Tag` — labelled pill with optional close button, built on the
//! extensible Tone + Variant trait surface.
//!
//! ```ignore
//! use std::rc::Rc;
//! use idea_ui::extensible::tag::{tag, TagProps};
//! use idea_theme::extensible::{tone, variant};
//!
//! ui! {
//!     Tag(
//!         label = "Rust",
//!         tone = tone::Primary,
//!         variant = variant::Soft,
//!         on_remove = Some(Rc::new(move || remove("Rust"))),
//!     )
//! }
//! ```
//!
//! Same Tone + Variant axes as [`badge`](super::badge::badge) — the
//! only difference is the optional close affordance. Reuses
//! [`Tag`](crate::stylesheets::Tag) base sheet for the container
//! and [`TagLabel`](crate::stylesheets::TagLabel)/[`TagClose`](crate::stylesheets::TagClose)
//! for the children.

use std::rc::Rc;

use runtime_core::{ui, IntoElement, Element, Reactive, StyleApplication};

use idea_theme::extensible::{installed_tag_sheet, tone, variant, ToneRef, VariantRef};

use crate::stylesheets::{TagClose, TagLabel};

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct TagProps {
    /// Tag text. `Reactive<String>` — static or live (signal/`rx!`).
    pub label: Reactive<String>,
    pub tone: ToneRef,
    pub variant: VariantRef,
    /// When `Some`, a close button renders to the right of the label.
    pub on_remove: Option<Rc<dyn Fn()>>,
}

impl Default for TagProps {
    fn default() -> Self {
        Self {
            label: Reactive::Static(String::new()),
            tone: tone::Neutral.into(),
            variant: variant::Soft.into(),
            on_remove: None,
        }
    }
}

pub fn tag(props: &TagProps) -> Element {
    let label = props.label.clone();
    let tone = props.tone.clone();
    let variant = props.variant.clone();

    let appearance_key = format!("{}_{}", tone.key(), variant.key());

    // Static style — build-time apply, no flicker (see Button).
    let container_style =
        StyleApplication::new(installed_tag_sheet()).with("appearance", appearance_key);

    let label_style = TagLabel();
    let close_style = TagClose();

    match props.on_remove.clone() {
        Some(on_remove) => {
            let close_text = runtime_core::text("×".to_string()).into_element();
            let close = runtime_core::pressable(vec![close_text], move || (on_remove)())
                .with_style(close_style)
                .into_element();
            ui! {
                View(style = container_style) {
                    Text(style = label_style) { label }
                    close
                }
            }
        }
        None => ui! {
            View(style = container_style) {
                Text(style = label_style) { label }
            }
        },
    }
}
