//! `Typography` — text component driven by the extensible
//! [`TypographyKind`](super::TypographyKind) trait.
//!
//! ```ignore
//! use idea_ui::extensible::{typography, typography_component::TypographyProps};
//!
//! ui! {
//!     Typography(
//!         content = "Welcome".into(),
//!         kind = typography::H1,
//!     )
//! }
//! ```
//!
//! The `kind` axis carries the full per-variant character — font size,
//! weight, line height, letter spacing — so apps add a `SexySubtitle`
//! kind by implementing the trait on a marker type without re-deriving
//! anything.
//!
//! Color: defaults to `theme.colors().text`. Set `muted = true` to use
//! `theme.colors().text_muted`. Intent-colored text (e.g. a Danger
//! heading) is a future axis — for now apps that need it can wrap the
//! call in a colored container.

use std::rc::Rc;

use runtime_core::{text, IntoPrimitive, Primitive, StyleApplication, StyleRules, TextAlign};

use idea_theme::active_theme;
use idea_theme::extensible::{typography as kinds, TypographyKind};
use idea_theme::theme::{IdeaTheme, IdeaThemeRef};
use crate::stylesheets::Typography as TypographySheet;

/// Props for the extensible Typography. `kind` is a trait object so
/// the prop accepts any `TypographyKind` impl — built-in or custom.
pub struct TypographyProps {
    pub content: String,
    pub kind: Rc<dyn TypographyKind>,
    /// When `true`, render with the theme's muted text color instead
    /// of the default text color.
    pub muted: bool,
    pub align: TextAlign,
}

impl Default for TypographyProps {
    fn default() -> Self {
        Self {
            content: String::new(),
            kind: Rc::new(kinds::Body),
            muted: false,
            align: TextAlign::Left,
        }
    }
}

/// Render Typography. The computed layer pulls font-size, font-weight,
/// line-height, letter-spacing from the `kind` and text-align from the
/// prop. Color is read from the theme directly so it follows theme
/// swap without needing a per-instance token.
pub fn typography(props: &TypographyProps) -> Primitive {
    let content = props.content.clone();
    let kind = props.kind.clone();
    let muted = props.muted;
    let align = props.align;

    // The computed cache key — every (kind, muted, align) combination
    // gets its own resolved StyleRules. Sharing keys across instances
    // is what makes the same Typography call site materialize one CSS
    // class regardless of how many times it renders.
    let cache_key = format!(
        "{}+{}+{:?}",
        kind.key(),
        if muted { "muted" } else { "default" },
        align,
    );

    let style = move || {
        // Subscribe to theme changes so the apply Effect re-runs on swap.
        let _ = active_theme()
            .downcast_ref::<IdeaThemeRef>()
            .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");

        let k = kind.clone();
        let compute = move || -> StyleRules {
            let theme = active_theme();
            let theme_ref = theme
                .downcast_ref::<IdeaThemeRef>()
                .expect("idea-ui: no IdeaTheme installed");
            let color = if muted {
                theme_ref.colors().text_muted.clone()
            } else {
                theme_ref.colors().text.clone()
            };
            StyleRules {
                font_size: Some(k.font_size()),
                font_weight: Some(k.font_weight()),
                line_height: Some(k.line_height()),
                letter_spacing: Some(k.letter_spacing()),
                color: Some(color),
                text_align: Some(align),
                ..Default::default()
            }
        };

        StyleApplication::new(TypographySheet::sheet()).with_computed(cache_key.clone(), compute)
    };

    text(content).with_style(style).into_primitive()
}
