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

use runtime_core::{ui, IntoPrimitive, Primitive, StyleApplication, StyleRules};

use idea_theme::extensible::{tone, variant, ResolutionCtx, ToneRef, VariantRef};
use idea_theme::theme::IdeaThemeRef;

use crate::stylesheets::{Tag as TagSheet, TagClose, TagLabel};

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct TagProps {
    pub label: String,
    pub tone: ToneRef,
    pub variant: VariantRef,
    /// When `Some`, a close button renders to the right of the label.
    pub on_remove: Option<Rc<dyn Fn()>>,
}

impl Default for TagProps {
    fn default() -> Self {
        Self {
            label: String::new(),
            tone: tone::Neutral.into(),
            variant: variant::Soft.into(),
            on_remove: None,
        }
    }
}

pub fn tag(props: &TagProps) -> Primitive {
    let label = props.label.clone();
    let tone = props.tone.clone();
    let variant = props.variant.clone();

    let cache_key = format!("tag+{}+{}", variant.key(), tone.key());

    let container_style = {
        let tone = tone.clone();
        let variant = variant.clone();
        let cache_key = cache_key.clone();
        move || {
            let _ = idea_theme::active_theme()
                .downcast_ref::<IdeaThemeRef>()
                .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
            let var = variant.clone();
            let tn = tone.clone();
            let compute = move || -> StyleRules {
                let theme = idea_theme::active_theme();
                let theme_ref = theme
                    .downcast_ref::<IdeaThemeRef>()
                    .expect("idea-ui: no IdeaTheme installed");
                let ctx = ResolutionCtx {
                    theme: theme_ref,
                    tone: &*tn,
                };
                var.render(&ctx)
            };
            StyleApplication::new(TagSheet::sheet()).with_computed(cache_key.clone(), compute)
        }
    };

    let label_style = TagLabel();
    let close_style = TagClose();

    match props.on_remove.clone() {
        Some(on_remove) => {
            let close_text = runtime_core::text("×".to_string()).into_primitive();
            let close = runtime_core::pressable(vec![close_text], move || (on_remove)())
                .with_style(close_style)
                .into_primitive();
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
