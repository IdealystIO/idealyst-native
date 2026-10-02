//! Built-in tones — semantic palettes that route through the
//! `IdeaTheme.intents()` block. Each tone is a zero-sized marker
//! struct; call sites use the type name as the value.
//!
//! ```ignore
//! use idea_ui::extensible::tone;
//!
//! Button(tone = tone::Primary, ...);
//! Button(tone = tone::Danger, ...);
//! ```
//!
//! Apps add custom tones by implementing [`super::Tone`] on a marker
//! struct of their own. The convention is to place them in a `tone`
//! module within the app that `pub use idea_ui::extensible::tone::*`
//! so call sites uniformly read `tone::Name` regardless of origin.

use runtime_core::{Color, IdealystSchema, Tokenized};

use super::Tone;
use crate::theme::IdeaTheme;

// Helper — pulls one slot off an `IntentColors` block. The seven
// built-ins differ only in which IntentColors block they target.
macro_rules! builtin_tone {
    ($name:ident, $key:literal, $block:ident) => {
        /// Built-in semantic tone.
        #[derive(Copy, Clone, Default, IdealystSchema)]
        #[schema(value_of = "ToneRef", via = "idea_ui::tone")]
        pub struct $name;

        crate::__remote_marker!(super::ToneRef, $name);

        impl Tone for $name {
            fn key(&self) -> &'static str {
                $key
            }
            fn fill_bg(&self, theme: &dyn IdeaTheme) -> Tokenized<Color> {
                theme.intents().$block.solid_bg.clone()
            }
            fn fill_fg(&self, theme: &dyn IdeaTheme) -> Tokenized<Color> {
                theme.intents().$block.solid_text.clone()
            }
            fn soft_bg(&self, theme: &dyn IdeaTheme) -> Tokenized<Color> {
                theme.intents().$block.soft_bg.clone()
            }
            fn soft_fg(&self, theme: &dyn IdeaTheme) -> Tokenized<Color> {
                theme.intents().$block.soft_text.clone()
            }
            fn stroke_color(&self, theme: &dyn IdeaTheme) -> Tokenized<Color> {
                theme.intents().$block.border.clone()
            }
            fn stroke_fg(&self, theme: &dyn IdeaTheme) -> Tokenized<Color> {
                theme.intents().$block.fg.clone()
            }
            fn ghost_fg(&self, theme: &dyn IdeaTheme) -> Tokenized<Color> {
                theme.intents().$block.fg.clone()
            }
            fn disabled(&self, theme: &dyn IdeaTheme) -> Tokenized<Color> {
                theme.colors().text_muted.clone()
            }
            fn focus_ring(&self, theme: &dyn IdeaTheme) -> Tokenized<Color> {
                theme.colors().focus_ring.clone()
            }
        }

        // Reactive-prop coercion: `tone = tone::Primary` into a
        // `#[props]`-wrapped `Reactive<ToneRef>` field. The marker → ref →
        // `Reactive` chain can't go through one `.into()`. See the matching
        // note in `extensible/typography.rs::builtin_kind!`.
        impl ::core::convert::From<$name> for ::runtime_core::Reactive<super::ToneRef> {
            fn from(marker: $name) -> Self {
                ::runtime_core::Reactive::Static(super::ToneRef::from(marker))
            }
        }

        // Optional-tone coercion: `tone = tone::Danger` into an
        // `Option<ToneRef>` prop (`#[props]` wraps it as
        // `Reactive<Option<ToneRef>>`) with no `Some(...)`. Orphan-legal
        // because the marker — the trait's type parameter — is local: only
        // a BLANKET `impl<T: Tone> From<T> for Option<ToneRef>` is barred.
        // See the note after `ToneRef::builtins` in `extensible/mod.rs`.
        impl ::core::convert::From<$name> for ::core::option::Option<super::ToneRef> {
            fn from(marker: $name) -> Self {
                ::core::option::Option::Some(super::ToneRef::from(marker))
            }
        }
        impl ::core::convert::From<$name>
            for ::runtime_core::Reactive<::core::option::Option<super::ToneRef>>
        {
            fn from(marker: $name) -> Self {
                ::runtime_core::Reactive::Static(::core::option::Option::Some(
                    super::ToneRef::from(marker),
                ))
            }
        }
    };
}

builtin_tone!(Primary, "primary", primary);
builtin_tone!(Secondary, "secondary", secondary);
builtin_tone!(Neutral, "neutral", neutral);
builtin_tone!(Success, "success", success);
builtin_tone!(Danger, "danger", danger);
builtin_tone!(Warning, "warning", warning);
builtin_tone!(Info, "info", info);
