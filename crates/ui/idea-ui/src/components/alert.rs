//! `Alert` — a banner conveying a notable message. Same intent +
//! kind vocabulary as [`Badge`](super::badge::Badge); kinds are
//! Solid / Soft / Outlined (no Ghost — an alert needs a visible
//! surface).
//!
//! ```ignore
//! use std::rc::Rc;
//!
//! ui! {
//!     Alert(
//!         title = "Couldn't save".to_string(),
//!         body = Some("Server returned 503.".to_string()),
//!         intent = IntentTag::Danger,
//!         kind = BadgeKind::Soft,
//!         on_dismiss = Some(Rc::new(move || hide_alert()))
//!     )
//! }
//! ```

use std::rc::Rc;

use framework_core::{ui, IntoPrimitive, Primitive, StyleApplication};

use crate::components::badge::BadgeKind;
use crate::components::button::IntentTag;
use crate::stylesheets::{Alert, AlertBody, AlertTitle, TagClose};
use crate::theme::IdeaThemeRef;

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct AlertProps {
    pub title: String,
    /// Optional second-line detail text. Rendered beneath the title.
    pub body: Option<String>,
    pub intent: IntentTag,
    pub kind: BadgeKind,
    /// When `Some`, a close affordance appears in the top-right.
    pub on_dismiss: Option<Rc<dyn Fn()>>,
}

impl Default for AlertProps {
    fn default() -> Self {
        Self {
            title: String::new(),
            body: None,
            // Default to Info/Soft — informational alerts are the
            // common case; "use Danger/Solid for breaking news".
            intent: IntentTag::Info,
            kind: BadgeKind::Soft,
            on_dismiss: None,
        }
    }
}

pub fn alert(props: &AlertProps) -> Primitive {
    let title = props.title.clone();
    let body = props.body.clone();
    let intent = props.intent;
    let kind = props.kind;
    let appearance = format!("{}_{}", intent.as_str(), kind.as_str());

    let container_style = {
        let appearance = appearance.clone();
        move || {
            let _ = framework_theme::active_theme()
                .downcast_ref::<IdeaThemeRef>()
                .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
            StyleApplication::new(Alert::sheet()).with("appearance", appearance.clone())
        }
    };

    let title_style = AlertTitle();
    let body_style = AlertBody();
    let close_style = TagClose();

    let title_node: Primitive = ui! { Text(style = title_style) { title } };
    let body_node: Option<Primitive> = body.map(|b| ui! { Text(style = body_style) { b } });

    let close_node: Option<Primitive> = props.on_dismiss.clone().map(|on_dismiss| {
        // Bare-clickable close × — see Tag for the same reasoning.
        let close_text = framework_core::text("×".to_string()).into_primitive();
        framework_core::pressable(vec![close_text], move || (on_dismiss)())
            .with_style(close_style)
            .into_primitive()
    });

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
