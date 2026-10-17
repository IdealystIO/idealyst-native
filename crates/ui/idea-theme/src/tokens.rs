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
//! # Where it comes from
//!
//! [`IdeaThemeDefaults`] is a `#[derive(Tokens)]` theme, and this
//! vocabulary is its generated accessor tree — the names below are aliases
//! of the derived types under idea's established names. So **the accessor
//! path is the token name** by construction: `t.color.surface_alt()` →
//! `color-surface-alt`, `t.intent.primary.solid_bg()` →
//! `intent-primary-solid-bg`, `t.spacing.md()` → `spacing-md`,
//! `t.radius.pill()` → the computed `radius-pill`. The same derived name
//! table is what [`ThemeTokens::tokens`] installs, so a token cannot be
//! referenceable but never installed (or the reverse), and each accessor's
//! fallback is read from [`light_theme`] (the theme's
//! `#[tokens(default = …)]`), never restated. The tests in this module pin
//! the names byte-for-byte against idea's canonical lists, in install order.
//!
//! [`ThemeTokens::tokens`]: crate::theme_runtime::ThemeTokens::tokens
//! [`IdeaThemeDefaults`]: crate::IdeaThemeDefaults

use std::sync::OnceLock;

use runtime_core::{TokenTheme, TokenValue, TokenVocabulary, Tokens};

use crate::theme::{light_theme, IdeaThemeDefaults, IdeaThemeRef};

/// The theme the vocabulary's names are indexed against.
type Root = IdeaThemeDefaults;

// =============================================================================
// The vocabulary as data
// =============================================================================

/// The payload a token carries. Decides which control a generated
/// editor renders for it, and which style properties it is valid on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Color,
    Length,
}

/// One token of the vocabulary, described as data — the ungated
/// sibling of the catalog's `StyleTokenEntry`.
///
/// The catalog slice carries richer metadata but rides
/// `runtime-core/catalog`, which is off in every production build and
/// pulls the whole component inventory with it. A theme EDITOR shipped
/// inside an app needs the vocabulary at runtime without paying for
/// that, and it needs one thing the catalog doesn't carry:
/// [`field_path`](Self::field_path), the `IdeaThemeDefaults` path to
/// assign when generating source.
///
/// Built once from the derived name and path tables, so a descriptor
/// cannot drift from the accessor it describes.
#[derive(Clone, Copy, Debug)]
pub struct TokenDescriptor {
    /// The registry key — `color-table-header`. What
    /// `runtime_core::token_value` takes and `update_tokens` writes.
    pub name: &'static str,
    /// The accessor path inside a `stylesheet!` block, without the
    /// binding — `color.table_header`, `intent.primary.solid_bg`.
    pub path: &'static str,
    /// First path segment — `color`, `intent`, `palette`, `effect`,
    /// `spacing`, `radius`, `typography`. The grouping an editor lays out by.
    pub namespace: &'static str,
    /// The field path on [`IdeaThemeDefaults`](crate::IdeaThemeDefaults)
    /// this token reads from — `colors.table_header`,
    /// `intents.primary.solid_bg`, `spacing.md`. Prepend a theme
    /// binding and it is an assignable place expression, which is what
    /// makes "export this theme as Rust" a mechanical walk.
    ///
    /// `None` for a token with no field behind it: `radius-pill` is
    /// the only one (a computed token, always [`Length::Full`] — not a
    /// number a theme can pick). Codegen must skip those rather than emit
    /// an assignment to a field that does not exist.
    ///
    /// [`Length::Full`]: runtime_core::Length::Full
    pub field_path: Option<&'static str>,
    /// Which `Tokenized<T>` the accessor returns.
    pub kind: TokenKind,
}

/// The struct field a vocabulary namespace reads: the three groups
/// `#[tokens(rename = …)]`d to the singular on [`IdeaThemeDefaults`].
fn group_field(namespace: &str) -> &str {
    match namespace {
        "color" => "colors",
        "intent" => "intents",
        "effect" => "effects",
        other => other,
    }
}

/// The computed tokens — no struct field behind them.
const COMPUTED: &[&str] = &["radius-pill"];

