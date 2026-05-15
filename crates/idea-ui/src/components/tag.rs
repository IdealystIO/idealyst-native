//! `Tag` — a labelled pill, optionally dismissable.
//!
//! Like `Badge`, but with a configurable close affordance. When
//! `on_remove` is `Some`, a small "×" sits to the right of the
//! label; clicking it fires the callback. The host is responsible
//! for actually removing the tag from its data — `Tag` is stateless.
//!
//! ```ignore
//! use idea_ui::{Primary, IntoRcIntent};
//! use std::rc::Rc;
//!
//! ui! {
//!     Tag(
//!         label = "Rust".to_string(),
//!         intent = Primary.into_rc(),
//!         on_remove = Some(Rc::new(move || remove("Rust")))
//!     )
//! }
//! ```

use std::rc::Rc;

use framework_core::{ui, Primitive, StyleApplication};

use crate::intent::{apply_palette, Intent, IntoRcIntent, Neutral};
use crate::stylesheets::{Tag, TagClose, TagLabel};
use crate::theme::IdeaThemeRef;

pub struct TagProps {
    pub label: String,
    pub intent: Rc<dyn Intent>,
    /// When `Some`, a close button renders to the right of the label.
    /// When `None`, the tag is non-dismissable.
    pub on_remove: Option<Rc<dyn Fn()>>,
}

impl Default for TagProps {
    fn default() -> Self {
        Self {
            label: String::new(),
            intent: Neutral.into_rc(),
            on_remove: None,
        }
    }
}

pub fn tag(props: &TagProps) -> Primitive {
    let label = props.label.clone();
    let intent: Rc<dyn Intent> = props.intent.clone();
    let intent_for_label = intent.clone();

    let container_style = move || {
        let theme = framework_core::active_theme();
        let theme_ref = theme
            .downcast_ref::<IdeaThemeRef>()
            .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
        let palette = intent.palette(theme_ref);
        let app = StyleApplication::new(Tag::sheet());
        apply_palette(app, &palette)
    };

    let label_style = move || {
        let theme = framework_core::active_theme();
        let theme_ref = theme
            .downcast_ref::<IdeaThemeRef>()
            .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
        let palette = intent_for_label.palette(theme_ref);
        StyleApplication::new(TagLabel::sheet()).override_color(palette.foreground)
    };

    let close_style = TagClose();

    match props.on_remove.clone() {
        Some(on_remove) => ui! {
            View(style = container_style) {
                Text(style = label_style) { label }
                Button(
                    label = "×".to_string(),
                    on_click = move || (on_remove)(),
                    style = close_style
                )
            }
        },
        None => ui! {
            View(style = container_style) {
                Text(style = label_style) { label }
            }
        },
    }
}
