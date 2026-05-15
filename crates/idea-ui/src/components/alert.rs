//! `Alert` — a banner conveying a notable message.
//!
//! Composes a tinted background (from the intent), a title, an
//! optional body, and an optional dismiss affordance. Use for
//! inline status messages — success confirmations, validation
//! errors, "you're in dev mode" banners.
//!
//! ```ignore
//! use idea_ui::{Danger, IntoRcIntent};
//! use std::rc::Rc;
//!
//! ui! {
//!     Alert(
//!         title = "Couldn't save".to_string(),
//!         body = Some("Server returned 503.".to_string()),
//!         intent = Danger.into_rc(),
//!         on_dismiss = Some(Rc::new(move || hide_alert()))
//!     )
//! }
//! ```

use std::rc::Rc;

use framework_core::{ui, Primitive, StyleApplication};

use crate::intent::{apply_palette, Intent, IntoRcIntent, Primary};
use crate::stylesheets::{Alert, AlertBody, AlertTitle, TagClose};
use crate::theme::IdeaThemeRef;

pub struct AlertProps {
    pub title: String,
    /// Optional second-line detail text. Rendered beneath the title.
    pub body: Option<String>,
    pub intent: Rc<dyn Intent>,
    /// When `Some`, a close affordance appears in the top-right
    /// corner. When `None`, the alert is non-dismissable.
    pub on_dismiss: Option<Rc<dyn Fn()>>,
}

impl Default for AlertProps {
    fn default() -> Self {
        Self {
            title: String::new(),
            body: None,
            intent: Primary.into_rc(),
            on_dismiss: None,
        }
    }
}

pub fn alert(props: &AlertProps) -> Primitive {
    let title = props.title.clone();
    let body = props.body.clone();
    let intent: Rc<dyn Intent> = props.intent.clone();
    let intent_for_title = intent.clone();
    let intent_for_body = intent.clone();

    let container_style = move || {
        let theme = framework_core::active_theme();
        let theme_ref = theme
            .downcast_ref::<IdeaThemeRef>()
            .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
        let palette = intent.palette(theme_ref);
        let app = StyleApplication::new(Alert::sheet());
        apply_palette(app, &palette)
    };

    let title_style = move || {
        let theme = framework_core::active_theme();
        let theme_ref = theme
            .downcast_ref::<IdeaThemeRef>()
            .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
        let palette = intent_for_title.palette(theme_ref);
        StyleApplication::new(AlertTitle::sheet()).override_color(palette.foreground)
    };

    let body_style = move || {
        let theme = framework_core::active_theme();
        let theme_ref = theme
            .downcast_ref::<IdeaThemeRef>()
            .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
        let palette = intent_for_body.palette(theme_ref);
        StyleApplication::new(AlertBody::sheet()).override_color(palette.foreground)
    };

    let close_style = TagClose();

    // Compose the message text column.
    let title_node: Primitive = ui! { Text(style = title_style) { title } };
    let body_node: Option<Primitive> = body.map(|b| ui! { Text(style = body_style) { b } });

    // Compose the optional close button.
    let close_node: Option<Primitive> = props.on_dismiss.clone().map(|on_dismiss| {
        ui! {
            Button(
                label = "×".to_string(),
                on_click = move || (on_dismiss)(),
                style = close_style
            )
        }
    });

    // Final layout: title + body stacked, with the close button to
    // the right. Built as one View; the inner stack of text lives
    // inline so we don't introduce a separate child component just
    // for the column.
    let mut children: Vec<Primitive> = Vec::with_capacity(2);
    let mut text_column: Vec<Primitive> = Vec::with_capacity(2);
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
