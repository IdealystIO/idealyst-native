//! Shim — see [`crate::theme`] note. Real impl in `idea-theme`.

#[allow(unused_imports)]
pub use idea_theme::{
    active_theme, active_theme_untracked, install_theme, install_themes, set_theme, ThemeTokens, TokenEntry, TokenValue,
    Tokenized,
};
