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

use runtime_core::{ui, IntoPrimitive, Primitive, StyleApplication, StyleRules};

use idea_theme::extensible::{tone, variant, ResolutionCtx, Tone, Variant};
use idea_theme::theme::IdeaThemeRef;

use crate::stylesheets::{Alert as AlertSheet, AlertBody, AlertTitle, TagClose};

pub struct AlertProps {
    pub title: String,
    /// Optional second-line detail text. Rendered beneath the title.
    pub body: Option<String>,
    pub tone: Rc<dyn Tone>,
    pub variant: Rc<dyn Variant>,
    /// When `Some`, a close affordance appears in the top-right.
    pub on_dismiss: Option<Rc<dyn Fn()>>,
}

impl Default for AlertProps {
    fn default() -> Self {
        Self {
            title: String::new(),
            body: None,
            // Info/Soft = the common informational alert. Use Danger/Filled
            // for breaking news, Warning/Soft for cautionary, etc.
            tone: Rc::new(tone::Info),
            variant: Rc::new(variant::Soft),
            on_dismiss: None,
        }
    }
}

pub fn alert(props: &AlertProps) -> Primitive {
    let title = props.title.clone();
    let body = props.body.clone();
    let tone = props.tone.clone();
    let variant = props.variant.clone();

    let cache_key = format!("alert+{}+{}", variant.key(), tone.key());

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
            StyleApplication::new(AlertSheet::sheet()).with_computed(cache_key.clone(), compute)
        }
    };

    let title_style = AlertTitle();
    let body_style = AlertBody();
    let close_style = TagClose();

    let title_node: Primitive = ui! { Text(style = title_style) { title } };
    let body_node: Option<Primitive> = body.map(|b| ui! { Text(style = body_style) { b } });

    let close_node: Option<Primitive> = props.on_dismiss.clone().map(|on_dismiss| {
        let close_text = runtime_core::text("×".to_string()).into_primitive();
        runtime_core::pressable(vec![close_text], move || (on_dismiss)())
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
