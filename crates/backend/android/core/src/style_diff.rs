//! Pure style-decision helpers, factored out of the JNI-only style path
//! (`backend-android-mobile`'s `imp::style`) so they build and unit-test
//! on the host target. The rest of that path is JNI-only and compiles to
//! nothing off-device.
//!
//! Currently holds the "what color should an unstyled text node use"
//! decision, which is regression-prone enough to deserve a host test.

use runtime_core::{Color, Tokenized};

/// The installed theme's text-color token name. Same token idea-ui /
/// idea-theme bind their default text color to (`tok("color-text", …)` in
/// `idea-theme/src/theme.rs`; every `Tokenized::token("color-text", …)` in
/// `idea-ui/src/stylesheets.rs`). Resolving an unstyled `text()` node's
/// color through THIS token (via the exact same `Tokenized<Color>::resolve()`
/// path a styled `color:` token uses) is what makes the native backends
/// converge on the theme's value instead of the OS system label color.
///
/// MUST match the iOS (`backend_ios_core::style_diff::THEME_TEXT_COLOR_TOKEN`)
/// and macOS backends so the resolved value is byte-identical across
/// platforms (CLAUDE.md §7).
pub const THEME_TEXT_COLOR_TOKEN: &str = "color-text";

/// Fallback used when no theme has installed `color-text` yet. Matches
/// idea-theme's light-mode `color-text` default (`#1a1a1f`) so a no-theme
/// render still produces visible dark text on a light surface — rather
/// than Android's default TextView color, which tracks the OS appearance
/// (white in dark mode → invisible on a light pill). Explicit author
/// colors never reach this; see [`effective_text_color`].
pub const THEME_TEXT_COLOR_FALLBACK: &str = "#1a1a1f";

/// The effective text color for a (non-editable) text node, as a
/// `Tokenized<Color>` that still flows through the *same* `resolve()` path
/// a styled `color:` uses.
///
/// Decision (pure, host-testable — the bug this guards is that an
/// unstyled `text()` rendered in Android's default TextView color, which
/// tracks the OS dark/light appearance and is therefore light/white in
/// dark mode and invisible over a light surface even when the app
/// installed a light theme):
///
///   * explicit color present → pass it through unchanged (author wins);
///   * no explicit color → resolve the installed theme's text color via
///     the `color-text` token, NOT the OS default TextView color.
///
/// Returning a `Tokenized::Token` (rather than a pre-resolved `Color`)
/// keeps the caller on the one true resolution path: the JNI style code
/// calls `.resolve()` on the result, so a theme swap re-fires exactly as
/// it does for an authored `color:` token, and the resolved bytes match
/// web + iOS + macOS.
///
/// Editable widgets (`EditText`) are intentionally NOT routed here — they
/// keep their native default until the author sets a color.
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

    // A full JNI test isn't reachable on the host (TextView.setTextColor
    // is a JVM method on a class that only links on-device), so we test
    // the pure decision that feeds the `setTextColor(int)` call. The bug:
    // when `rules.color` is absent, the style path used to leave the
    // TextView at Android's default color — light in dark mode →
    // invisible over a light surface. The fix routes the absent case to
    // the theme's `color-text` token instead.

    // Explicit author color passes straight through — author always wins.
    #[test]
    fn explicit_text_color_wins() {
        let explicit = Tokenized::Literal(Color("#ff0000".into()));
        assert_eq!(effective_text_color(Some(&explicit)), explicit);

        let tok: Tokenized<Color> = Tokenized::token("color-accent", Color("#0af".into()));
        assert_eq!(effective_text_color(Some(&tok)), tok);
    }

    // The regression: no explicit color must yield the THEME's text color
    // token — never an OS/system default. We assert the result is the
    // `color-text` token (so `.resolve()` reads the installed theme), and
    // that its no-theme fallback is a visible dark color, not anything
    // system-appearance-derived.
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
            Tokenized::Literal(_) => {
                panic!("absent text color must be a token, so a theme swap re-fires it");
            }
        }
    }
}
