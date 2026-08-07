//! [`IdeaTokens`] — idea-ui's token vocabulary as a *type*.
//!
//! This is how a stylesheet names a theme token. Instead of
//!
//! ```ignore
//! padding: Tokenized::token("spacing-md", Length::Px(12.0)),
//! ```
//!
//! a sheet that declares `<IdeaThemeRef>` (or `<IdeaTokens>`) writes
//!
//! ```ignore
//! padding: t.spacing.md(),
//! ```
//!
//! where `t` is the binding in the block header — `base(t) { … }` — which
//! [`runtime_core::TokenVocabulary`] materializes for the declared type.
//!
//! # What changed, and what deliberately didn't
//!
//! Only the *reference* changed. Each accessor returns the same
//! `Tokenized::Token { name, fallback }` the string form built, so
//! resolution, premint, class hashing, and theme swap are byte-for-byte
//! what they were: the installed theme still owns every value, and
//! `update_tokens` still swaps a theme with one write per token. What's
//! gone is the chance to write a name nothing resolves —
//! `t.typography.size_md()` doesn't compile, while
//! `Tokenized::token("typography-size-md", …)` compiled fine and rendered
//! its fallback forever (two sheets in this repo were doing exactly that).
//!
//! # The invariant
//!
//! **The accessor path is the token name.** `t.color.surface_alt()` →
//! `color-surface-alt`, `t.intent.primary.solid_bg()` →
//! `intent-primary-solid-bg`, `t.spacing.md()` → `spacing-md`. Two
//! properties keep that honest, both enforced by tests in this module:
//!
//! 1. Each accessor's *fallback* is read from the field the macro derives
//!    from the accessor's own name (`surface_alt()` reads
//!    `light.colors.surface_alt`) — so an accessor cannot be paired with
//!    the wrong palette entry, and the fallback can't drift from
//!    [`light_theme`] the way hand-restated hex values did.
//! 2. The set of names this vocabulary can emit is asserted equal, in both
//!    directions, to the set [`ThemeTokens::tokens`] installs. A token you
//!    can install but not reference — or reference but not install — fails
//!    the build.
//!
//! [`ThemeTokens::tokens`]: crate::theme_runtime::ThemeTokens::tokens

use runtime_core::{Color, Length, TokenVocabulary, Tokenized};

use crate::theme::{light_theme, IdeaThemeDefaults, IdeaThemeRef};

thread_local! {
    /// The base palette every accessor's fallback comes from, built once
    /// per thread.
    ///
    /// A fallback is only ever observed *before* a theme installs (after
    /// that the registry value wins in `Tokenized::resolve`), so which
    /// palette it comes from is a cosmetic pre-install detail — but it
    /// must come from a real palette rather than a restated literal, or
    /// the two drift. Reading `light_theme()` keeps the accessor honest
    /// by construction and matches what the retired `theme_token!` macro
    /// did.
    ///
    /// Built once because the alternative — the old
    /// `canonical_color(name)` path — reconstructed the whole theme and
    /// linear-scanned ~70 entries on *every* property, and idea-ui alone
    /// references 600+.
    static BASE: IdeaThemeDefaults = light_theme();
}

/// Render a length for catalog display (`12px`). Only ever sees the
/// base palette's values, which are all `Px`; the other arms exist so a
/// vocabulary that grows percent/auto tokens still reads correctly.
///
/// Deliberately NOT `#[cfg(feature = "catalog")]`: the registration it
/// serves is gated on `runtime-core/catalog`, a different feature, so a
/// cfg here would strip the helper out from under a live call site (it
/// did). Unused without the catalog — the optimizer drops it.
#[allow(dead_code)]
fn length_display(l: Length) -> String {
    match l {
        Length::Px(v) => format!("{v}px"),
        Length::Percent(v) => format!("{v}%"),
        Length::Auto => "auto".to_string(),
    }
}

// =============================================================================
// Namespace generators
// =============================================================================

