//! Extensible theming — the open-trait modifier system.
//!
//! This module is the abstract surface and built-in modifier set:
//! traits ([`Tone`], [`Variant`], [`ButtonSize`], [`Shape`],
//! [`TypographyKind`]), declarative macros, and zero-sized markers
//! (`tone::Primary`, `variant::Filled`, ...). Consumers — component
//! libraries like `idea-ui` — depend on this crate to drive their
//! own widgets via the trait surface.
//!
//! Composes four orthogonal modifier axes, each backed by a trait:
//!
//! | Axis | Trait | What it answers |
//! |---|---|---|
//! | **Variant** | [`Variant`] | Skeleton — which surfaces have fill, stroke, or are transparent |
//! | **Tone** | [`Tone`] | Semantic palette — Primary, Danger, custom Hype, … |
//! | **Size** | [`ButtonSize`] | Scale — padding + font size for the Button family |
//! | **Shape** | [`Shape`] | Corner radius |
//!
//! Plus [`TypographyKind`] for the Typography component's per-variant
//! font characteristics. Each axis is a separate trait so different
//! components can compose only the axes they need — a Card uses
//! Tone+Variant+Shape, a Typography uses TypographyKind alone, etc.
//!
//! # Compile-safe extension
//!
//! Apps add new modifiers by declaring a ZST and implementing the
//! relevant trait. Required trait methods enforce slot completeness at
//! compile time — there's no `Option`-returning fallback, no
//! `Custom(&'static str)` escape hatch, no runtime panic for a missing
//! slot. If you can write `impl Tone for Hype { … }` and it compiles,
//! every component that consumes a Tone works with Hype.
//!
//! # Closing the loop with the framework
//!
//! Each component's apply-style closure builds a [`StyleApplication`]
//! with a *computed layer* — a closure that returns a
//! [`StyleRules`](runtime_core::StyleRules) and a cache key derived
//! from the modifier identities. The framework caches one resolved
//! `StyleRules` per unique modifier combination per theme, so 1000
//! buttons sharing `(Filled, Danger, Md, Pill)` materialize one class
//! on the backend.

mod macros;
pub mod shape;
mod sheets;
pub mod size;
pub mod tone;
pub mod typography;
pub mod variant;

pub use sheets::{
    install_alert_sheet, install_badge_sheet, install_button_sheet, install_default_alert_sheet,
    install_default_badge_sheet, install_default_button_sheet, install_default_icon_button_sheet,
    install_default_tag_sheet, install_default_typography_sheet, install_icon_button_sheet,
    install_tag_sheet, install_typography_sheet, installed_alert_sheet, installed_badge_sheet,
    installed_button_sheet, installed_icon_button_sheet, installed_tag_sheet,
    installed_typography_sheet, AlertSheetBuilder, BadgeSheetBuilder, ButtonSheetBuilder,
    IconButtonSheetBuilder, TagSheetBuilder, TypographySheetBuilder,
};

use std::rc::Rc;

use runtime_core::{Color, FontWeight, Length, StyleRules, Tokenized};

use crate::theme::IdeaTheme;

// =============================================================================
// Tone — semantic palette
// =============================================================================

