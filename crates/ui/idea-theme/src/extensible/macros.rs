//! Declarative macros: `tone!` collapses the boilerplate of implementing
//! [`Tone`](super::Tone) on a custom marker type; `color_token!` and
//! `app_theme!` build tokens and app themes.
//!
//! Macros are **sugar**, not the canonical API. The underlying traits
//! remain the source of truth — you can always write the impl by hand,
//! and `cargo expand` shows that each macro produces a plain trait
//! impl. The point of the macros is to make the typical case (a ZST
//! plus a per-slot property map) one short block rather than ~9
//! method bodies.
//!
//! # `tone!`
//!
//! ```ignore
//! use idea_ui::{tone, color_token};
//!
//! tone! {
//!     pub Hype using self, theme {
//!         key = "hype",
//!         fill_bg = color_token!("tone-hype-fill-bg", "#ff00aa"),
//!         fill_fg = color_token!("tone-hype-fill-fg", "#ffffff"),
//!         soft_bg = color_token!("tone-hype-soft-bg", "rgba(255,0,170,0.12)"),
//!         soft_fg = color_token!("tone-hype-soft-fg", "#a30070"),
//!         stroke_color = color_token!("tone-hype-stroke", "#ff00aa"),
//!         stroke_fg = self.soft_fg(theme),
//!         ghost_fg = self.fill_bg(theme),
//!         disabled = theme.colors().text_muted.clone(),
//!         focus_ring = self.fill_bg(theme),
//!     }
//! }
//! ```
//!
//! Each `slot = expr` pair becomes one trait method. The
//! `using self, theme` clause names both the receiver and the
//! `&dyn IdeaTheme` parameter at the macro caller's hygiene scope —
//! this is what makes `self.X(theme)` and `theme.colors()` inside
//! slot expressions actually resolve. Pick any identifiers; the
//! convention is `using self, theme`.
//!
//! (Why both? Rust `macro_rules!` hygiene puts identifiers introduced
//! INSIDE the macro in a different scope than identifiers at the call
//! site. Even `self` is affected — the user's `self.X(theme)` doesn't
//! see the `&self` parameter on the macro-generated method unless the
//! receiver was matched from the caller's tokens. The `using` clause
//! captures both binding sites in the caller's scope.)
//!
//! Required slots — the compiler enforces all are present because
//! each maps to a required trait method. Missing one is a compile
//! error citing the unimplemented `Tone::<slot>`.

/// Construct a `Tokenized<Color>` from a static token name and a hex
/// fallback. Lowers to `Tokenized::token(name, Color(hex.into()))`.
///
/// ```ignore
/// let bg = color_token!("tone-hype-fill-bg", "#ff00aa");
/// ```
///
/// # The `name` argument when overriding a built-in theme color
///
/// When you assign a `color_token!(...)` to a field of [`Colors`] or
/// [`IntentColors`] (rebranding `light_theme()`/`dark_theme()`), the
/// **first argument is irrelevant** — `install_idea_theme` registers each
/// theme field's *value* under that field's fixed CANONICAL token name
/// (`color-surface`, `intent-primary-solid-bg`, …; see
/// [`is_canonical_token`](crate::theme::is_canonical_token)), which is what
/// idea-ui's stylesheets resolve by. So:
///
/// ```ignore
/// let mut t = light_theme();
/// t.colors.surface = color_token!("anything-you-like", "#ffffff"); // WORKS
/// t.intents.primary.solid_bg = color_token!("brand-bg", "#3f73e3"); // WORKS
/// ```
///
/// both take effect — the override is keyed by the field, not by the name
/// you pass. The `name` is only load-bearing for **brand-new
/// custom tokens** (e.g. a `tone!` slot whose value reaches a
/// stylesheet that references the same string) — there it must match the
/// reader exactly.
#[macro_export]
macro_rules! color_token {
    ($name:literal, $hex:literal) => {
        $crate::Tokenized::token(
            $name,
            ::runtime_core::Color($hex.into()),
        )
    };
}

