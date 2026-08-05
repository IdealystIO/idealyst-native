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
//!   resolve through. The trait also carries a default body
//!   [`theme::IdeaTheme::font_family`] — a system-sans stack
//!   ([`theme::DEFAULT_FONT_STACK`]) out of the box so web text isn't
//!   serif; override it (the `font` field on [`theme::IdeaThemeDefaults`])
//!   to ship a brand [`runtime_core::Typeface`].
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
//!   declaration each. [`theme_token!`] / [`theme_length!`] reference a
//!   canonical theme token from a `stylesheet!` by name only (fallback
//!   pulled from the base palette, name checked at compile time) — so a
//!   stylesheet tracks the installed theme without restating any hex.
//! - **Theme runtime** — [`install_theme`], [`set_theme`],
//!   [`install_themes`], plus [`ThemeTokens`] / [`TokenEntry`] /
//!   [`TokenValue`] for theme installation and live swap.

pub mod extensible;
pub mod intent;
mod theme_runtime;

/// In-place signal read-modify-write.
///
/// The kernel's inherent [`Signal::update`] takes `FnOnce(&T) -> T` —
/// it reads the current (staged-aware) value and stages the returned
/// one. `modify` is the `&mut`-closure spelling on top of it, so
/// `sig.modify(|v| v.push(x))` reads naturally for collection-valued
/// signals. Kept in one place so every idea-* crate (and app code built
/// on them) shares a single definition.
///
/// [`Signal::update`]: runtime_core::Signal::update
pub mod compat {
    /// In-place read-modify-write: `sig.modify(|v| v.push(x))`.
    pub trait SignalModify<T> {
        fn modify(&self, f: impl FnOnce(&mut T));
    }

    impl<T: PartialEq + Clone + 'static> SignalModify<T> for runtime_core::Signal<T> {
        fn modify(&self, f: impl FnOnce(&mut T)) {
            // The kernel's `update` reads the CURRENT (staged-aware)
            // value and stages the returned one — the correct
            // read-modify-write primitive under staged commits (a bare
            // `set(get()+…)` would lose earlier same-turn writes).
            self.update(|v| {
                let mut next = v.clone();
                f(&mut next);
                next
            });
        }
    }
}

/// Test-support shared by this crate's unit tests and idea-ui's.
/// Hidden: not part of the theming surface.
#[doc(hidden)]
pub mod testing {
    /// Run a test body inside a reactive context.
    ///
    /// Signals and effects are world-backed, so they need an ambient
    /// world: `f` runs inside a fresh `World` (entered, flushed, and
    /// dropped afterwards).
    pub fn with_test_world<R>(f: impl FnOnce() -> R) -> R {
        runtime_core::__with_fresh_world(f)
    }

    /// Commit staged signal writes mid-test — flushes the innermost
    /// [`with_test_world`] world so a `set` is observable by a following
    /// `get` (writes stage until the world flushes).
    pub fn commit() {
        runtime_core::__flush_test_world();
    }
}

/// Compile-checked usage recipes (docs / MCP catalog). Present only under
/// the `catalog` feature — see [`recipes`].
#[cfg(feature = "catalog")]
pub mod recipes;

pub mod theme;

// Generic theme-as-struct runtime. Re-exported at the crate root so
// callers can reach `install_theme`, `set_theme`, `ThemeTokens`, and
// the token-entry primitives without an extra `theme_runtime::` path.
pub use theme_runtime::{
    active_font_family, active_theme, active_theme_untracked, install_theme, install_themes,
    set_theme, theme_installed, ThemeTokens, TokenEntry, TokenValue, Tokenized,
};

// The opinionated theme + extensibility surface re-exported at root
// for convenience. Authors writing extension code reach these names
// most often — keeping them flat avoids `idea_theme::theme::IdeaTheme`
// pile-ups in user code.
pub use theme::{
    canonical_color, canonical_length, dark_theme, idea_color, idea_header, install_idea_theme,
    is_canonical_color_token, is_canonical_length_token, is_canonical_token, light_theme,
    set_idea_theme, theme_color, theme_length, Colors, IdeaTheme, IdeaThemeDefaults, IdeaThemeRef,
    IntentColors, Intents, Radius, Spacing, Typography, CANONICAL_INTENT_TOKENS,
    CANONICAL_LENGTH_TOKENS, CANONICAL_NEUTRAL_TOKENS, DEFAULT_FONT_STACK, INTENT_NAMES,
    INTENT_SLOTS,
};

// The legacy `Intent` trait + 7 built-in marker types for apps that
// want custom intents (used by the older closed-enum-style components).
pub use intent::{
    Danger, Info, Intent, IntoRcIntent, Neutral, Primary, Secondary, Success, Warning,
};