/// A semantic color palette. Built-ins answer with theme intent colors;
/// apps add custom tones by implementing this trait on a marker type.
///
/// Each method returns a [`Tokenized<Color>`] — a token reference plus
/// fallback. The framework resolves these against the active theme at
/// apply time, so two themes binding different concrete colors to
/// `tone-hype-fill-bg` swap without any class regeneration.
///
/// **Slot completeness is compile-enforced.** All seven methods are
/// required; there's no `Option` return type. If a custom Tone needs
/// a slot it doesn't have an obvious value for, it picks a sensible
/// reuse (e.g. `ghost_fg = self.fill_bg(theme)`).
pub trait Tone: 'static {
    /// Stable identifier — joined with other modifier keys to form
    /// the resolution-cache key. Must be unique across tone impls.
    fn key(&self) -> &'static str;

    /// Filled-background color for Solid-kind surfaces.
    fn fill_bg(&self, theme: &dyn IdeaTheme) -> Tokenized<Color>;

    /// Text/icon color rendered on top of `fill_bg`.
    fn fill_fg(&self, theme: &dyn IdeaTheme) -> Tokenized<Color>;

    /// Tinted background for Soft-kind surfaces — a muted version of
    /// the tone's identity color, distinct from the solid fill.
    fn soft_bg(&self, theme: &dyn IdeaTheme) -> Tokenized<Color>;

    /// Text/icon color rendered on top of `soft_bg`. Usually the
    /// tone's "foreground" tone, chosen for legibility on the tint.
    fn soft_fg(&self, theme: &dyn IdeaTheme) -> Tokenized<Color>;

    /// Stroke color for Outlined-kind borders.
    fn stroke_color(&self, theme: &dyn IdeaTheme) -> Tokenized<Color>;

    /// Text/icon color for Outlined-kind surfaces (over the page
    /// background, not over a tinted fill).
    fn stroke_fg(&self, theme: &dyn IdeaTheme) -> Tokenized<Color>;

    /// Text/icon color for Ghost-kind surfaces (transparent, no border).
    fn ghost_fg(&self, theme: &dyn IdeaTheme) -> Tokenized<Color>;

    /// Color used when the component is in a disabled state. Built-ins
    /// route to `theme.colors().text_muted`; custom tones may match or
    /// pick something distinct.
    fn disabled(&self, theme: &dyn IdeaTheme) -> Tokenized<Color>;

    /// Color of the focus ring around the component when keyboard-focused.
    fn focus_ring(&self, theme: &dyn IdeaTheme) -> Tokenized<Color>;
}

/// Typed handle wrapping `Rc<dyn Tone>` so prop fields accept
/// `impl Into<ToneRef>` and call sites just write `tone: Hype.into()`.
///
/// The newtype routes around the orphan rule — a blanket
/// `impl<T: Tone> From<T> for Rc<dyn Tone>` isn't allowed because `Rc`
/// is foreign and not `#[fundamental]`. `ToneRef` is local, so we get
/// the From blanket here.
///
/// `Deref<Target = dyn Tone>` makes the wrapper transparent at use
/// sites — call methods on it directly (`tone.fill_bg(theme)`),
/// no `.0`.
#[derive(Clone)]
pub struct ToneRef(pub Rc<dyn Tone>);

impl<T: Tone> From<T> for ToneRef {
    fn from(t: T) -> Self {
        ToneRef(Rc::new(t))
    }
}

impl std::ops::Deref for ToneRef {
    type Target = dyn Tone;
    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}

impl Default for ToneRef {
    fn default() -> Self {
        ToneRef(Rc::new(tone::Primary))
    }
}

impl ToneRef {
    /// All built-in tones as `(key, ref)` pairs. Used by docs-tooling
    /// to render an interactive picker over the seven canonical
    /// intents. Apps that ship custom tones can extend their docs
    /// surface with their own enumeration helper.
    pub fn builtins() -> Vec<(&'static str, ToneRef)> {
        vec![
            ("primary", ToneRef(Rc::new(tone::Primary))),
            ("secondary", ToneRef(Rc::new(tone::Secondary))),
            ("neutral", ToneRef(Rc::new(tone::Neutral))),
            ("success", ToneRef(Rc::new(tone::Success))),
            ("danger", ToneRef(Rc::new(tone::Danger))),
            ("warning", ToneRef(Rc::new(tone::Warning))),
            ("info", ToneRef(Rc::new(tone::Info))),
        ]
    }
}

// Note: a blanket `From<T: Tone> for Option<ToneRef>` would let
// components with an optional tone accept `tone: Hype.into()` without
// `Some(...)`, but Rust's orphan rule rejects it — `Option` isn't
// `#[fundamental]`. Affected components (Field) require an explicit
// `Some(Hype.into())`. One extra `Some(...)` per call site.

// =============================================================================
// Variant — skeleton (which surfaces fill, stroke, or are transparent)
// =============================================================================