/// Generate a color-token namespace. Each entry is
/// `accessor => "token-name"`; the fallback is always read from
/// `light.$group.$accessor`, so the accessor and the palette field it
/// reports are the same identifier by construction.
macro_rules! color_namespace {
    (
        $(#[$ty_meta:meta])*
        $ty:ident as $ns:literal on $group:ident {
            $( $accessor:ident => $token:literal ),+ $(,)?
        }
    ) => {
        $(#[$ty_meta])*
        #[derive(Default, Clone, Copy, Debug)]
        pub struct $ty;

        impl $ty {
            /// Every token name this namespace can emit, in declaration
            /// order. Drives the round-trip tests; not needed to author a
            /// stylesheet.
            pub const NAMES: &'static [&'static str] = &[$($token),+];

            $(
                #[doc = concat!("Theme token `", $token, "`.")]
                pub fn $accessor(&self) -> Tokenized<Color> {
                    BASE.with(|b| {
                        Tokenized::token($token, b.$group.$accessor.value().clone())
                    })
                }
            )+
        }

        $(
            catalog_token!($token, concat!($ns, ".", stringify!($accessor)), $ns, "Color", {
                $ty.$accessor().value().0.clone()
            });
        )+
    };
}

/// Generate a length-token namespace (spacing / radius / typography size).
/// Same accessor-is-the-field rule as [`color_namespace`]; the palette
/// stores these as bare `f32` px, so the accessor wraps in [`Length::Px`].
macro_rules! length_namespace {
    (
        $(#[$ty_meta:meta])*
        $ty:ident as $ns:literal on $group:ident {
            $( $accessor:ident => $token:literal ),+ $(,)?
        }
    ) => {
        $(#[$ty_meta])*
        #[derive(Default, Clone, Copy, Debug)]
        pub struct $ty;

        impl $ty {
            /// Every token name this namespace can emit, in declaration order.
            pub const NAMES: &'static [&'static str] = &[$($token),+];

            $(
                #[doc = concat!("Theme token `", $token, "`.")]
                pub fn $accessor(&self) -> Tokenized<Length> {
                    BASE.with(|b| {
                        Tokenized::token($token, Length::Px(b.$group.$accessor))
                    })
                }
            )+
        }

        $(
            catalog_token!($token, concat!($ns, ".", stringify!($accessor)), $ns, "Length", {
                length_display(*$ty.$accessor().value())
            });
        )+
    };
}

/// One intent slot accessor. Split out of `intent_namespaces!` because
/// the slot's *token* spelling (`solid-bg`) and its *field* spelling
/// (`solid_bg`) differ, so both have to be passed; the fallback still
/// comes from the field, never a literal.
macro_rules! intent_slot {
    ($intent_field:ident, $intent:literal, $slot_field:ident, $slot:literal) => {
        #[doc = concat!("Theme token `intent-", $intent, "-", $slot, "`.")]
        pub fn $slot_field(&self) -> Tokenized<Color> {
            BASE.with(|b| {
                Tokenized::token(
                    concat!("intent-", $intent, "-", $slot),
                    b.intents.$intent_field.$slot_field.value().clone(),
                )
            })
        }
    };
}

/// Register one token into the MCP catalog, so tooling gets the
/// vocabulary as data (`list_tokens`, and the editor completion that
/// offers `t.spacing.│`).
///
/// Rides the same declaration as the accessor: a token cannot be added
/// to the vocabulary without appearing in the catalog, and the displayed
/// default is produced by CALLING the accessor, so it can't drift from
/// the fallback the accessor actually emits.
///
/// Delegates to `runtime_core::register_style_token!`, whose catalog
/// gate follows `runtime-core/catalog` — a `#[cfg(feature = "catalog")]`
/// written HERE would see only idea-theme's own feature, which the
/// catalog wrapper never turns on, so the entries would silently never
/// register.
macro_rules! catalog_token {
    ($name:expr, $path:expr, $ns:expr, $ty:literal, $default:block) => {
        ::runtime_core::register_style_token!($name, $path, $ns, $ty, "IdeaThemeRef", $default);
    };
}

/// Generate one namespace per intent, each carrying the six canonical
/// slots. The slot accessors are fixed (they're the `IntentColors`
/// fields), so only the intent name varies — and the token name is
/// assembled from it at compile time via `concat!`, which is what keeps
/// `t.intent.primary.solid_bg()` and `intent-primary-solid-bg` the same
/// fact.
macro_rules! intent_namespaces {
    ( $( $ty:ident => $intent_field:ident, $intent:literal );+ $(;)? ) => {
        $(
            #[doc = concat!("The `", $intent, "` intent's six color slots.")]
            #[derive(Default, Clone, Copy, Debug)]
            pub struct $ty;

            impl $ty {
                /// Every token name this namespace can emit, in slot order.
                pub const NAMES: &'static [&'static str] = &[
                    concat!("intent-", $intent, "-solid-bg"),
                    concat!("intent-", $intent, "-solid-text"),
                    concat!("intent-", $intent, "-soft-bg"),
                    concat!("intent-", $intent, "-soft-text"),
                    concat!("intent-", $intent, "-fg"),
                    concat!("intent-", $intent, "-border"),
                ];

                intent_slot!($intent_field, $intent, solid_bg, "solid-bg");
                intent_slot!($intent_field, $intent, solid_text, "solid-text");
                intent_slot!($intent_field, $intent, soft_bg, "soft-bg");
                intent_slot!($intent_field, $intent, soft_text, "soft-text");
                intent_slot!($intent_field, $intent, fg, "fg");
                intent_slot!($intent_field, $intent, border, "border");
            }

            catalog_intent_slot!($ty, $intent, solid_bg, "solid-bg");
            catalog_intent_slot!($ty, $intent, solid_text, "solid-text");
            catalog_intent_slot!($ty, $intent, soft_bg, "soft-bg");
            catalog_intent_slot!($ty, $intent, soft_text, "soft-text");
            catalog_intent_slot!($ty, $intent, fg, "fg");
            catalog_intent_slot!($ty, $intent, border, "border");
        )+
    };
}

/// Catalog registration for one intent slot — peer of `catalog_token!`,
/// split out because the intent path has three segments.
macro_rules! catalog_intent_slot {
    ($ty:ident, $intent:literal, $slot_field:ident, $slot:literal) => {
        catalog_token!(
            concat!("intent-", $intent, "-", $slot),
            concat!("intent.", $intent, ".", stringify!($slot_field)),
            "intent",
            "Color",
            { $ty.$slot_field().value().0.clone() }
        );
    };
}

// =============================================================================
// The vocabulary
// =============================================================================

color_namespace! {
    /// Non-intent neutrals — page background, surfaces, text, borders,
    /// focus ring, overlay.
    ColorTokens as "color" on colors {
        background => "color-background",
        surface => "color-surface",
        surface_alt => "color-surface-alt",
        text => "color-text",
        text_muted => "color-text-muted",
        text_inverse => "color-text-inverse",
        border => "color-border",
        border_hover => "color-border-hover",
        border_strong => "color-border-strong",
        focus_ring => "color-focus-ring",
        overlay => "color-overlay",
    }
}

length_namespace! {
    /// The spacing scale — padding, margin, and gap steps.
    SpacingTokens as "spacing" on spacing {
        xs => "spacing-xs",
        sm => "spacing-sm",
        md => "spacing-md",
        lg => "spacing-lg",
        xl => "spacing-xl",
        xxl => "spacing-xxl",
    }
}

length_namespace! {
    /// The corner-radius scale.
    RadiusTokens as "radius" on radius {
        sm => "radius-sm",
        md => "radius-md",
        lg => "radius-lg",
        pill => "radius-pill",
    }
}

length_namespace! {
    /// Per-variant font sizes. One entry per `Typography` kind — the only
    /// type property pulled up to the theme (see [`crate::theme::Typography`]
    /// for why weight/line-height aren't).
    TypographyTokens as "typography" on typography {
        display_size => "typography-display-size",
        h1_size => "typography-h1-size",
        h2_size => "typography-h2-size",
        h3_size => "typography-h3-size",
        body_xl_size => "typography-body-xl-size",
        body_lg_size => "typography-body-lg-size",
        body_size => "typography-body-size",
        body_sm_size => "typography-body-sm-size",
        caption_size => "typography-caption-size",
        overline_size => "typography-overline-size",
    }
}

intent_namespaces! {
    PrimaryTokens => primary, "primary";
    SecondaryTokens => secondary, "secondary";
    NeutralTokens => neutral, "neutral";
    SuccessTokens => success, "success";
    DangerTokens => danger, "danger";
    WarningTokens => warning, "warning";
    InfoTokens => info, "info";
}

/// The seven intent palettes, each reached as `t.intent.<name>.<slot>()`.
#[derive(Default, Clone, Copy, Debug)]
pub struct IntentTokens {
    pub primary: PrimaryTokens,
    pub secondary: SecondaryTokens,
    pub neutral: NeutralTokens,
    pub success: SuccessTokens,
    pub danger: DangerTokens,
    pub warning: WarningTokens,
    pub info: InfoTokens,
}

/// idea-ui's token vocabulary — the root namespace bound as `t` inside a
/// `stylesheet!` block. See the [module docs](self) for the accessor-path-
/// is-the-token-name invariant.
///
/// ```
/// let t = idea_theme::IdeaTokens::default();
/// assert_eq!(t.spacing.md().name(), Some("spacing-md"));
/// assert_eq!(t.color.surface_alt().name(), Some("color-surface-alt"));
/// assert_eq!(t.intent.primary.solid_bg().name(), Some("intent-primary-solid-bg"));
/// ```
///
/// A token that doesn't exist is a compile error rather than a reference
/// that resolves to nothing. `"typography-size-md"` — a plausible-looking
/// name two sheets in this repo actually shipped, silently rendering the
/// fallback forever — has no accessor:
///
/// ```compile_fail
/// let t = idea_theme::IdeaTokens::default();
/// let _ = t.typography.size_md(); // no such token → does not compile
/// ```
#[derive(Default, Clone, Copy, Debug)]
pub struct IdeaTokens {
    pub color: ColorTokens,
    pub intent: IntentTokens,
    pub spacing: SpacingTokens,
    pub radius: RadiusTokens,
    pub typography: TypographyTokens,
}

/// The vocabulary, for code that isn't inside a `stylesheet!` block.
///
/// Component code that builds a `StyleRules` by hand (computed layers,
/// per-instance overrides) has no block binding to reach for, but needs
/// the same typed reference: `tokens().color.text_muted()` rather than
/// `Tokenized::token("color-text-muted", Color("#6b7280".into()))` — a
/// spelling that was not only stringly-typed but carried a restated hex
/// free to drift from the palette (several in idea-ui had).
///
/// ```
/// use idea_theme::tokens;
/// assert_eq!(tokens().radius.lg().name(), Some("radius-lg"));
/// ```
pub fn tokens() -> IdeaTokens {
    IdeaTokens::default()
}

/// A sheet may declare the vocabulary directly (`<IdeaTokens>`).
impl TokenVocabulary for IdeaTokens {
    type Tokens = Self;
}

/// …or name the theme carrier it belongs to (`<IdeaThemeRef>`), which is
/// how every sheet in this repo spells it. Both resolve to the same
/// namespace: a theme *is* an assignment of values to this vocabulary, so
/// naming either one selects the same token names.
impl TokenVocabulary for IdeaThemeRef {
    type Tokens = IdeaTokens;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme_runtime::{ThemeTokens, TokenValue};
    use std::collections::BTreeMap;

    /// Every color accessor in the vocabulary, paired with nothing —
    /// the *name* is what the accessor itself reports, which is the
    /// point of the tests below. The one place all 53 are enumerated by
    /// hand; `NAMES` (macro-generated) is checked against it, so an
    /// accessor added without a name — or vice versa — fails here.
    fn all_colors(t: &IdeaTokens) -> Vec<Tokenized<Color>> {
        let i = &t.intent;
        let mut out = vec![
            t.color.background(),
            t.color.surface(),
            t.color.surface_alt(),
            t.color.text(),
            t.color.text_muted(),
            t.color.text_inverse(),
            t.color.border(),
            t.color.border_hover(),
            t.color.border_strong(),
            t.color.focus_ring(),
            t.color.overlay(),
        ];
        macro_rules! slots {
            ($($ns:expr),+ $(,)?) => {
                $(out.extend([
                    $ns.solid_bg(), $ns.solid_text(), $ns.soft_bg(),
                    $ns.soft_text(), $ns.fg(), $ns.border(),
                ]);)+
            };
        }
        slots!(i.primary, i.secondary, i.neutral, i.success, i.danger, i.warning, i.info);
        out
    }

    /// Every length accessor. Peer of [`all_colors`].
    fn all_lengths(t: &IdeaTokens) -> Vec<Tokenized<Length>> {
        vec![
            t.spacing.xs(),
            t.spacing.sm(),
            t.spacing.md(),
            t.spacing.lg(),
            t.spacing.xl(),
            t.spacing.xxl(),
            t.radius.sm(),
            t.radius.md(),
            t.radius.lg(),
            t.radius.pill(),
            t.typography.display_size(),
            t.typography.h1_size(),
            t.typography.h2_size(),
            t.typography.h3_size(),
            t.typography.body_xl_size(),
            t.typography.body_lg_size(),
            t.typography.body_size(),
            t.typography.body_sm_size(),
            t.typography.caption_size(),
            t.typography.overline_size(),
        ]
    }

    fn declared_names() -> Vec<&'static str> {
        let i = IntentTokens::default();
        let _ = i;
        let mut names: Vec<&'static str> = ColorTokens::NAMES.to_vec();
        for ns in [
            PrimaryTokens::NAMES,
            SecondaryTokens::NAMES,
            NeutralTokens::NAMES,
            SuccessTokens::NAMES,
            DangerTokens::NAMES,
            WarningTokens::NAMES,
            InfoTokens::NAMES,
        ] {
            names.extend(ns);
        }
        names.extend(SpacingTokens::NAMES);
        names.extend(RadiusTokens::NAMES);
        names.extend(TypographyTokens::NAMES);
        names
    }

    /// Each accessor reports the name its declaration promised, in
    /// declaration order. This is what pins `t.color.surface_alt()` to
    /// `color-surface-alt` — without it an accessor could quietly return
    /// a neighbouring token's name.
    #[test]
    fn accessor_reports_its_declared_name() {
        let t = IdeaTokens::default();
        let reported: Vec<&'static str> = all_colors(&t)
            .iter()
            .map(|v| v.name().expect("accessor must emit a token reference, not a literal"))
            .chain(
                all_lengths(&t)
                    .iter()
                    .map(|v| v.name().expect("accessor must emit a token reference")),
            )
            .collect();
        assert_eq!(reported, declared_names());
    }

    /// The vocabulary and the install path describe the same token set.
    /// A name you can reference but never install would resolve to its
    /// fallback forever (the `typography-size-md` bug); a name installed
    /// but unreferenceable is dead weight no sheet can reach.
    #[test]
    fn vocabulary_and_install_path_agree() {
        let installed: Vec<&'static str> =
            IdeaThemeRef::new(light_theme()).tokens().iter().map(|e| e.name).collect();
        let mut declared = declared_names();
        let mut installed_sorted = installed.clone();
        declared.sort_unstable();
        installed_sorted.sort_unstable();

        let missing: Vec<_> =
            installed_sorted.iter().filter(|n| !declared.contains(n)).collect();
        assert!(missing.is_empty(), "installed but not referenceable from IdeaTokens: {missing:?}");
        let unreachable: Vec<_> =
            declared.iter().filter(|n| !installed_sorted.contains(n)).collect();
        assert!(
            unreachable.is_empty(),
            "referenceable from IdeaTokens but never installed: {unreachable:?}"
        );
    }

    /// Every accessor's fallback equals the value the base palette
    /// installs for that same name. This is the property the old
    /// hand-restated fallbacks broke (`intent-primary-solid-bg` carried
    /// `#5b6cff` in idea-ui's sheets while the palette said `#4f46e5`),
    /// so it's asserted rather than assumed.
    #[test]
    fn accessor_fallback_matches_the_base_palette() {
        let installed: BTreeMap<&'static str, TokenValue> = IdeaThemeRef::new(light_theme())
            .tokens()
            .into_iter()
            .map(|e| (e.name, e.value))
            .collect();
        let t = IdeaTokens::default();

        for tok in all_colors(&t) {
            let name = tok.name().unwrap();
            match installed.get(name) {
                Some(TokenValue::Color(c)) => assert_eq!(
                    tok.value(),
                    c,
                    "fallback for `{name}` drifted from the base palette"
                ),
                other => panic!("`{name}` installs as {other:?}, expected a Color"),
            }
        }
        for tok in all_lengths(&t) {
            let name = tok.name().unwrap();
            match installed.get(name) {
                Some(TokenValue::Length(l)) => assert_eq!(
                    tok.value(),
                    l,
                    "fallback for `{name}` drifted from the base palette"
                ),
                other => panic!("`{name}` installs as {other:?}, expected a Length"),
            }
        }
    }

    /// The MCP catalog slice describes exactly this vocabulary — same
    /// names, and every entry's `path` actually resolves to its `name`.
    ///
    /// This is the drift guard for the tooling surface: the editor
    /// completion and `list_tokens` both read the catalog, so an entry
    /// naming a token the vocabulary doesn't have would offer code that
    /// doesn't compile, and a missing entry would hide a real token.
    #[cfg(feature = "catalog")]
    #[test]
    fn catalog_slice_matches_the_vocabulary() {
        let entries: Vec<_> = runtime_core::__mcp::style_tokens().collect();
        let mut catalog_names: Vec<&str> = entries.iter().map(|e| e.name).collect();
        catalog_names.sort_unstable();
        let mut declared = declared_names();
        declared.sort_unstable();
        assert_eq!(
            catalog_names, declared,
            "the catalog slice and the vocabulary must describe the same tokens"
        );

        // Every entry's accessor path must be the token's own name — the
        // invariant the completion depends on when it inserts `path`.
        for e in &entries {
            let from_path = format!(
                "{}{}",
                if e.namespace == "intent" { "intent-" } else { "" },
                e.path
                    .trim_start_matches("intent.")
                    .replace('.', "-")
                    .replace('_', "-")
            );
            assert_eq!(
                from_path, e.name,
                "accessor path `{}` must spell token name `{}`",
                e.path, e.name
            );
            assert!(!e.default_value.get().is_empty(), "`{}` must render a base value", e.name);
        }
    }

    /// The vocabulary agrees with the `CANONICAL_*` lists that back the
    /// public `is_canonical_*` predicates, so the typed path and the
    /// string path (`theme_color`, for computed names) can't disagree
    /// about which names exist.
    #[test]
    fn vocabulary_matches_canonical_token_lists() {
        use crate::theme::{
            CANONICAL_INTENT_TOKENS, CANONICAL_LENGTH_TOKENS, CANONICAL_NEUTRAL_TOKENS,
        };
        let mut canonical: Vec<&str> = CANONICAL_NEUTRAL_TOKENS.to_vec();
        canonical.extend(CANONICAL_INTENT_TOKENS);
        canonical.extend(CANONICAL_LENGTH_TOKENS);
        canonical.sort_unstable();

        let mut declared = declared_names();
        declared.sort_unstable();
        assert_eq!(declared, canonical);
    }
}
