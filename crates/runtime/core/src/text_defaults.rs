//! The framework's default-text-color decision, shared by every native
//! backend.
//!
//! An unstyled `text()` node must NOT fall back to the OS system label
//! color (white in dark mode → invisible over a light surface even when
//! the app installed a light theme). Instead it resolves the installed
//! theme's `color-text` token through the *same* [`Tokenized<Color>`]
//! resolution path a styled `color:` token uses, so a theme swap re-fires
//! identically and the resolved bytes match across web/iOS/Android/macOS
//! (CLAUDE.md §7).
//!
//! This decision used to be copy-pasted byte-for-byte into
//! `backend-ios-core` and `backend-android-core` (with the constants
//! re-stated again in `backend-macos`), each carrying a "MUST match the
//! other backends" comment. Hosting it here makes that invariant a single
//! definition instead of a hand-synced one — there is no cross-platform
//! backend crate below the framework that all of iOS/Android/macOS can
//! reach, so the framework core is the canonical home.

use crate::{Color, Tokenized};

/// The installed theme's body-text color token. `idea-theme` installs
/// `color-text` in every variant (`#1a1a1f` light / `#e8eaf0` dark) and
/// `idea-ui`'s `Typography` resolves the same token. Every native backend
/// MUST use this same name so an unstyled `text()` converges on the
/// theme's value rather than the OS system label color.
pub const THEME_TEXT_COLOR_TOKEN: &str = "color-text";

/// Fallback when no theme has installed `color-text` yet. The framework
/// light theme's text color — a near-black legible on the default light
/// surface. CRUCIAL that this is a concrete dark color and NOT a
/// system-appearance color, or the no-theme render goes invisible in dark
/// mode.
pub const THEME_TEXT_COLOR_FALLBACK: &str = "#1a1a1f";

/// The effective text color for a (non-editable) text node, as a
/// `Tokenized<Color>` that still flows through the same `resolve()` path a
/// styled `color:` uses.
///
///   * explicit color present → pass it through unchanged (author wins);
///   * no explicit color → resolve the installed theme's text color via
///     the `color-text` token, never the OS default label color.
///
/// Returning a `Tokenized::Token` (not a pre-resolved `Color`) keeps the
/// caller on the one true resolution path: the backend calls `.resolve()`
/// on the result inside its `apply_style` effect, so a theme swap re-fires
/// exactly as it does for an authored `color:` token.
///
/// Editable widgets (UITextField / EditText / NSTextField) are
/// intentionally NOT routed here — they keep their native default until
/// the author sets a color.
pub fn effective_text_color(explicit: Option<&Tokenized<Color>>) -> Tokenized<Color> {
    match explicit {
        Some(c) => c.clone(),
        None => Tokenized::token(
            THEME_TEXT_COLOR_TOKEN,
            Color(THEME_TEXT_COLOR_FALLBACK.to_string()),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Explicit author color passes straight through — author always wins.
    #[test]
    fn explicit_text_color_wins() {
        let explicit = Tokenized::Literal(Color("#ff0000".into()));
        assert_eq!(effective_text_color(Some(&explicit)), explicit);

        let tok: Tokenized<Color> = Tokenized::token("color-accent", Color("#0af".into()));
        assert_eq!(effective_text_color(Some(&tok)), tok);
    }

    // The regression: no explicit color must yield the THEME's text color
    // token — never an OS/system default — and its no-theme fallback must
    // be a concrete dark color, byte-identical across every backend.
    #[test]
    fn regression_absent_text_color_uses_theme_token_not_os_default() {
        let effective = effective_text_color(None);
        match &effective {
            Tokenized::Token { name, fallback } => {
                assert_eq!(
                    *name, THEME_TEXT_COLOR_TOKEN,
                    "absent text color must resolve through the theme's color-text token",
                );
                assert_eq!(*name, "color-text");
                assert_eq!(fallback.0, THEME_TEXT_COLOR_FALLBACK);
                assert_eq!(fallback.0, "#1a1a1f");
            }
            other => panic!("expected a color-text Token, got {other:?}"),
        }
    }
}