/// The structural form of a component. Variants are mutually exclusive
/// for a given component instance — a Button is filled OR outlined OR
/// ghost, never two.
///
/// A Variant's [`render`](Self::render) returns a `StyleRules` block
/// that — together with the modifier defaults from `ResolutionCtx` —
/// fully describes the variant's appearance. The framework merges the
/// returned rules into the resolved [`StyleApplication`] via the
/// computed layer.
pub trait Variant: 'static {
    /// Stable identifier — joined with other modifier keys to form
    /// the resolution-cache key.
    fn key(&self) -> &'static str;

    /// Build the property contributions for this variant against the
    /// active modifier set. Variants typically start from
    /// [`ResolutionCtx::modifier_defaults`] and overlay their
    /// variant-specific properties (`background`, `color`, border
    /// width/color, etc.).
    fn render(&self, ctx: &ResolutionCtx) -> StyleRules;
}

/// Typed handle wrapping `Rc<dyn Variant>`. See [`ToneRef`] for the
/// rationale.
#[derive(Clone)]
pub struct VariantRef(pub Rc<dyn Variant>);

impl<V: Variant> From<V> for VariantRef {
    fn from(v: V) -> Self {
        VariantRef(Rc::new(v))
    }
}

impl std::ops::Deref for VariantRef {
    type Target = dyn Variant;
    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}

impl Default for VariantRef {
    fn default() -> Self {
        VariantRef(Rc::new(variant::Filled))
    }
}

impl VariantRef {
    pub fn builtins() -> Vec<(&'static str, VariantRef)> {
        vec![
            ("filled", VariantRef(Rc::new(variant::Filled))),
            ("soft", VariantRef(Rc::new(variant::Soft))),
            ("outlined", VariantRef(Rc::new(variant::Outlined))),
            ("ghost", VariantRef(Rc::new(variant::Ghost))),
        ]
    }
}

// =============================================================================
// ButtonSize — scale modifier for the Button family
// =============================================================================

/// A scale step for buttons. Resolves padding (horizontal + vertical)
/// and font-size — the three knobs that move together when a button
/// gets larger or smaller.
///
/// Component-specific because a "size" answers different questions
/// for different components (Button vs Typography vs Field). Keeping
/// each component's scale axis its own trait avoids fake-uniform slots.
pub trait ButtonSize: 'static {
    fn key(&self) -> &'static str;
    fn padding_vertical(&self) -> Tokenized<Length>;
    fn padding_horizontal(&self) -> Tokenized<Length>;
    fn font_size(&self) -> Tokenized<Length>;
}

/// Typed handle wrapping `Rc<dyn ButtonSize>`. See [`ToneRef`].
#[derive(Clone)]
pub struct ButtonSizeRef(pub Rc<dyn ButtonSize>);

impl<S: ButtonSize> From<S> for ButtonSizeRef {
    fn from(s: S) -> Self {
        ButtonSizeRef(Rc::new(s))
    }
}

impl std::ops::Deref for ButtonSizeRef {
    type Target = dyn ButtonSize;
    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}

impl Default for ButtonSizeRef {
    fn default() -> Self {
        ButtonSizeRef(Rc::new(size::Md))
    }
}

impl ButtonSizeRef {
    pub fn builtins() -> Vec<(&'static str, ButtonSizeRef)> {
        vec![
            ("sm", ButtonSizeRef(Rc::new(size::Sm))),
            ("md", ButtonSizeRef(Rc::new(size::Md))),
            ("lg", ButtonSizeRef(Rc::new(size::Lg))),
        ]
    }
}

// =============================================================================
// Shape — corner radius
// =============================================================================

/// A discrete corner-radius token. Built-ins map onto the theme's
/// radius scale (sm/md/lg/pill); custom shapes can hold any
/// `Tokenized<Length>`.
pub trait Shape: 'static {
    fn key(&self) -> &'static str;
    fn border_radius(&self) -> Tokenized<Length>;
}

/// Typed handle wrapping `Rc<dyn Shape>`. See [`ToneRef`].
#[derive(Clone)]
pub struct ShapeRef(pub Rc<dyn Shape>);

impl<S: Shape> From<S> for ShapeRef {
    fn from(s: S) -> Self {
        ShapeRef(Rc::new(s))
    }
}