/// Every token in the idea vocabulary, described as data — the whole
/// palette an editor can offer, in install order (neutrals, the seven
/// intents, the twelve palette hues, effects, spacing, radius,
/// typography).
///
/// Ungated, unlike the catalog slice: this is the enumeration a theme
/// editor shipped inside an app walks to generate one control per
/// token. It is the STATIC vocabulary, so it lists exactly what has an
/// accessor — pair it with `runtime_core::token_names()` to also reach
/// extension tokens (a `tone!`'s `tokens = [...]` block), which reach
/// the live world but have no accessor to be described by.
///
/// ```
/// use idea_theme::{token_descriptors, TokenKind};
///
/// let d = token_descriptors().find(|d| d.name == "color-table-header").unwrap();
/// assert_eq!(d.path, "color.table_header");
/// assert_eq!(d.field_path, Some("colors.table_header"));
/// assert_eq!(d.kind, TokenKind::Color);
/// ```
pub fn token_descriptors() -> impl Iterator<Item = &'static TokenDescriptor> {
    static DESCRIPTORS: OnceLock<Vec<TokenDescriptor>> = OnceLock::new();
    DESCRIPTORS
        .get_or_init(|| {
            let values = light_theme().token_entries();
            Root::NAMES
                .iter()
                .zip(Root::PATHS)
                .zip(values)
                .map(|((name, path), entry)| {
                    let namespace = path.split('.').next().unwrap_or(path);
                    let field_path = (!COMPUTED.contains(name)).then(|| {
                        let rest = &path[namespace.len()..];
                        &*Box::leak(format!("{}{rest}", group_field(namespace)).into_boxed_str())
                    });
                    let kind = match entry.value {
                        TokenValue::Color(_) => TokenKind::Color,
                        TokenValue::Length(_) => TokenKind::Length,
                        TokenValue::Number(_) => unreachable!("idea's vocabulary has no number tokens"),
                    };
                    TokenDescriptor { name, path, namespace, field_path, kind }
                })
                .collect()
        })
        .iter()
}

// =============================================================================
// The vocabulary — idea's names for the derived accessor types
// =============================================================================

/// idea-ui's token vocabulary — the root namespace bound as `t` inside a
/// `stylesheet!` block declared `<IdeaThemeRef>` (or `<IdeaTokens>`). The
/// accessors [`IdeaThemeDefaults`]'s `#[derive(Tokens)]` generates; see
/// the [module docs](self).
///
/// ```
/// let t = idea_theme::IdeaTokens::default();
/// assert_eq!(t.spacing.md().name(), Some("spacing-md"));
/// assert_eq!(t.color.surface_alt().name(), Some("color-surface-alt"));
/// assert_eq!(t.intent.primary.solid_bg().name(), Some("intent-primary-solid-bg"));
/// assert_eq!(t.palette.blue.soft_bg().name(), Some("palette-blue-soft-bg"));
/// ```
///
/// A token that doesn't exist is a compile error rather than a reference
/// that resolves to nothing. `"typography-size-md"` — a plausible-looking
/// name two sheets in this repo once shipped, silently rendering the
/// fallback forever — has no accessor:
///
/// ```compile_fail
/// let t = idea_theme::IdeaTokens::default();
/// let _ = t.typography.size_md(); // no such token → does not compile
/// ```
pub type IdeaTokens = crate::theme::IdeaThemeDefaultsTokens<Root>;

/// Non-intent neutrals — page background, surfaces, text, borders, focus
/// ring, overlay, table tints (`t.color.*`).
pub type ColorTokens = crate::theme::ColorsTokens<Root>;
/// The seven intent palettes, each reached as `t.intent.<name>.<slot>()`.
pub type IntentTokens = crate::theme::IntentsTokens<Root>;
/// The twelve general-purpose hues, each reached as `t.palette.<hue>.<slot>()`.
pub type PaletteTokens = crate::theme::PaletteTokens<Root>;
/// Effect colors — shadow, glows, the accent gradient (`t.effect.*`).
pub type EffectTokens = crate::theme::EffectsTokens<Root>;
/// The spacing scale (`t.spacing.*`).
pub type SpacingTokens = crate::theme::SpacingTokens<Root>;
/// The corner-radius scale, plus the computed `t.radius.pill()`.
pub type RadiusTokens = crate::theme::RadiusTokens<Root>;
/// Per-variant font sizes (`t.typography.*`).
pub type TypographyTokens = crate::theme::TypographyTokens<Root>;

