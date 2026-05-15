//! `Badge` — small pill-shaped status indicator. Coloring is driven
//! by an [`Intent`] — the same vocabulary `Pressable` uses.
//!
//! ```ignore
//! use idea_ui::{Primary, IntoRcIntent};
//!
//! ui! { Badge(label = "New", intent = Primary.into_rc()) }
//! ```

use std::rc::Rc;

use framework_core::{ui, Primitive, StyleApplication};

use crate::intent::{apply_palette, Intent, IntoRcIntent, Neutral};
use crate::stylesheets::Badge;
use crate::theme::IdeaThemeRef;

pub struct BadgeProps {
    pub label: String,
    /// Defaults to [`Neutral`] — a muted surface-tinted look.
    pub intent: Rc<dyn Intent>,
}

impl Default for BadgeProps {
    fn default() -> Self {
        Self {
            label: String::new(),
            intent: Neutral.into_rc(),
        }
    }
}

pub fn badge(props: &BadgeProps) -> Primitive {
    let label = props.label.clone();
    let intent: Rc<dyn Intent> = props.intent.clone();

    let style = move || {
        let theme = framework_core::active_theme();
        let theme_ref = theme
            .downcast_ref::<IdeaThemeRef>()
            .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
        let palette = intent.palette(theme_ref);

        let app = StyleApplication::new(Badge::sheet());
        apply_palette(app, &palette)
    };

    ui! { Text(style = style) { label } }
}