impl std::ops::Deref for ShapeRef {
    type Target = dyn Shape;
    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}

impl Default for ShapeRef {
    fn default() -> Self {
        ShapeRef(Rc::new(shape::Md))
    }
}

impl ShapeRef {
    pub fn builtins() -> Vec<(&'static str, ShapeRef)> {
        vec![
            ("sm", ShapeRef(Rc::new(shape::Sm))),
            ("md", ShapeRef(Rc::new(shape::Md))),
            ("lg", ShapeRef(Rc::new(shape::Lg))),
            ("pill", ShapeRef(Rc::new(shape::Pill))),
        ]
    }
}

// =============================================================================
// TypographyKind — typography component's per-variant characteristics
// =============================================================================

/// One variant of the Typography component (H1, Body, Caption, …). A
/// kind owns *all* of its visual characteristics: size, weight, line
/// height, letter spacing. Apps add new typography variants — say a
/// brand display kind — by implementing this trait.
pub trait TypographyKind: 'static {
    fn key(&self) -> &'static str;
    fn font_size(&self) -> Tokenized<Length>;
    fn font_weight(&self) -> FontWeight;
    fn line_height(&self) -> Tokenized<f32>;
    fn letter_spacing(&self) -> Tokenized<f32>;
}

/// Typed handle wrapping `Rc<dyn TypographyKind>`. See [`ToneRef`].
#[derive(Clone)]
pub struct TypographyKindRef(pub Rc<dyn TypographyKind>);

impl<K: TypographyKind> From<K> for TypographyKindRef {
    fn from(k: K) -> Self {
        TypographyKindRef(Rc::new(k))
    }
}

impl std::ops::Deref for TypographyKindRef {
    type Target = dyn TypographyKind;
    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}

impl Default for TypographyKindRef {
    fn default() -> Self {
        TypographyKindRef(Rc::new(typography::Body))
    }
}

/// Uniform handle over the five `*Ref` newtypes for docs-tooling.
/// Lets a single generic picker render any modifier axis without one
/// hardcoded function per type.
///
/// Apps with custom modifier types can implement this trait to plug
/// into the same docs-controls surface.
pub trait RefBuiltins: Clone + 'static {
    /// Built-in (key, instance) pairs the picker enumerates.
    fn builtins_list() -> Vec<(&'static str, Self)>;
    /// The stable key of the current selection — used to drive the
    /// picker's active highlight.
    fn current_key(&self) -> &'static str;
}

impl RefBuiltins for ToneRef {
    fn builtins_list() -> Vec<(&'static str, Self)> {
        ToneRef::builtins()
    }
    fn current_key(&self) -> &'static str {
        self.key()
    }
}
impl RefBuiltins for VariantRef {
    fn builtins_list() -> Vec<(&'static str, Self)> {
        VariantRef::builtins()
    }
    fn current_key(&self) -> &'static str {
        self.key()
    }
}
impl RefBuiltins for ButtonSizeRef {
    fn builtins_list() -> Vec<(&'static str, Self)> {
        ButtonSizeRef::builtins()
    }
    fn current_key(&self) -> &'static str {
        self.key()
    }
}
impl RefBuiltins for ShapeRef {
    fn builtins_list() -> Vec<(&'static str, Self)> {
        ShapeRef::builtins()
    }
    fn current_key(&self) -> &'static str {
        self.key()
    }
}
impl RefBuiltins for TypographyKindRef {
    fn builtins_list() -> Vec<(&'static str, Self)> {
        TypographyKindRef::builtins()
    }
    fn current_key(&self) -> &'static str {
        self.key()
    }
}

