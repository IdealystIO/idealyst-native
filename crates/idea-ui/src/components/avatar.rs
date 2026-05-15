//! `Avatar` — circular user-identity element.
//!
//! Renders an image when `src` is set, otherwise falls back to
//! `initials` rendered on a colored background. Sized via the
//! `size` variant; coloring of the initials background is driven by
//! an [`Intent`] so apps can theme it.
//!
//! ```ignore
//! use idea_ui::{Primary, IntoRcIntent};
//!
//! ui! {
//!     Avatar(
//!         src = Some("https://example.com/avatar.png".to_string()),
//!         initials = "AB".to_string(),
//!         intent = Primary.into_rc()
//!     )
//! }
//! ```

use std::rc::Rc;

use framework_core::{ui, Primitive, StyleApplication, VariantEnum};

use crate::intent::{apply_palette, Intent, IntoRcIntent, Neutral};
use crate::stylesheets::{Avatar, AvatarText};
use crate::theme::IdeaThemeRef;

pub use crate::stylesheets::AvatarSize;

pub struct AvatarProps {
    /// Optional image URL. When `Some`, an `Image` primitive renders
    /// and the initials are hidden. When `None`, the initials show.
    pub src: Option<String>,
    /// Fallback text rendered when `src` is `None`. Typically the
    /// user's initials ("AB"); the component doesn't crop or
    /// uppercase, so pass it pre-formatted.
    pub initials: String,
    pub intent: Rc<dyn Intent>,
    pub size: AvatarSize,
}

impl Default for AvatarProps {
    fn default() -> Self {
        Self {
            src: None,
            initials: String::new(),
            intent: Neutral.into_rc(),
            size: AvatarSize::default(),
        }
    }
}

pub fn avatar(props: &AvatarProps) -> Primitive {
    let size = props.size;
    let intent: Rc<dyn Intent> = props.intent.clone();
    let intent_for_text = intent.clone();

    // Outer container: circular, sized, intent-colored background.
    let container_style = move || {
        let theme = framework_core::active_theme();
        let theme_ref = theme
            .downcast_ref::<IdeaThemeRef>()
            .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
        let palette = intent.palette(theme_ref);
        let app = StyleApplication::new(Avatar::sheet())
            .with("size", size.as_variant_str().to_string());
        apply_palette(app, &palette)
    };

    // Text style: foreground from intent, centered, sized to the avatar.
    let text_style = move || {
        let theme = framework_core::active_theme();
        let theme_ref = theme
            .downcast_ref::<IdeaThemeRef>()
            .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
        let palette = intent_for_text.palette(theme_ref);
        StyleApplication::new(AvatarText::sheet())
            .with("size", size.as_variant_str().to_string())
            .override_color(palette.foreground)
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