/// One intent's (or hue's) six color slots. Every intent and hue shares
/// the one derived type; the names below say which one a binding means.
pub type SwatchTokens = crate::theme::IntentColorsTokens<Root>;
pub type PrimaryTokens = SwatchTokens;
pub type SecondaryTokens = SwatchTokens;
pub type NeutralTokens = SwatchTokens;
pub type SuccessTokens = SwatchTokens;
pub type DangerTokens = SwatchTokens;
pub type WarningTokens = SwatchTokens;
pub type InfoTokens = SwatchTokens;
pub type RedTokens = SwatchTokens;
pub type OrangeTokens = SwatchTokens;
pub type AmberTokens = SwatchTokens;
pub type YellowTokens = SwatchTokens;
pub type LimeTokens = SwatchTokens;
pub type GreenTokens = SwatchTokens;
pub type TealTokens = SwatchTokens;
pub type CyanTokens = SwatchTokens;
pub type BlueTokens = SwatchTokens;
pub type IndigoTokens = SwatchTokens;
pub type VioletTokens = SwatchTokens;
pub type PinkTokens = SwatchTokens;

/// The vocabulary, for code that isn't inside a `stylesheet!` block.
///
/// Component code that builds a `StyleRules` by hand (computed layers,
/// per-instance overrides) has no block binding to reach for, but needs
/// the same typed reference: `tokens().color.text_muted()` rather than
/// `Tokenized::token("color-text-muted", Color("#6b7280".into()))` — a
/// spelling that was not only stringly-typed but carried a restated hex
/// free to drift from the palette. Same as `IdeaThemeDefaults::tokens()`.
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
    use crate::theme_runtime::ThemeTokens;
    use runtime_core::{Color, Length, Tokenized};
    use std::collections::BTreeMap;

    /// Every color accessor in the vocabulary, paired with nothing —
    /// the *name* is what the accessor itself reports, which is the
    /// point of the tests below. The one place they are all enumerated by
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
            t.color.table_header(),
            t.color.table_row_highlight(),
            t.color.table_row_highlight_hover(),
            t.color.table_row_warning(),
            t.color.table_row_warning_hover(),
            t.color.table_row_danger(),
            t.color.table_row_danger_hover(),
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
        let p = &t.palette;
        slots!(
            p.red, p.orange, p.amber, p.yellow, p.lime, p.green,
            p.teal, p.cyan, p.blue, p.indigo, p.violet, p.pink,
        );
        let e = &t.effect;
        out.extend([
            e.shadow(),
            e.glow_primary(),
            e.glow_secondary(),
            e.glow_neutral(),
            e.glow_success(),
            e.glow_danger(),
            e.glow_warning(),
            e.glow_info(),
            e.gradient_start(),
            e.gradient_mid(),
            e.gradient_end(),
            e.gradient_text(),
        ]);
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

    /// The derived name table — what the theme installs and what every
    /// accessor indexes.
    fn declared_names() -> Vec<&'static str> {
        IdeaThemeDefaults::NAMES.to_vec()
    }

    /// The names, byte for byte and in the order idea has always installed
    /// them: neutrals, intents, palette, effects, then the length tokens.
    /// The order is what a static render's `:root` block is written in, so
    /// keeping it is what keeps the emitted CSS unchanged.
    #[test]
    fn derived_names_are_the_canonical_names_in_install_order() {
        use crate::theme::{
            CANONICAL_EFFECT_TOKENS, CANONICAL_INTENT_TOKENS, CANONICAL_LENGTH_TOKENS,
            CANONICAL_NEUTRAL_TOKENS, CANONICAL_PALETTE_TOKENS,
        };
        let mut legacy: Vec<&str> = CANONICAL_NEUTRAL_TOKENS.to_vec();
        legacy.extend(CANONICAL_INTENT_TOKENS);
        legacy.extend(CANONICAL_PALETTE_TOKENS);
        legacy.extend(CANONICAL_EFFECT_TOKENS);
        legacy.extend(CANONICAL_LENGTH_TOKENS);
        assert_eq!(declared_names(), legacy);
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
            assert_eq!(e.vocabulary, "IdeaThemeRef", "listed under the type sheets name");
        }
    }

    /// The descriptor list and the accessor vocabulary are the same
    /// facts in the same order. Compared sequence-wise, not as sets: a
    /// descriptor that drifted onto the wrong namespace would still be
    /// present, and only order catches it.
    ///
    /// This is what lets a generated editor be trusted — every control
    /// it renders corresponds to an accessor a stylesheet can actually
    /// name, and no token is missing a control.
    #[test]
    fn descriptors_match_the_accessor_vocabulary() {
        let described: Vec<&'static str> = token_descriptors().map(|d| d.name).collect();
        assert_eq!(described, declared_names());
    }

    /// Each descriptor's accessor path spells its own token name —
    /// the same invariant the catalog slice is held to, restated for
    /// the ungated list because an editor inserts `path` into
    /// generated source while keying edits by `name`.
    #[test]
    fn descriptor_path_spells_its_token_name() {
        for d in token_descriptors() {
            let from_path = format!(
                "{}{}",
                if d.namespace == "intent" { "intent-" } else { "" },
                d.path.trim_start_matches("intent.").replace('.', "-").replace('_', "-")
            );
            assert_eq!(from_path, d.name, "path `{}` must spell `{}`", d.path, d.name);
            assert_eq!(
                d.namespace,
                d.path.split('.').next().unwrap(),
                "namespace must be the path's first segment"
            );
        }
    }

    /// `field_path` names a real place on `IdeaThemeDefaults`: it
    /// starts at one of the five theme groups and ends at the same
    /// accessor the path does. A generated `theme.<field_path> = …`
    /// that pointed at a group that doesn't exist would only fail in
    /// the USER's crate, long after the descriptor shipped.
    ///
    /// `radius-pill` is the sole exception and is asserted as such —
    /// the `Radius` struct deliberately has no `pill` field, so
    /// codegen must skip it rather than emit an assignment.
    #[test]
    fn descriptor_field_path_points_at_a_theme_field() {
        const GROUPS: [&str; 7] =
            ["colors", "intents", "palette", "effects", "spacing", "radius", "typography"];
        let mut without_field = Vec::new();
        for d in token_descriptors() {
            let Some(field) = d.field_path else {
                without_field.push(d.name);
                continue;
            };
            let mut segments = field.split('.');
            let group = segments.next().unwrap();
            assert!(GROUPS.contains(&group), "`{}` → unknown theme group `{group}`", d.name);
            assert_eq!(
                field.rsplit('.').next(),
                d.path.rsplit('.').next(),
                "`{}`: field path and accessor path must end at the same identifier",
                d.name
            );
        }
        assert_eq!(
            without_field,
            vec!["radius-pill"],
            "only radius-pill has no theme field behind it"
        );
    }

    /// A descriptor's declared [`TokenKind`] matches the `TokenValue`
    /// variant the base palette actually installs under that name.
    ///
    /// The one assertion that catches a Color/Length mix-up in the
    /// macros: an editor picks its control from `kind`, so a token
    /// described as a Color but installed as a Length would render a
    /// color picker that writes a value nothing can resolve.
    #[test]
    fn descriptor_kind_matches_the_installed_value() {
        let installed: BTreeMap<&'static str, TokenValue> = IdeaThemeRef::new(light_theme())
            .tokens()
            .into_iter()
            .map(|e| (e.name, e.value))
            .collect();

        for d in token_descriptors() {
            match (d.kind, installed.get(d.name)) {
                (TokenKind::Color, Some(TokenValue::Color(_)))
                | (TokenKind::Length, Some(TokenValue::Length(_))) => {}
                (kind, other) => panic!("`{}` described as {kind:?} but installs as {other:?}", d.name),
            }
        }
    }

    /// The vocabulary agrees with the `CANONICAL_*` lists that back the
    /// public `is_canonical_*` predicates, so the typed path and the
    /// string path (`theme_color`, for computed names) can't disagree
    /// about which names exist.
    #[test]
    fn vocabulary_matches_canonical_token_lists() {
        use crate::theme::{
            CANONICAL_EFFECT_TOKENS, CANONICAL_INTENT_TOKENS, CANONICAL_LENGTH_TOKENS,
            CANONICAL_NEUTRAL_TOKENS, CANONICAL_PALETTE_TOKENS,
        };
        let mut canonical: Vec<&str> = CANONICAL_NEUTRAL_TOKENS.to_vec();
        canonical.extend(CANONICAL_INTENT_TOKENS);
        canonical.extend(CANONICAL_PALETTE_TOKENS);
        canonical.extend(CANONICAL_EFFECT_TOKENS);
        canonical.extend(CANONICAL_LENGTH_TOKENS);
        canonical.sort_unstable();

        let mut declared = declared_names();
        declared.sort_unstable();
        assert_eq!(declared, canonical);
    }
}