impl TypographyKindRef {
    pub fn builtins() -> Vec<(&'static str, TypographyKindRef)> {
        vec![
            ("display", TypographyKindRef(Rc::new(typography::Display))),
            ("h1", TypographyKindRef(Rc::new(typography::H1))),
            ("h2", TypographyKindRef(Rc::new(typography::H2))),
            ("h3", TypographyKindRef(Rc::new(typography::H3))),
            ("body-xl", TypographyKindRef(Rc::new(typography::BodyXl))),
            ("body-lg", TypographyKindRef(Rc::new(typography::BodyLg))),
            ("body", TypographyKindRef(Rc::new(typography::Body))),
            ("body-sm", TypographyKindRef(Rc::new(typography::BodySm))),
            ("caption", TypographyKindRef(Rc::new(typography::Caption))),
            ("overline", TypographyKindRef(Rc::new(typography::Overline))),
        ]
    }
}

// =============================================================================
// ResolutionCtx — bundle passed into Variant::render
// =============================================================================

/// The active modifier set a [`Variant`] composes against. Variants
/// pull semantic colors from `tone` and may consult `theme` for
/// non-intent neutrals (focus ring, page background, etc.).
///
/// **Variants only see tone + theme** — size and shape contributions
/// (padding, font-size, border-radius) live in the *component*'s
/// compose closure via [`modifier_defaults`]. This keeps variants
/// pure skeletons and lets components like Badge or Tag (which have
/// their own intrinsic dimensions, not the Button scale) compose
/// without dragging in irrelevant size/shape axes.
pub struct ResolutionCtx<'a> {
    pub theme: &'a dyn IdeaTheme,
    pub tone: &'a dyn Tone,
}

/// Property contributions from the Size and Shape modifiers: padding
/// and font-size from `size`, border-radius from `shape`.
///
/// Components that have a Size/Shape axis (Button, IconButton) call
/// this and merge it with the variant's render output:
///
/// ```ignore
/// let mut rules = modifier_defaults(&*size, &*shape);
/// rules = rules.merge(&variant.render(&ctx));  // variant skeleton on top
/// ```
///
/// Components without a Size/Shape axis (Badge, Tag) skip this — their
/// intrinsic padding/font lives in a base stylesheet.
pub fn modifier_defaults(size: &dyn ButtonSize, shape: &dyn Shape) -> StyleRules {
    let p_v = size.padding_vertical();
    let p_h = size.padding_horizontal();
    let r = shape.border_radius();
    StyleRules {
        padding_top: Some(p_v.clone()),
        padding_bottom: Some(p_v),
        padding_left: Some(p_h.clone()),
        padding_right: Some(p_h),
        font_size: Some(size.font_size()),
        border_top_left_radius: Some(r.clone()),
        border_top_right_radius: Some(r.clone()),
        border_bottom_left_radius: Some(r.clone()),
        border_bottom_right_radius: Some(r),
        ..Default::default()
    }
}

// =============================================================================
// Tests — verify default modifier shape and trait coherence
// =============================================================================

