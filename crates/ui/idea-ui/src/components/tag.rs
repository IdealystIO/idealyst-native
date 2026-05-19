//! `Tag` — a labelled pill, optionally dismissable. Same intent +
//! kind vocabulary as [`Badge`](super::badge::Badge).
//!
//! ```ignore
//! use std::rc::Rc;
//!
//! ui! {
//!     Tag(
//!         label = "Rust".to_string(),
//!         intent = IntentTag::Primary,
//!         kind = BadgeKind::Soft,
//!         on_remove = Some(Rc::new(move || remove("Rust")))
//!     )
//! }
//! ```

use std::rc::Rc;

use framework_core::{ui, IntoPrimitive, Primitive, StyleApplication};

use crate::components::badge::BadgeKind;
use crate::components::button::IntentTag;
use crate::stylesheets::{Tag, TagClose, TagLabel};
use crate::theme::IdeaThemeRef;

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct TagProps {
    pub label: String,
    pub intent: IntentTag,
    pub kind: BadgeKind,
    /// When `Some`, a close button renders to the right of the label.
    pub on_remove: Option<Rc<dyn Fn()>>,
}

impl Default for TagProps {
    fn default() -> Self {
        Self {
            label: String::new(),
            intent: IntentTag::Neutral,
            kind: BadgeKind::Soft,
            on_remove: None,
        }
    }
}

pub fn tag(props: &TagProps) -> Primitive {
    let label = props.label.clone();
    let intent = props.intent;
    let kind = props.kind;
    let appearance = format!("{}_{}", intent.as_str(), kind.as_str());

    let container_style = {
        let appearance = appearance.clone();
        move || {
            let _ = framework_theme::active_theme()
                .downcast_ref::<IdeaThemeRef>()
                .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
            StyleApplication::new(Tag::sheet()).with("appearance", appearance.clone())
        }
    };

    let label_style = TagLabel();
    let close_style = TagClose();

    match props.on_remove.clone() {
        Some(on_remove) => {
            // Close × built on the framework's pressable primitive,
            // not `Button(...)` — the ui!-side `Button` tag is now
            // idea-ui's styled Button, and we want a bare clickable
            // here that the TagClose stylesheet fully owns.
            let close_text = framework_core::text("×".to_string()).into_primitive();
            let close = framework_core::pressable(vec![close_text], move || (on_remove)())
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
