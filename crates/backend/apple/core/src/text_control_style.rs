//! Pure style decisions for native editable text controls.
//!
//! The Apple backends materialise an editable input as a native widget —
//! `UITextField` / `UITextView` on iOS, `NSTextField` / `NSTextView` on macOS.
//! Those widgets paint their OWN background + text through the toolkit, using
//! the OS *system* colors (`systemBackground` / `labelColor`, etc.) when the
//! app doesn't override them. Under a dark-mode device those system colors are
//! near-black / white — so a light-theme app's text input renders as a dark box
//! with invisible text (the idea-ui `Textarea`-renders-black field report).
//!
//! The framework is theme-driven, not OS-driven (CLAUDE.md §7): every backend
//! must converge on the *installed theme's* colors, not the device appearance.
//! These two pure decisions encode "what color does an editable control get"
//! once, so iOS and macOS share a single source of truth and resolve
//! byte-identically to web + Android.
//!
//! Both return a `Tokenized<Color>` (never a pre-resolved `Color`) so the
//! caller stays on the one true resolution path: it calls `.resolve()` inside
//! its `apply_style` effect, and a theme swap re-fires exactly as it does for an
//! authored `background:` / `color:` token.

use runtime_core::{Color, Tokenized};

/// Theme token for the surface (input/card) background — the same token
/// idea-theme installs (`#ffffff` light / `#1a1d24` dark) and idea-ui's field
/// input sheet binds for text inputs.
pub const SURFACE_TOKEN: &str = "color-surface";
/// No-theme fallback for [`SURFACE_TOKEN`]: the framework light surface. MUST
/// be a real light color, never a system-appearance fill — the whole point is
/// that a light-theme app's input box stays light under a dark-mode device.
pub const SURFACE_FALLBACK: &str = "#ffffff";

/// Theme token for body/input text color (`#1a1a1f` light / `#e8eaf0` dark).
pub const TEXT_TOKEN: &str = "color-text";
/// No-theme fallback for [`TEXT_TOKEN`]: a near-black legible on the light
/// surface. Never the OS system label color (white in dark mode → invisible).
pub const TEXT_FALLBACK: &str = "#1a1a1f";

/// The effective BACKGROUND for an editable text control.
///
///   * explicit author background → passed through unchanged (author wins —
///     this is the path idea-ui's `text_area`/`text_input` take, setting
///     `color-surface` directly, which is why the explicit value must reach the
///     native widget);
///   * no explicit background → the theme's `color-surface` token.
pub fn effective_input_background(explicit: Option<&Tokenized<Color>>) -> Tokenized<Color> {
    match explicit {
        Some(c) => c.clone(),
        None => Tokenized::token(SURFACE_TOKEN, Color(SURFACE_FALLBACK.to_string())),
    }
}

/// The effective TEXT COLOR for an editable text control. Same shape:
/// explicit author color wins, else the theme's `color-text` token — never the
/// OS system label color.
pub fn effective_input_text_color(explicit: Option<&Tokenized<Color>>) -> Tokenized<Color> {
    match explicit {
        Some(c) => c.clone(),
        None => Tokenized::token(TEXT_TOKEN, Color(TEXT_FALLBACK.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A full UIKit/AppKit test isn't reachable on the host (the native text
    // widgets only link on-device). We test the pure decision that feeds the
    // toolkit `setBackgroundColor:` / `drawsBackground` + `setTextColor:`
    // calls — the bug being that an editable control with no explicit colors
    // rendered the OS system fill/label (a near-black box with white text in
    // dark mode) instead of the installed theme's surface/text.

    #[test]
    fn explicit_background_wins() {
        let surface: Tokenized<Color> =
            Tokenized::token("color-surface", Color("#ffffff".into()));
        assert_eq!(effective_input_background(Some(&surface)), surface);
        let literal = Tokenized::Literal(Color("#fafafa".into()));
        assert_eq!(effective_input_background(Some(&literal)), literal);
    }

    #[test]
    fn regression_absent_background_uses_theme_surface_not_os_default() {
        match effective_input_background(None) {
            Tokenized::Token { name, fallback } => {
                assert_eq!(name, SURFACE_TOKEN);
                assert_eq!(name, "color-surface");
                assert_eq!(fallback.0, SURFACE_FALLBACK);
                assert_eq!(fallback.0, "#ffffff");
            }
            Tokenized::Literal(_) => {
                panic!("absent background must be a token so a theme swap re-fires it");
            }
        }
    }

    #[test]
    fn explicit_text_color_wins() {
        let explicit = Tokenized::Literal(Color("#123456".into()));
        assert_eq!(effective_input_text_color(Some(&explicit)), explicit);
    }

    #[test]
    fn regression_absent_text_color_uses_theme_text_not_os_default() {
        match effective_input_text_color(None) {
            Tokenized::Token { name, fallback } => {
                assert_eq!(name, TEXT_TOKEN);
                assert_eq!(name, "color-text");
                assert_eq!(fallback.0, TEXT_FALLBACK);
                assert_eq!(fallback.0, "#1a1a1f");
            }
            Tokenized::Literal(_) => {
                panic!("absent text color must be a token so a theme swap re-fires it");
            }
        }
    }
}
