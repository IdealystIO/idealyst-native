//! [`LayoutTheme`] — the kit's own theme tokens: the measures a layout
//! takes that an app may want to change as one.

use runtime_core::{Length, Tokens};

/// The page gutter at each form factor (rule `gutter`): the space
/// between the screen's edge and its content. The header, the toolbar
/// row, a page's body and a pinned action bar all take it, so a body's
/// first text stands under the title.
#[derive(Tokens, Clone, Debug, PartialEq)]
pub struct Gutter {
    /// Under 640 (`layout-gutter-phone`).
    pub phone: Length,
    /// From 640 (`layout-gutter-tablet`).
    pub tablet: Length,
    /// From 1024 (`layout-gutter-desktop`).
    pub desktop: Length,
}

/// The kit's theme: its tokens, installed beside the idea theme's and
/// named under `layout-`. Every kit stylesheet reads these by name, so
/// installing a changed theme re-gutters every screen at once; an app
/// that installs none gets [`LayoutTheme::standard`].
///
/// ```ignore
/// // In the app's brand module, after `install_idea_theme`:
/// LayoutTheme::install(LayoutTheme::standard().with(|t| {
///     t.gutter.tablet = 16.0.into();
///     t.gutter.desktop = 24.0.into();
/// }));
/// ```
#[derive(Tokens, Clone, Debug, PartialEq)]
#[tokens(default = LayoutTheme::standard, prefix = "layout")]
pub struct LayoutTheme {
    /// The page gutter.
    pub gutter: Gutter,
}

/// [`LayoutTheme::standard`]'s gutter on a phone: the idea theme's `lg`
/// step.
const GUTTER_PHONE: f32 = 16.0;
/// On a tablet: the idea theme's `xl` step.
const GUTTER_TABLET: f32 = 24.0;
/// On a desktop: the idea theme's `xxl` step.
const GUTTER_DESKTOP: f32 = 32.0;

impl LayoutTheme {
    /// The kit's defaults.
    pub fn standard() -> Self {
        Self {
            gutter: Gutter {
                phone: GUTTER_PHONE.into(),
                tablet: GUTTER_TABLET.into(),
                desktop: GUTTER_DESKTOP.into(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use runtime_vocabulary::glue::TokenTheme as _;

    use super::*;

    #[test]
    fn the_tokens_are_named_under_layout() {
        assert_eq!(LayoutTheme::NAMES, ["layout-gutter-phone", "layout-gutter-tablet", "layout-gutter-desktop"]);
        let t = LayoutTheme::tokens();
        assert_eq!(t.gutter.phone().name(), Some("layout-gutter-phone"));
    }

    /// A sheet resolved before any theme installs still has the
    /// standard gutter: the token's fallback is the default's value.
    #[test]
    fn an_uninstalled_theme_falls_back_to_the_standard_gutter() {
        let t = LayoutTheme::tokens();
        assert_eq!(t.gutter.phone().value(), &Length::Px(GUTTER_PHONE));
        assert_eq!(t.gutter.tablet().value(), &Length::Px(GUTTER_TABLET));
        assert_eq!(t.gutter.desktop().value(), &Length::Px(GUTTER_DESKTOP));
    }
}
