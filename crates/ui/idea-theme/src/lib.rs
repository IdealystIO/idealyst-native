//! `idea-theme` — theming abstraction + extensibility for the idealyst
//! design system.
//!
//! This crate is the *abstraction layer*: theme trait surface, modifier
//! traits, declarative macros, and reference defaults (light/dark). It
//! has no knowledge of any particular component — `idea-ui` (and any
//! other component library built on this design system) depends on
//! this crate for theming primitives.
//!
//! # What's here
//!
//! - **Theme trait + data shapes** — [`theme::IdeaTheme`], [`theme::Colors`],
//!   [`theme::Intents`], [`theme::IntentColors`], [`theme::Spacing`],
//!   [`theme::Radius`], [`theme::Typography`]. The data shapes define
//!   *what* a theme provides; the trait is the contract stylesheets
//!   resolve through.
//! - **Reference themes** — [`theme::light_theme`] and [`theme::dark_theme`]
//!   provide opinionated defaults. Apps install one via
//!   [`theme::install_idea_theme`] (or compose them into a custom
//!   theme via the [`theme!`] macro).
//! - **Extensible modifier system** — [`extensible::Tone`],
//!   [`extensible::Variant`], [`extensible::ButtonSize`],
//!   [`extensible::Shape`], [`extensible::TypographyKind`] traits, plus
//!   built-in ZSTs and [`extensible::ResolutionCtx`] for composing
//!   variants against modifiers.
//! - **Macros** — [`tone!`], [`variant!`], [`theme!`], [`color_token!`]
//!   make defining custom modifiers and app themes a one-block
//!   declaration each.
//! - **Theme runtime** — [`install_theme`], [`set_theme`],
//!   [`install_themes`], plus [`ThemeTokens`] / [`TokenEntry`] /
//!   [`TokenValue`] for theme installation and live swap.

pub mod extensible;
pub mod intent;
mod theme_runtime;

pub mod theme;

// Generic theme-as-struct runtime. Re-exported at the crate root so
// callers can reach `install_theme`, `set_theme`, `ThemeTokens`, and
// the token-entry primitives without an extra `theme_runtime::` path.
pub use theme_runtime::{
    active_theme, active_theme_untracked, install_theme, install_themes, set_theme, ThemeTokens, TokenEntry, TokenValue,
    Tokenized,
};

// The opinionated theme + extensibility surface re-exported at root
// for convenience. Authors writing extension code reach these names
// most often — keeping them flat avoids `idea_theme::theme::IdeaTheme`
// pile-ups in user code.
pub use theme::{
    dark_theme, idea_color, idea_header, install_idea_theme, light_theme, set_idea_theme, Colors,
    IdeaTheme, IdeaThemeDefaults, IdeaThemeRef, IntentColors, Intents, Radius, Spacing, Typography,
};

// The legacy `Intent` trait + 7 built-in marker types for apps that
// want custom intents (used by the older closed-enum-style components).
pub use intent::{
    Danger, Info, Intent, IntoRcIntent, Neutral, Primary, Secondary, Success, Warning,
};