/// Declarative tone definition. Generates a ZST + `impl Tone` filling
/// every slot with the given expression. See the module docs for
/// syntax details.
///
/// Each `slot = expr` is wrapped into a one-line trait method body.
/// `self` and `theme: &dyn IdeaTheme` are bound; expressions can read
/// the theme, call sibling slots via `self.X(theme)`, or evaluate to
/// any `Tokenized<Color>` value.
#[macro_export]
macro_rules! tone {
    (
        $(#[$meta:meta])*
        $vis:vis $name:ident using $me:tt , $theme:ident {
            key = $key:literal,
            fill_bg = $fill_bg:expr,
            fill_fg = $fill_fg:expr,
            soft_bg = $soft_bg:expr,
            soft_fg = $soft_fg:expr,
            stroke_color = $stroke_color:expr,
            stroke_fg = $stroke_fg:expr,
            ghost_fg = $ghost_fg:expr,
            disabled = $disabled:expr,
            focus_ring = $focus_ring:expr
            $(, tokens = [ $($tok_name:literal => $tok_val:literal),* $(,)? ] )?
            $(,)?
        }
    ) => {
        // `IdealystSchema` + `value_of` registers the marker as a
        // completable `tone = …` value in the MCP catalog (a no-op
        // without the `catalog` feature). No `via`: the catalog spells
        // it through the defining module, which by convention is the
        // app's `tone` module.
        $(#[$meta])*
        #[derive(Copy, Clone, Default, ::runtime_core::IdealystSchema)]
        #[schema(value_of = "ToneRef")]
        $vis struct $name;

        $crate::__remote_marker!($crate::extensible::ToneRef, $name);

        impl $crate::extensible::Tone for $name {
            fn key(&$me) -> &'static str {
                let _ = $me;
                $key
            }
            fn fill_bg(
                &$me,
                $theme: &dyn $crate::theme::IdeaTheme,
            ) -> $crate::Tokenized<::runtime_core::Color> {
                let _ = (&$me, $theme);
                $fill_bg
            }
            fn fill_fg(
                &$me,
                $theme: &dyn $crate::theme::IdeaTheme,
            ) -> $crate::Tokenized<::runtime_core::Color> {
                let _ = (&$me, $theme);
                $fill_fg
            }
            fn soft_bg(
                &$me,
                $theme: &dyn $crate::theme::IdeaTheme,
            ) -> $crate::Tokenized<::runtime_core::Color> {
                let _ = (&$me, $theme);
                $soft_bg
            }
            fn soft_fg(
                &$me,
                $theme: &dyn $crate::theme::IdeaTheme,
            ) -> $crate::Tokenized<::runtime_core::Color> {
                let _ = (&$me, $theme);
                $soft_fg
            }
            fn stroke_color(
                &$me,
                $theme: &dyn $crate::theme::IdeaTheme,
            ) -> $crate::Tokenized<::runtime_core::Color> {
                let _ = (&$me, $theme);
                $stroke_color
            }
            fn stroke_fg(
                &$me,
                $theme: &dyn $crate::theme::IdeaTheme,
            ) -> $crate::Tokenized<::runtime_core::Color> {
                let _ = (&$me, $theme);
                $stroke_fg
            }
            fn ghost_fg(
                &$me,
                $theme: &dyn $crate::theme::IdeaTheme,
            ) -> $crate::Tokenized<::runtime_core::Color> {
                let _ = (&$me, $theme);
                $ghost_fg
            }
            fn disabled(
                &$me,
                $theme: &dyn $crate::theme::IdeaTheme,
            ) -> $crate::Tokenized<::runtime_core::Color> {
                let _ = (&$me, $theme);
                $disabled
            }
            fn focus_ring(
                &$me,
                $theme: &dyn $crate::theme::IdeaTheme,
            ) -> $crate::Tokenized<::runtime_core::Color> {
                let _ = (&$me, $theme);
                $focus_ring
            }
        }

        // Reactive-prop coercion: lets a `ui!` call site pass a bare tone
        // marker (`tone = tone::Primary`) to a `#[props]`-wrapped
        // `Reactive<ToneRef>` field — the marker → ref → `Reactive` chain
        // can't go through a single `.into()` (the compiler won't chain two
        // `From` impls).
        impl ::core::convert::From<$name>
            for ::runtime_core::Reactive<$crate::extensible::ToneRef>
        {
            fn from(marker: $name) -> Self {
                ::runtime_core::Reactive::Static(
                    $crate::extensible::ToneRef::from(marker),
                )
            }
        }

        // Optional-tone coercion: `tone = Hype` into an `Option<ToneRef>`
        // prop (`Reactive<Option<ToneRef>>` under `#[props]`) without a
        // `Some(...)`. Orphan-legal in the app's crate because `$name` is
        // local there — see the builtin half in `extensible/tone.rs`.
        impl ::core::convert::From<$name>
            for ::core::option::Option<$crate::extensible::ToneRef>
        {
            fn from(marker: $name) -> Self {
                ::core::option::Option::Some($crate::extensible::ToneRef::from(marker))
            }
        }
        impl ::core::convert::From<$name>
            for ::runtime_core::Reactive<::core::option::Option<$crate::extensible::ToneRef>>
        {
            fn from(marker: $name) -> Self {
                ::runtime_core::Reactive::Static(::core::option::Option::Some(
                    $crate::extensible::ToneRef::from(marker),
                ))
            }
        }

        // Optional inherent `tokens()` — emitted only when the
        // `tokens = [...]` block is present. Returns the token
        // entries this tone introduces, so they can be aggregated by
        // an app theme's `ThemeTokens::tokens()` implementation.
        //
        // ```ignore
        // impl ThemeTokens for MyTheme {
        //     fn tokens(&self) -> Vec<TokenEntry> {
        //         let mut t = IdeaThemeRef::new(self.idea.clone()).tokens();
        //         t.extend(Hype::tokens());  // <- emitted by this macro arm
        //         t
        //     }
        // }
        // ```
        $(
            impl $name {
                /// Color tokens this tone introduces. Aggregate via
                /// `ThemeTokens::tokens()` so they flow into the
                /// runtime token registry on theme install/swap.
                pub fn tokens() -> ::std::vec::Vec<$crate::TokenEntry> {
                    ::std::vec![
                        $(
                            $crate::TokenEntry {
                                name: $tok_name,
                                value: $crate::TokenValue::Color(
                                    ::runtime_core::Color($tok_val.into()),
                                ),
                            }
                        ),*
                    ]
                }
            }
        )?
    };
}

/// Bundle an app theme: a struct that wraps [`IdeaThemeDefaults`] and
/// optionally aggregates custom tokens from extension tones.
///
/// Generates:
///
/// - The struct definition with an `idea: IdeaThemeDefaults` field.
/// - `impl IdeaTheme` — every method delegates to `self.idea`.
/// - `impl ThemeTokens` — concatenates `idea`'s built-in tokens with
///   each listed extension's `tokens()` output.
///
/// ```ignore
/// app_theme! {
///     pub MyTheme {
///         idea: IdeaThemeDefaults,
///         tones: [Hype, Beta],
///     }
/// }
///
/// // Use it like the built-in theme:
/// install_idea_theme(MyTheme { idea: light_theme() });
/// // For dark mode:
/// set_idea_theme(MyTheme { idea: dark_theme() });
/// ```
///
/// (Named `app_theme!` rather than `theme!` to avoid a name collision
/// with the `theme` *module* re-exported at the same path.)
///
/// The `tones: [...]` list is optional. Anything with a static
/// `fn tokens() -> Vec<TokenEntry>` works — most commonly tones built
/// with [`tone!`]'s `tokens = [...]` block.
///
/// **Why this exists.** Hand-writing both `impl IdeaTheme for MyTheme`
/// (5 delegating methods) and `impl ThemeTokens for MyTheme`
/// (extending the idea tokens with each extension's entries) is rote
/// boilerplate. The macro consolidates the contract so adding a new
/// extension tone is a one-line addition to the `tones: [...]` list.
///
/// **Compile-time enforcement.** If an extension named in the list
/// doesn't have a `tokens()` method, the build fails citing the
/// missing function — exactly the slot-completeness guarantee you'd
/// expect.
#[macro_export]
macro_rules! app_theme {
    (
        $vis:vis $name:ident {
            idea: $idea_ty:ty
            $(, tones: [ $($tone:ty),* $(,)? ] )?
            $(,)?
        }
    ) => {
        #[derive(Clone)]
        $vis struct $name {
            pub idea: $idea_ty,
        }

        impl $crate::theme::IdeaTheme for $name {
            fn colors(&self) -> &$crate::theme::Colors {
                <$idea_ty as $crate::theme::IdeaTheme>::colors(&self.idea)
            }
            fn intents(&self) -> &$crate::theme::Intents {
                <$idea_ty as $crate::theme::IdeaTheme>::intents(&self.idea)
            }
            fn palette(&self) -> &$crate::theme::Palette {
                <$idea_ty as $crate::theme::IdeaTheme>::palette(&self.idea)
            }
            fn effects(&self) -> &$crate::theme::Effects {
                <$idea_ty as $crate::theme::IdeaTheme>::effects(&self.idea)
            }
            fn spacing(&self) -> &$crate::theme::Spacing {
                <$idea_ty as $crate::theme::IdeaTheme>::spacing(&self.idea)
            }
            fn radius(&self) -> &$crate::theme::Radius {
                <$idea_ty as $crate::theme::IdeaTheme>::radius(&self.idea)
            }
            fn typography(&self) -> &$crate::theme::Typography {
                <$idea_ty as $crate::theme::IdeaTheme>::typography(&self.idea)
            }
            fn font_family(&self) -> ::runtime_core::FontFamily {
                <$idea_ty as $crate::theme::IdeaTheme>::font_family(&self.idea)
            }
            fn extension_tones(&self) -> ::std::vec::Vec<$crate::extensible::ToneRef> {
                // Installing the theme registers these, so every
                // tone-aware component draws them.
                ::std::vec![$($( $crate::extensible::ToneRef::from(<$tone as ::core::default::Default>::default()), )*)?]
            }
        }

        impl $crate::ThemeTokens for $name {
            fn tokens(&self) -> ::std::vec::Vec<$crate::TokenEntry> {
                // Built-in tokens (Idea defaults: 7 intents × 6 slots
                // + 11 neutrals + spacing + radius + typography sizes).
                let mut tokens = $crate::theme::IdeaThemeRef::new(self.idea.clone()).tokens();
                // Extension tones each contribute their declared
                // tokens. The macro emits one `.extend()` per type
                // listed in `tones: [...]`.
                $(
                    $(
                        tokens.extend(<$tone>::tokens());
                    )*
                )?
                tokens
            }
        }
    };
}