// =============================================================================
// Tests + extension examples
//
// These tests double as the canonical examples of how an app extends
// the system. Each axis (Tone, Variant, ButtonSize, TypographyKind) is
// extended with a custom marker type. The tests confirm the extension
// composes with built-ins through every consumer (Variant::render,
// modifier_defaults, etc.).
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensible::{shape, size, tone, variant};
    use runtime_core::{Color, FontWeight, Length, Tokenized};

    /// Sanity: `.into()` wraps any built-in ZST into its typed
    /// `*Ref` handle. This is the standard call-site idiom — prop
    /// fields use `impl Into<...Ref>` so users never write `Rc::new`.
    #[test]
    fn builtin_zsts_wrap_into_ref_handles() {
        let _t: ToneRef = tone::Primary.into();
        let _t: ToneRef = tone::Danger.into();
        let _v: VariantRef = variant::Filled.into();
        let _v: VariantRef = variant::Outlined.into();
        let _s: ButtonSizeRef = size::Md.into();
        let _h: ShapeRef = shape::Pill.into();
        let _k: TypographyKindRef = typography::H1.into();
    }

    /// Optional tone fields (Field's `tone: Option<ToneRef>`) require
    /// an explicit `Some(...)` wrap. Orphan rule blocks a blanket
    /// `From<T: Tone> for Option<ToneRef>`.
    #[test]
    fn optional_tone_wrap_is_explicit() {
        let opt: Option<ToneRef> = Some(tone::Danger.into());
        assert!(opt.is_some());
    }

    /// `modifier_defaults` (the free function) emits padding,
    /// font-size, and border-radius from a Size/Shape pair. Used by
    /// components like Button that have those axes; components like
    /// Badge skip it entirely.
    #[test]
    fn modifier_defaults_populate_expected_slots() {
        let r = modifier_defaults(&size::Md, &shape::Md);
        assert!(r.padding_top.is_some());
        assert!(r.padding_bottom.is_some());
        assert!(r.padding_left.is_some());
        assert!(r.padding_right.is_some());
        assert!(r.font_size.is_some());
        assert!(r.border_top_left_radius.is_some());
        assert!(r.border_top_right_radius.is_some());
        assert!(r.border_bottom_left_radius.is_some());
        assert!(r.border_bottom_right_radius.is_some());
    }

    // ----- Extension example 1: a custom Tone -----------------------------
    //
    // `Hype` is an app-defined tone with its own brand palette. The
    // `tone!` macro collapses the per-slot impls into a declarative
    // block — same trait surface, less typing. The compiler enforces
    // slot completeness because each slot maps to a required trait
    // method; the macro just shapes the syntax.
    //
    // Token names like `tone-hype-fill-bg` are referenced here. To make
    // them theme-aware (light vs dark), an app installs `MyTheme`
    // whose `ThemeTokens::tokens()` emits these names with per-mode
    // values. Without that installation, the literal fallbacks shown
    // below are what renders.

    crate::tone! {
        pub Hype using self, theme {
            key = "hype",
            fill_bg = crate::color_token!("tone-hype-fill-bg", "#ff00aa"),
            fill_fg = crate::color_token!("tone-hype-fill-fg", "#ffffff"),
            soft_bg = crate::color_token!("tone-hype-soft-bg", "rgba(255,0,170,0.12)"),
            soft_fg = crate::color_token!("tone-hype-soft-fg", "#a30070"),
            stroke_color = crate::color_token!("tone-hype-stroke", "#ff00aa"),
            stroke_fg = self.soft_fg(theme),
            ghost_fg = self.fill_bg(theme),
            disabled = theme.colors().text_muted.clone(),
            focus_ring = self.fill_bg(theme),
            // Declares the tokens this tone introduces. The macro
            // emits `Hype::tokens() -> Vec<TokenEntry>` so an app's
            // ThemeTokens impl can `.extend(Hype::tokens())` instead
            // of hand-maintaining a parallel list of names.
            tokens = [
                "tone-hype-fill-bg" => "#ff00aa",
                "tone-hype-fill-fg" => "#ffffff",
                "tone-hype-soft-bg" => "rgba(255,0,170,0.12)",
                "tone-hype-soft-fg" => "#a30070",
                "tone-hype-stroke" => "#ff00aa",
            ],
        }
    }

    /// The `tokens = [...]` block on `tone!` generates a `tokens()`
    /// inherent method returning every entry declared. Apps aggregate
    /// these via `ThemeTokens::tokens()` so custom-tone token names
    /// land in the runtime token registry on theme install/swap.
    #[test]
    fn custom_tone_emits_tokens_for_theme_aggregation() {
        let entries = Hype::tokens();
        assert_eq!(entries.len(), 5);
        // Names match the slot references — installing these makes
        // Hype's `Tokenized::token(...)` references resolve to the
        // app's chosen values rather than the literal fallbacks.
        let names: Vec<&'static str> = entries.iter().map(|e| e.name).collect();
        assert!(names.contains(&"tone-hype-fill-bg"));
        assert!(names.contains(&"tone-hype-fill-fg"));
        assert!(names.contains(&"tone-hype-soft-bg"));
        assert!(names.contains(&"tone-hype-soft-fg"));
        assert!(names.contains(&"tone-hype-stroke"));
    }

    #[test]
    fn custom_tone_renders_through_builtin_variants() {
        // The point: a built-in Variant (Filled) consumes a custom
        // Tone (Hype) and produces a StyleRules with the custom
        // tone's tokens. No registration, no string keys, no special
        // case for built-in vs custom.
        let theme = crate::theme::light_theme();
        let ctx = ResolutionCtx {
            theme: &theme,
            tone: &Hype,
        };
        let rules = variant::Filled.render(&ctx);
        assert_eq!(
            rules.background.as_ref().and_then(|t| t.name()),
            Some("tone-hype-fill-bg"),
        );
        assert_eq!(
            rules.color.as_ref().and_then(|t| t.name()),
            Some("tone-hype-fill-fg"),
        );

        // And the Outlined variant pulls the stroke + stroke_fg
        // slots — same Tone, different slot selection by the variant.
        let outlined = variant::Outlined.render(&ctx);
        assert_eq!(
            outlined.border_top_color.as_ref().and_then(|t| t.name()),
            Some("tone-hype-stroke"),
        );
        assert_eq!(
            outlined.color.as_ref().and_then(|t| t.name()),
            Some("tone-hype-soft-fg"),
        );
    }

    // ----- Extension example 2: a custom Variant ---------------------------
    //
    // `Elevated` is a Filled-like skeleton with a subtle drop shadow.
    // The `variant!` macro is a thin wrapper — the `render(ctx)` body
    // is normal Rust returning `StyleRules`. Demonstrates that variants
    // are first-class extension points: this one introduces shadow
    // (which built-in variants don't use) and composes with every tone.

    crate::variant! {
        pub Elevated {
            key = "elevated",
            render(ctx) {
                // Variants are pure skeletons now — they don't touch
                // padding/font/radius. Just tone-driven properties +
                // whatever distinguishes this variant (shadow here).
                runtime_core::StyleRules {
                    background: Some(ctx.tone.fill_bg(ctx.theme)),
                    color: Some(ctx.tone.fill_fg(ctx.theme)),
                    shadow: Some(runtime_core::Shadow {
                        x: 0.0,
                        y: 2.0,
                        blur: 8.0,
                        color: Color("rgba(0,0,0,0.18)".into()),
                    }),
                    ..Default::default()
                }
            }
        }
    }

    #[test]
    fn custom_variant_renders_with_builtin_tone() {
        let theme = crate::theme::light_theme();
        let ctx = ResolutionCtx {
            theme: &theme,
            tone: &tone::Primary,
        };
        let rules = Elevated.render(&ctx);
        assert!(rules.background.is_some());
        assert!(rules.shadow.is_some(), "Elevated variant must emit a shadow");
    }

    // ----- Extension example 3: a custom ButtonSize ------------------------
    //
    // `Xxxxs` — extra-extra-small. Smaller than the built-in `Sm`. The
    // size axis is component-specific (different from Typography's
    // size axis), so this trait impl satisfies only ButtonSize.

    struct Xxxxs;

    impl ButtonSize for Xxxxs {
        fn key(&self) -> &'static str {
            "xxxxs"
        }
        fn padding_vertical(&self) -> Tokenized<Length> {
            Tokenized::token("spacing-xxxxs", Length::Px(1.0))
        }
        fn padding_horizontal(&self) -> Tokenized<Length> {
            Tokenized::token("spacing-xxxxs-h", Length::Px(2.0))
        }
        fn font_size(&self) -> Tokenized<Length> {
            Tokenized::token("typography-xxxxs-size", Length::Px(9.0))
        }
    }

    #[test]
    fn custom_size_flows_through_modifier_defaults() {
        let r = modifier_defaults(&Xxxxs, &shape::Md);
        // padding_top picks up `Xxxxs::padding_vertical`'s 1.0 fallback.
        match r.padding_top.as_ref().expect("padding_top set") {
            Tokenized::Token { name, fallback } => {
                assert_eq!(*name, "spacing-xxxxs");
                assert_eq!(*fallback, Length::Px(1.0));
            }
            other => panic!("expected token, got {:?}", other),
        }
    }

    // ----- Extension example 4: a custom TypographyKind --------------------
    //
    // `SexySubtitle` — a brand-specific typography variant. Apps add
    // these by implementing TypographyKind; the Typography component
    // (not built here yet) would consume `Rc<dyn TypographyKind>` for
    // its `kind` prop.

    struct SexySubtitle;

    impl TypographyKind for SexySubtitle {
        fn key(&self) -> &'static str {
            "sexy-subtitle"
        }
        fn font_size(&self) -> Tokenized<Length> {
            Tokenized::token("typography-sexy-subtitle-size", Length::Px(22.0))
        }
        fn font_weight(&self) -> FontWeight {
            FontWeight::Light
        }
        fn line_height(&self) -> Tokenized<f32> {
            Tokenized::Literal(1.3)
        }
        fn letter_spacing(&self) -> Tokenized<f32> {
            Tokenized::Literal(1.4)
        }
    }

    #[test]
    fn custom_typography_kind_exposes_all_slots() {
        let k = SexySubtitle;
        assert_eq!(k.key(), "sexy-subtitle");
        assert_eq!(k.font_weight(), FontWeight::Light);
        match k.font_size() {
            Tokenized::Token { name, fallback } => {
                assert_eq!(name, "typography-sexy-subtitle-size");
                assert_eq!(fallback, Length::Px(22.0));
            }
            other => panic!("expected token, got {:?}", other),
        }
        let _: TypographyKindRef = SexySubtitle.into();
    }

    // ----- Theme bundle composition ----------------------------------------
    //
    // The `theme!` macro generates `impl IdeaTheme + impl ThemeTokens`
    // for an app theme that wraps `IdeaThemeDefaults` and aggregates
    // tokens from extension tones. Adding a new brand tone to your
    // app is a one-line change in the `tones: [...]` list.

    crate::app_theme! {
        pub MyTheme {
            idea: crate::theme::IdeaThemeDefaults,
            tones: [Hype],
        }
    }

    #[test]
    fn theme_macro_bundles_idea_defaults_and_custom_tone_tokens() {
        use crate::theme::IdeaTheme;
        use crate::ThemeTokens;

        let theme = MyTheme {
            idea: crate::theme::light_theme(),
        };

        // IdeaTheme delegation works — every accessor returns the
        // wrapped idea defaults' values.
        assert!(!theme.colors().background.name().unwrap_or("").is_empty());
        assert!(theme.spacing().md > 0.0);

        // ThemeTokens aggregation: idea's built-in tokens PLUS the
        // 5 token entries declared in `Hype`'s `tokens = [...]` block.
        let tokens = theme.tokens();
        let names: Vec<&'static str> = tokens.iter().map(|e| e.name).collect();
        // A few built-in idea tokens should be present.
        assert!(names.contains(&"color-background"));
        assert!(names.contains(&"intent-primary-solid-bg"));
        // The custom Hype tokens too.
        assert!(names.contains(&"tone-hype-fill-bg"));
        assert!(names.contains(&"tone-hype-stroke"));
    }

    // ----- Composition test: every extension axis simultaneously ----------
    //
    // Compose all four custom modifiers in a single Button-shaped call.
    // The whole point of the open-trait design is that built-ins and
    // customs interoperate seamlessly; this test confirms that.

    #[test]
    fn all_custom_modifiers_compose_simultaneously() {
        // Compose like a component would: Size/Shape contributions
        // first, then variant skeleton on top.
        let theme = crate::theme::light_theme();
        let ctx = ResolutionCtx {
            theme: &theme,
            tone: &Hype,
        };
        let rules = modifier_defaults(&Xxxxs, &shape::Pill).merge(&Elevated.render(&ctx));
        // Background from custom Tone (via the Elevated variant).
        assert_eq!(
            rules.background.as_ref().and_then(|t| t.name()),
            Some("tone-hype-fill-bg"),
        );
        // Padding from custom Size (via modifier_defaults).
        assert_eq!(
            rules.padding_top.as_ref().and_then(|t| t.name()),
            Some("spacing-xxxxs"),
        );
        // Border radius from built-in Pill Shape (via modifier_defaults).
        assert_eq!(
            rules
                .border_top_left_radius
                .as_ref()
                .and_then(|t| t.name()),
            Some("radius-pill"),
        );
        // Shadow from custom Variant (Elevated).
        assert!(rules.shadow.is_some());
    }
}
