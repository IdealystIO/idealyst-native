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

use runtime_core::{component, ui, Element, IdealystSchema, Reactive, StyleApplication, VariantEnum};

use crate::stylesheets::{Avatar as AvatarStyle, AvatarText};
use crate::theme::IdeaThemeRef;

pub use crate::stylesheets::{AvatarColor, AvatarSize};

#[derive(IdealystSchema)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct AvatarProps {
    /// Optional image URL. When `Some`, an `Image` primitive renders
    /// and the initials are hidden. When `None`, the initials show.
    #[schema(constraint = "absolute image URL when present")]
    pub src: Option<String>,
    /// Fallback text rendered when `src` is `None`.
    /// `Reactive<String>` — static or live (signal/`rx!`).
    pub initials: Reactive<String>,
    /// Placeholder tint. Reads from `theme.intents().<color>.soft_bg`
    /// and matching `soft_text`. Distinct from `Intent` because an
    /// avatar doesn't represent a semantic action.
    pub color: AvatarColor,
    /// Diameter scale (Sm/Md/Lg → theme avatar-size tokens).
    pub size: AvatarSize,
}

impl Default for AvatarProps {
    fn default() -> Self {
        Self {
            src: None,
            initials: Reactive::Static(String::new()),
            color: AvatarColor::default(),
            size: AvatarSize::default(),
        }
    }
}

/// Circular user-identity element. Renders the `src` image when set,
/// otherwise the `initials` on a `color`-tinted placeholder background.
#[component]
pub fn Avatar(props: &AvatarProps) -> Element {
    let size = props.size;
    let color = props.color;

    let container_style = move || {
        let _ = crate::theme_runtime::active_theme()
            .downcast_ref::<IdeaThemeRef>()
            .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
        StyleApplication::new(AvatarStyle::sheet())
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
            view(style = container_style) {
                image(src = src)
            }
        },
        None => ui! {
            view(style = container_style) {
                text(style = text_style) { initials }
            }
        },
    }
}
