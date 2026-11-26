//! idea-theme's half of the active-theme stash.
//!
//! The generic stash — [`ThemeTokens`], [`install_theme`] / [`set_theme`] /
//! [`install_themes`], [`active_theme`] — is framework infrastructure every
//! component library needs, so it lives in the runtime
//! (`runtime_core::{install_theme, active_theme, …}`) and is re-exported
//! here unchanged. What stays is idea-specific: installing an
//! [`IdeaThemeRef`](crate::theme::IdeaThemeRef) through idea's own derived
//! token set, and reading idea values off the stashed theme.

use std::rc::Rc;

pub use runtime_core::{
    active_theme, active_theme_untracked, install_theme, install_themes, set_theme, theme_installed, ThemeTokens,
    TokenEntry, TokenValue, Tokenized,
};

/// Install an idea theme: stash the carrier as the active theme (what
/// [`active_theme`] returns — component code reads `hover_overlay`,
/// `font_family` … through it) and install its tokens through the derived
/// vocabulary, [`IdeaThemeDefaults::install`], so idea's token names are
/// OWNED by idea's set: a second token set naming one of them panics
/// instead of silently repainting idea-ui.
///
/// [`IdeaThemeDefaults::install`]: runtime_core::Tokens::install
pub(crate) fn install_idea(theme: crate::theme::IdeaThemeRef) {
    let data = crate::theme::IdeaThemeDefaults::from_theme(&theme);
    let tokens = runtime_core::Tokens::token_entries(&data);
    let theme_ext = crate::theme::IdeaTheme::extension_tokens(&theme);
    crate::extensible::register_theme_tones(&theme);
    runtime_core::stash_active_theme(Rc::new(theme));
    <crate::theme::IdeaThemeDefaults as runtime_core::Tokens>::install(data);
    install_extension_tokens(&theme_ext, false);
    runtime_core::apply_host_surface_tokens(&tokens);
}

/// Swap the active idea theme — [`install_idea`]'s swap half, through
/// [`IdeaThemeDefaults::set`](runtime_core::Tokens::set).
pub(crate) fn set_idea(theme: crate::theme::IdeaThemeRef) {
    let data = crate::theme::IdeaThemeDefaults::from_theme(&theme);
    let tokens = runtime_core::Tokens::token_entries(&data);
    let theme_ext = crate::theme::IdeaTheme::extension_tokens(&theme);
    crate::extensible::register_theme_tones(&theme);
    runtime_core::stash_active_theme(Rc::new(theme));
    <crate::theme::IdeaThemeDefaults as runtime_core::Tokens>::set(data);
    install_extension_tokens(&theme_ext, true);
    runtime_core::apply_host_surface_tokens(&tokens);
}

/// Register the theme's own tokens ([`IdeaTheme::extension_tokens`]) beside
/// idea's: the first install creates them, a swap pushes the new values.
/// They are not fields of [`IdeaThemeDefaults`], so its derived install
/// never names them.
///
/// [`IdeaTheme::extension_tokens`]: crate::theme::IdeaTheme::extension_tokens
/// [`IdeaThemeDefaults`]: crate::theme::IdeaThemeDefaults
fn install_extension_tokens(entries: &[TokenEntry], swap: bool) {
    if entries.is_empty() {
        return;
    }
    if swap {
        runtime_core::update_tokens(entries);
    } else {
        runtime_core::install_tokens(entries);
    }
}

/// The active idea-theme's default body [`FontFamily`](runtime_core::FontFamily).
///
/// Exists for sheets that style **portal'd** text — the Select dropdown,
/// the Autocomplete menu, etc. On web a portal mounts under `<body>`,
/// OUTSIDE the app tree's inherited `font-family`, so its text falls back
/// to the browser's serif default unless the menu pins the font itself.
/// Stamping this onto the menu panel keeps the dropdown's font cascade
/// self-contained instead of depending on an ancestor the portal escaped.
///
/// Reads [`active_theme`] (tracked) so a theme swap re-resolves the font
/// in place. Panics if no theme is installed, exactly like
/// [`active_theme`].
pub fn active_font_family() -> runtime_core::FontFamily {
    use crate::theme::IdeaTheme;
    active_theme()
        .downcast_ref::<crate::theme::IdeaThemeRef>()
        .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first")
        .font_family()
}
