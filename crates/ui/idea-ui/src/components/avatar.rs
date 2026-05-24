//! `Avatar` — circular user-identity element.
//!
//! Renders an image when `src` is set, otherwise falls back to
//! `initials` rendered on a colored background. The `color` prop
//! picks the placeholder tint — not an intent, since an avatar is a
//! person/object placeholder, not a semantic action.
//!
//! ```ignore
//! ui! {
//!     Avatar(
//!         initials = "AB".to_string(),
//!         color = AvatarColor::Primary,
//!         size = AvatarSize::Md,
//!     )
//! }
//! ```

use runtime_core::{ui, Primitive, StyleApplication, VariantEnum};

use crate::stylesheets::{Avatar, AvatarText};
use crate::theme::IdeaThemeRef;

pub use crate::stylesheets::{AvatarColor, AvatarSize};

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct AvatarProps {
    /// Optional image URL. When `Some`, an `Image` primitive renders
    /// and the initials are hidden. When `None`, the initials show.
    pub src: Option<String>,
    /// Fallback text rendered when `src` is `None`.
    pub initials: String,
    /// Placeholder tint. Reads from `theme.intents().<color>.soft_bg`
    /// and matching `soft_text`. Distinct from `Intent` because an
    /// avatar doesn't represent a semantic action.
    pub color: AvatarColor,
    pub size: AvatarSize,
}

impl Default for AvatarProps {
    fn default() -> Self {
        Self {
            src: None,
            initials: String::new(),
            color: AvatarColor::default(),
            size: AvatarSize::default(),
        }
    }
}

pub fn avatar(props: &AvatarProps) -> Primitive {
    let size = props.size;
    let color = props.color;

    let container_style = move || {
        let _ = crate::theme_runtime::active_theme()
            .downcast_ref::<IdeaThemeRef>()
            .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
        StyleApplication::new(Avatar::sheet())
            .with("size", size.as_variant_str().to_string())
            .with("color", color.as_variant_str().to_string())
    };

    let text_style = move || {
        let _ = crate::theme_runtime::active_theme()
            .downcast_ref::<IdeaThemeRef>()
            .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
        StyleApplication::new(AvatarText::sheet())
            .with("size", size.as_variant_str().to_string())
    };

    let initials = props.initials.clone();

    match props.src.clone() {
        Some(src) => ui! {
            View(style = container_style) {
                Image(src = src)
            }
        },
        None => ui! {
            View(style = container_style) {
                Text(style = text_style) { initials }
            }
        },
    }
}
