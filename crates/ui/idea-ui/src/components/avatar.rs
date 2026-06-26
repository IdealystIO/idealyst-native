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

use runtime_core::{
    component, image_from, ui, Element, IdealystSchema, ImageSource, IntoElement, Reactive,
    StyleApplication, VariantEnum,
};

use crate::stylesheets::{Avatar as AvatarStyle, AvatarText};
use crate::theme::IdeaThemeRef;

pub use crate::stylesheets::{AvatarColor, AvatarSize};

// Reactive-by-default: `#[props]` wraps `color`/`size` → `Reactive<…>` (and
// would wrap `src`, but it's `Option<ImageSource>` — see below); `initials` is
// already reactive. The style-driving props (`color`/`size`) route into the
// container/text style sinks, read `.get()` INSIDE so the apply-style Effect
// subscribes to whichever are live. `src` selects WHICH subtree renders
// (image vs initials) — a structural branch — so it's read once at build to
// pick the branch (see the TODO in the body).
#[runtime_core::props]
#[derive(IdealystSchema)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct AvatarProps {
    /// Optional image source — a URL **or** a bundled
    /// [`Asset`](runtime_core::assets::Asset), via
    /// [`ImageSource`]. When `Some`, an image renders and the initials are
    /// hidden; when `None`, the initials show. Build it from a string
    /// (`Some("https://…".into())`) or an asset (`Some(LOGO.into())`).
    #[schema(constraint = "ImageSource — URL string or bundled Asset")]
    pub src: Option<ImageSource>,
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
            src: Reactive::Static(None),
            initials: Reactive::Static(String::new()),
            color: Reactive::Static(AvatarColor::default()),
            size: Reactive::Static(AvatarSize::default()),
        }
    }
}

/// Circular user-identity element. Renders the `src` image when set,
/// otherwise the `initials` on a `color`-tinted placeholder background.
#[component]
pub fn Avatar(props: &AvatarProps) -> Element {
    // Style-driving props route into the style sinks below, read `.get()`
    // INSIDE each closure so the apply-style Effect subscribes to whichever
    // of `size`/`color` is live. When all are `Static` the closures collapse
    // to a build-time resolution (no per-node Effect, no first-paint flicker).
    let container_style = {
        let size = props.size.clone();
        let color = props.color.clone();
        move || {
            let _ = crate::theme_runtime::active_theme_untracked()
                .downcast_ref::<IdeaThemeRef>()
                .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
            StyleApplication::new(AvatarStyle::sheet())
                .with("size", size.get().as_variant_str().to_string())
                .with("color", color.get().as_variant_str().to_string())
                // Hug + center on the cross axis so a row of mixed-size avatars
                // centers instead of top-aligning under the parent's default
                // align-items: stretch (see `components::hug_self`).
                .with_computed("hug", crate::components::hug_self)
        }
    };

    let text_style = {
        let size = props.size.clone();
        move || {
            let _ = crate::theme_runtime::active_theme_untracked()
                .downcast_ref::<IdeaThemeRef>()
                .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
            StyleApplication::new(AvatarText::sheet())
                .with("size", size.get().as_variant_str().to_string())
        }
    };

    let initials = props.initials.clone();

    // TODO(reactive-sweep): `src` selects WHICH subtree renders (image vs
    // initials). It's read once here to pick the branch, so a live `src`
    // flipping between `Some`/`None` won't swap the subtree without a parent
    // rebuild. Routing it needs a `when`/`switch` on `src.get().is_some()` so
    // the image↔initials swap happens in place — a structural-reactivity
    // change, not a plain style sink. Left as a follow-on. (`size`/`color`
    // DO re-style in place via the closures above.)
    match props.src.get() {
        Some(source) => {
            // Build the image from the unified source (URL or Asset) and
            // splat it into the circular container.
            let img = image_from(source).into_element();
            ui! {
                view(style = container_style) {
                    img
                }
            }
        }
        None => ui! {
            view(style = container_style) {
                text(style = text_style) { initials }
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use idea_theme::theme::{install_idea_theme, light_theme};
    use runtime_core::{resolve_style, AlignSelf, StyleSource};

    // Regression: an Avatar is a fixed-size atom — it must center on its
    // parent's cross axis (`align_self: Center`) so a row of mixed-size
    // avatars centers instead of top-aligning under the default
    // `align-items: stretch` (the Avatar "Sizes" row report).
    #[test]
    fn avatar_centers_on_cross_axis() {
        install_idea_theme(light_theme());
        let app = match Avatar(&AvatarProps { initials: "AB".into(), ..Default::default() }) {
            Element::View { style: Some(StyleSource::Reactive(f)), .. } => f(),
            Element::View { style: Some(StyleSource::Static(a)), .. } => a,
            _ => panic!("Avatar renders a styled View"),
        };
        assert_eq!(
            resolve_style(&app).align_self,
            Some(AlignSelf::Center),
            "an Avatar centers on the cross axis instead of stretching/top-aligning"
        );
    }
}
