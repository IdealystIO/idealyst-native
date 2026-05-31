//! The theme abstraction — a *trait* idea-ui's stylesheets consume
//! and that downstream code is free to implement on any struct.
//!
//! The framework stores the active theme as `Rc<dyn Any>` and
//! stylesheets downcast at the closure boundary. To preserve that
//! contract while letting apps swap in their own theme types, idea-ui
//! wraps every active theme in [`IdeaThemeRef`] — a concrete `Any`
//! type that holds a trait object. Stylesheet closures receive
//! `&IdeaThemeRef`, then call trait methods on it; the trait dispatch
//! happens once per resolution, not once per property.
//!
//! # Color organization
//!
//! Colors are split into:
//!
//! - **Neutrals**: page background, surfaces, text colors, borders,
//!   focus ring, overlay — independent of intent.
//! - **Intent palettes**: one [`IntentColors`] block per built-in
//!   intent (Primary, Secondary, Neutral, Success, Danger, Warning,
//!   Info). Each block carries the tokens every intent-aware
//!   component needs to render across the three visual `kind`s
//!   (Solid, Soft, Outlined / Ghost).

use std::any::Any;
use std::rc::Rc;

use runtime_core::{Color, Length, Tokenized};
use crate::theme_runtime::{install_theme, set_theme, ThemeTokens, TokenEntry, TokenValue};

// =============================================================================
// IntentColors — per-intent palette
// =============================================================================

/// Color tokens for a single intent across the visual "kinds":
///
/// - **Solid**: filled background (`solid_bg`) + contrasting text (`solid_text`).
/// - **Soft**: tinted background (`soft_bg`) + intent-colored text (`soft_text`).
/// - **Outlined / Ghost**: transparent background; intent color used as
///   text (`fg`) and as the border color (`border`) for Outlined.
///
/// Hover and pressed feedback come from a uniform opacity dim at the
/// component-stylesheet level — no per-state color slots here.
/// (A future framework feature for per-state overrides would let us
/// add `*_hover` / `*_pressed` slots without breaking call sites; the
/// per-component opacity-dim is a v1 placeholder.)
#[derive(Clone)]
pub struct IntentColors {
    /// Filled background (Solid kind).
    pub solid_bg: Tokenized<Color>,
    /// Text/icon color rendered on top of `solid_bg`.
    pub solid_text: Tokenized<Color>,
    /// Tinted background (Soft kind).
    pub soft_bg: Tokenized<Color>,
    /// Text/icon color rendered on top of `soft_bg` — usually the
    /// intent's "foreground" tone, picked for legibility on the soft
    /// tint.
    pub soft_text: Tokenized<Color>,
    /// The intent color used as text for Outlined / Ghost kinds, and
    /// as the border color for Outlined. Picked for legibility on the
    /// page background, not on a filled surface.
    pub fg: Tokenized<Color>,
    /// Border color for Outlined kind. Usually equal to `fg`, but kept
    /// separate so themes can dial it independently.
    pub border: Tokenized<Color>,
}

// =============================================================================
// Intents — bundle of all 7 IntentColors blocks
// =============================================================================

/// All seven intent palettes a theme exposes. Accessed via the
/// [`Intents`] getters so call sites don't need to know which fields
/// the struct has.
#[derive(Clone)]
pub struct Intents {
    pub primary: IntentColors,
    pub secondary: IntentColors,
    pub neutral: IntentColors,
    pub success: IntentColors,
    pub danger: IntentColors,
    pub warning: IntentColors,
    pub info: IntentColors,
}

// =============================================================================
// Colors — non-intent neutrals
// =============================================================================

/// Theme-wide color tokens that aren't tied to a single intent.
#[derive(Clone)]
pub struct Colors {
    pub background: Tokenized<Color>,
    pub surface: Tokenized<Color>,
    pub surface_alt: Tokenized<Color>,

    pub text: Tokenized<Color>,
    pub text_muted: Tokenized<Color>,
    pub text_inverse: Tokenized<Color>,

    pub border: Tokenized<Color>,
    pub border_hover: Tokenized<Color>,
    pub border_strong: Tokenized<Color>,

    pub focus_ring: Tokenized<Color>,
    pub overlay: Tokenized<Color>,
}

#[derive(Clone)]
pub struct Spacing {
    pub xs: f32,
    pub sm: f32,
    pub md: f32,
    pub lg: f32,
    pub xl: f32,
    pub xxl: f32,
}

#[derive(Clone)]
pub struct Radius {
    pub sm: f32,
    pub md: f32,
    pub lg: f32,
    pub pill: f32,
}

/// Per-variant font size knobs. Each field corresponds to one
/// `TypographyKind` variant on the `Typography` component. Themes
/// override individual fields to retune the type scale without
/// touching the rest of the stylesheet system.
///
/// Font weight, line height, and letter spacing are not theme-tokenized
/// at this layer — they're encoded per-variant in the `Typography`
/// stylesheet block. Tokenizing every property per variant would emit
/// 40+ extra tokens per theme; sizes are by far the most-overridden
/// knob, so they're the only thing pulled up to the theme struct.
#[derive(Clone)]
pub struct Typography {
    pub display_size: f32,
    pub h1_size: f32,
    pub h2_size: f32,
    pub h3_size: f32,
    pub body_xl_size: f32,
    pub body_lg_size: f32,
    pub body_size: f32,
    pub body_sm_size: f32,
    pub caption_size: f32,
    pub overline_size: f32,
}

// =============================================================================
// The trait
// =============================================================================

/// The contract idea-ui's stylesheets depend on. Implement this on
/// any `'static` struct to make it a valid theme.
pub trait IdeaTheme: Any + 'static {
    fn colors(&self) -> &Colors;
    fn intents(&self) -> &Intents;
    fn spacing(&self) -> &Spacing;
    fn radius(&self) -> &Radius;
    fn typography(&self) -> &Typography;
}

// =============================================================================
// IdeaThemeRef — the framework-side concrete carrier
// =============================================================================

pub struct IdeaThemeRef {
    inner: Rc<dyn IdeaTheme>,
}

impl IdeaThemeRef {
    pub fn new<T: IdeaTheme>(theme: T) -> Self {
        Self { inner: Rc::new(theme) }
    }

    pub fn from_rc(inner: Rc<dyn IdeaTheme>) -> Self {
        Self { inner }
    }

    pub fn inner(&self) -> &dyn IdeaTheme {
        &*self.inner
    }
}

impl IdeaTheme for IdeaThemeRef {
    fn colors(&self) -> &Colors {
        self.inner.colors()
    }
    fn intents(&self) -> &Intents {
        self.inner.intents()
    }
    fn spacing(&self) -> &Spacing {
        self.inner.spacing()
    }
    fn radius(&self) -> &Radius {
        self.inner.radius()
    }
    fn typography(&self) -> &Typography {
        self.inner.typography()
    }
}

impl ThemeTokens for IdeaThemeRef {
    fn tokens(&self) -> Vec<TokenEntry> {
        let c = self.colors();
        let i = self.intents();
        let s = self.spacing();
        let r = self.radius();
        let ty = self.typography();
        fn entry(t: &Tokenized<Color>) -> TokenEntry {
            let name = t.name().expect("color fields must be Tokenized::Token");
            TokenEntry {
                name,
                value: TokenValue::Color(t.value().clone()),
            }
        }
        fn len(name: &'static str, v: f32) -> TokenEntry {
            TokenEntry {
                name,
                value: TokenValue::Length(Length::Px(v)),
            }
        }
        // Helper: emit every field of an IntentColors block.
        fn intent_entries(ic: &IntentColors, out: &mut Vec<TokenEntry>) {
            out.push(entry(&ic.solid_bg));
            out.push(entry(&ic.solid_text));
            out.push(entry(&ic.soft_bg));
            out.push(entry(&ic.soft_text));
            out.push(entry(&ic.fg));
            out.push(entry(&ic.border));
        }
        let mut out = vec![
            entry(&c.background),
            entry(&c.surface),
            entry(&c.surface_alt),
            entry(&c.text),
            entry(&c.text_muted),
            entry(&c.text_inverse),
            entry(&c.border),
            entry(&c.border_hover),
            entry(&c.border_strong),
            entry(&c.focus_ring),
            entry(&c.overlay),
        ];
        intent_entries(&i.primary, &mut out);
        intent_entries(&i.secondary, &mut out);
        intent_entries(&i.neutral, &mut out);
        intent_entries(&i.success, &mut out);
        intent_entries(&i.danger, &mut out);
        intent_entries(&i.warning, &mut out);
        intent_entries(&i.info, &mut out);

        // Spacing: one length token per Spacing field.
        out.push(len("spacing-xs", s.xs));
        out.push(len("spacing-sm", s.sm));
        out.push(len("spacing-md", s.md));
        out.push(len("spacing-lg", s.lg));
        out.push(len("spacing-xl", s.xl));
        out.push(len("spacing-xxl", s.xxl));

        // Radius: one length token per Radius field.
        out.push(len("radius-sm", r.sm));
        out.push(len("radius-md", r.md));
        out.push(len("radius-lg", r.lg));
        out.push(len("radius-pill", r.pill));

        // Typography: one length token per variant. Names match the
        // variant keys in the Typography stylesheet block.
        out.push(len("typography-display-size", ty.display_size));
        out.push(len("typography-h1-size", ty.h1_size));
        out.push(len("typography-h2-size", ty.h2_size));
        out.push(len("typography-h3-size", ty.h3_size));
        out.push(len("typography-body-xl-size", ty.body_xl_size));
        out.push(len("typography-body-lg-size", ty.body_lg_size));
        out.push(len("typography-body-size", ty.body_size));
        out.push(len("typography-body-sm-size", ty.body_sm_size));
        out.push(len("typography-caption-size", ty.caption_size));
        out.push(len("typography-overline-size", ty.overline_size));

        out
    }
}

// =============================================================================
// Default theme implementation
// =============================================================================

#[derive(Clone)]
pub struct IdeaThemeDefaults {
    pub colors: Colors,
    pub intents: Intents,
    pub spacing: Spacing,
    pub radius: Radius,
    pub typography: Typography,
}

impl IdeaTheme for IdeaThemeDefaults {
    fn colors(&self) -> &Colors {
        &self.colors
    }
    fn intents(&self) -> &Intents {
        &self.intents
    }
    fn spacing(&self) -> &Spacing {
        &self.spacing
    }
    fn radius(&self) -> &Radius {
        &self.radius
    }
    fn typography(&self) -> &Typography {
        &self.typography
    }
}

const DEFAULT_SPACING: Spacing = Spacing {
    xs: 4.0,
    sm: 8.0,
    md: 12.0,
    lg: 16.0,
    xl: 24.0,
    xxl: 32.0,
};

const DEFAULT_RADIUS: Radius = Radius {
    sm: 4.0,
    md: 8.0,
    lg: 12.0,
    pill: 999.0,
};

const DEFAULT_TYPOGRAPHY: Typography = Typography {
    display_size: 56.0,
    h1_size: 36.0,
    h2_size: 28.0,
    h3_size: 20.0,
    body_xl_size: 20.0,
    body_lg_size: 18.0,
    body_size: 14.0,
    body_sm_size: 13.0,
    caption_size: 12.0,
    overline_size: 11.0,
};

fn tok(name: &'static str, fallback: &str) -> Tokenized<Color> {
    Tokenized::token(name, Color(fallback.into()))
}

/// Build an `IntentColors` with token names auto-derived from the
/// intent name (e.g. `"primary"` → tokens `intent-primary-solid-bg`,
/// `intent-primary-solid-text`, …) at **compile time** via `concat!`.
///
/// Order: `solid_bg, solid_text, soft_bg, soft_text, fg, border`.
///
/// Previously a `fn` that did `Box::leak(format!(...))` for each name,
/// allocating 6 `&'static str`s on every call. Multiple constructions
/// of the same theme (hot-reload, fixture teardown, tests) accumulated
/// duplicate allocations; the compile-time form deduplicates by
/// pointer.
macro_rules! intent_colors {
    (
        $name:literal,
        $solid_bg:expr, $solid_text:expr,
        $soft_bg:expr, $soft_text:expr,
        $fg:expr, $border:expr $(,)?
    ) => {
        IntentColors {
            solid_bg: tok(concat!("intent-", $name, "-solid-bg"), $solid_bg),
            solid_text: tok(concat!("intent-", $name, "-solid-text"), $solid_text),
            soft_bg: tok(concat!("intent-", $name, "-soft-bg"), $soft_bg),
            soft_text: tok(concat!("intent-", $name, "-soft-text"), $soft_text),
            fg: tok(concat!("intent-", $name, "-fg"), $fg),
            border: tok(concat!("intent-", $name, "-border"), $border),
        }
    };
}

pub fn light_theme() -> IdeaThemeDefaults {
    IdeaThemeDefaults {
        colors: Colors {
            background: tok("color-background", "#f7f8fb"),
            surface: tok("color-surface", "#ffffff"),
            surface_alt: tok("color-surface-alt", "#eef0f7"),

            text: tok("color-text", "#1a1a1f"),
            text_muted: tok("color-text-muted", "#6b7280"),
            text_inverse: tok("color-text-inverse", "#ffffff"),

            border: tok("color-border", "#e4e6ef"),
            border_hover: tok("color-border-hover", "#b9bdcc"),
            border_strong: tok("color-border-strong", "#9097a8"),

            focus_ring: tok("color-focus-ring", "#5b6cff"),
            overlay: tok("color-overlay", "rgba(15, 17, 21, 0.45)"),
        },
        intents: Intents {
            // primary: indigo
            primary: intent_colors!(
                "primary",
                "#5b6cff", "#ffffff",
                "rgba(91, 108, 255, 0.12)", "#3947d6",
                "#3947d6", "#5b6cff",
            ),
            // secondary: slate gray
            secondary: intent_colors!(
                "secondary",
                "#475569", "#ffffff",
                "rgba(71, 85, 105, 0.10)", "#334155",
                "#334155", "#475569",
            ),
            // neutral: solid is near-black (think "Cancel" buttons);
            // soft is the surface-alt wash.
            neutral: intent_colors!(
                "neutral",
                "#1a1a1f", "#ffffff",
                "#eef0f7", "#1a1a1f",
                "#1a1a1f", "#e4e6ef",
            ),
            success: intent_colors!(
                "success",
                "#3ba55d", "#ffffff",
                "rgba(59, 165, 93, 0.12)", "#1f6e3a",
                "#1f6e3a", "#3ba55d",
            ),
            danger: intent_colors!(
                "danger",
                "#e5484d", "#ffffff",
                "rgba(229, 72, 77, 0.12)", "#a82127",
                "#a82127", "#e5484d",
            ),
            warning: intent_colors!(
                "warning",
                "#e0a82e", "#1a1a1f",
                "rgba(224, 168, 46, 0.16)", "#7a5810",
                "#7a5810", "#e0a82e",
            ),
            // info: cyan — visually distinct from primary's indigo.
            info: intent_colors!(
                "info",
                "#0ea5e9", "#ffffff",
                "rgba(14, 165, 233, 0.12)", "#065e85",
                "#065e85", "#0ea5e9",
            ),
        },
        spacing: DEFAULT_SPACING.clone(),
        radius: DEFAULT_RADIUS.clone(),
        typography: DEFAULT_TYPOGRAPHY.clone(),
    }
}

pub fn dark_theme() -> IdeaThemeDefaults {
    IdeaThemeDefaults {
        colors: Colors {
            background: tok("color-background", "#0f1115"),
            surface: tok("color-surface", "#1a1d24"),
            surface_alt: tok("color-surface-alt", "#262a35"),

            text: tok("color-text", "#e8eaf0"),
            text_muted: tok("color-text-muted", "#9099a8"),
            text_inverse: tok("color-text-inverse", "#0f1115"),

            border: tok("color-border", "#2a2e3a"),
            border_hover: tok("color-border-hover", "#3d4252"),
            border_strong: tok("color-border-strong", "#525868"),

            focus_ring: tok("color-focus-ring", "#8b9aff"),
            overlay: tok("color-overlay", "rgba(0, 0, 0, 0.55)"),
        },
        intents: Intents {
            primary: intent_colors!(
                "primary",
                "#8b9aff", "#0f1115",
                "rgba(139, 154, 255, 0.18)", "#b8c2ff",
                "#b8c2ff", "#8b9aff",
            ),
            secondary: intent_colors!(
                "secondary",
                "#64748b", "#ffffff",
                "rgba(148, 163, 184, 0.14)", "#cbd5e1",
                "#cbd5e1", "#64748b",
            ),
            neutral: intent_colors!(
                "neutral",
                "#e8eaf0", "#0f1115",
                "#262a35", "#e8eaf0",
                "#e8eaf0", "#2a2e3a",
            ),
            success: intent_colors!(
                "success",
                "#4cc77c", "#0f1115",
                "rgba(76, 199, 124, 0.16)", "#7fdfa1",
                "#7fdfa1", "#4cc77c",
            ),
            danger: intent_colors!(
                "danger",
                "#ff6369", "#ffffff",
                "rgba(255, 99, 105, 0.18)", "#ff9ba0",
                "#ff9ba0", "#ff6369",
            ),
            warning: intent_colors!(
                "warning",
                "#f0b942", "#0f1115",
                "rgba(240, 185, 66, 0.18)", "#f5cf7a",
                "#f5cf7a", "#f0b942",
            ),
            info: intent_colors!(
                "info",
                "#38bdf8", "#0f1115",
                "rgba(56, 189, 248, 0.16)", "#7dd0f5",
                "#7dd0f5", "#38bdf8",
            ),
        },
        spacing: DEFAULT_SPACING.clone(),
        radius: DEFAULT_RADIUS.clone(),
        typography: DEFAULT_TYPOGRAPHY.clone(),
    }
}

// =============================================================================
// Installation API
// =============================================================================

pub fn install_idea_theme<T: IdeaTheme>(theme: T) {
    install_theme(IdeaThemeRef::new(theme));
    install_default_idea_sheets();
}

pub fn set_idea_theme<T: IdeaTheme>(theme: T) {
    set_theme(IdeaThemeRef::new(theme));
}

/// Install the default stylesheets for every idea-ui component that
/// uses the extensible modifier system. Called from
/// [`install_idea_theme`] so apps that don't need custom modifiers
/// get a working setup with no extra calls.
///
/// Apps with custom modifiers (`Hype` tone, `Elevated` variant, etc.)
/// can override individual sheets by calling
/// `install_button_sheet(ButtonSheetBuilder::new().add_tone(Hype.into()).build())`
/// AFTER `install_idea_theme` returns.
pub fn install_default_idea_sheets() {
    crate::extensible::install_default_button_sheet();
    crate::extensible::install_default_badge_sheet();
    crate::extensible::install_default_tag_sheet();
    crate::extensible::install_default_alert_sheet();
    crate::extensible::install_default_typography_sheet();
    crate::extensible::install_default_icon_button_sheet();
    crate::extensible::install_default_switch_sheet();
    crate::extensible::install_default_checkbox_sheet();
    crate::extensible::install_default_radio_sheet();
    crate::extensible::install_default_progress_sheet();
}

/// Build a reactive color closure for a navigator's `header_*` /
/// `title_color` slot. The closure reads `active_theme()` on each
/// call, so it returns the *current* theme's color and the
/// surrounding Effect (set up by the navigator backend) re-fires on
/// theme swap — header bar and title re-tint without remounting.
///
/// ```ignore
/// Screen::new(page)
///     .header_background(idea_color(|c| c.surface.clone()))
///     .title_color(idea_color(|c| c.text.clone()))
///     .header_tint(idea_color(|c| c.text.clone()))
/// ```
pub fn idea_color<F>(getter: F) -> impl Fn() -> Color + 'static
where
    F: Fn(&Colors) -> Tokenized<Color> + 'static,
{
    move || {
        let theme = crate::theme_runtime::active_theme();
        let idea = theme
            .downcast_ref::<IdeaThemeRef>()
            .expect("idea_color: active theme is not an IdeaThemeRef — call install_idea_theme(...) first");
        getter(idea.colors()).value().clone()
    }
}

/// Build a reactive header-style closure for a navigator's bundled
/// `.header(...)` call. The closure is handed the current `IdeaTheme`
/// reference and returns the SDK's `HeaderStyle` (each SDK defines
/// its own — `stack_navigator::HeaderStyle`, `drawer_navigator::HeaderStyle`,
/// etc.). Re-invoked on every theme swap.
///
/// Generic over `HS` so authors can use `idea_header` with whichever
/// navigator SDK they're driving:
///
/// ```ignore
/// use drawer_navigator::HeaderStyle;
///
/// DrawerNavigator::new(&ROUTE)
///     .header(idea_header::<HeaderStyle, _>(|t| HeaderStyle {
///         background: Some(t.colors().surface.value().clone()),
///         title: Some(t.colors().text.value().clone()),
///         tint: Some(t.colors().text.value().clone()),
///         body_background: Some(t.colors().background.value().clone()),
///     }))
/// ```
pub fn idea_header<HS, F>(builder: F) -> impl Fn() -> HS + 'static
where
    HS: 'static,
    F: Fn(&IdeaThemeRef) -> HS + 'static,
{
    move || {
        let theme = crate::theme_runtime::active_theme();
        let idea = theme
            .downcast_ref::<IdeaThemeRef>()
            .expect("idea_header: active theme is not an IdeaThemeRef — call install_idea_theme(...) first");
        builder(idea)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_core::{Length, TokenValue};
    use crate::theme_runtime::ThemeTokens;

    /// Two theme impls with different spacing/radius/typography should
    /// emit different token values via the `ThemeTokens::tokens()`
    /// pipeline that `install_theme` / `set_theme` consume — that's
    /// the only contract the framework relies on to flow theme values
    /// into rendered stylesheets.
    fn find_length<'a>(toks: &'a [TokenEntry], name: &str) -> &'a Length {
        let entry = toks
            .iter()
            .find(|e| e.name == name)
            .unwrap_or_else(|| panic!("token '{}' not emitted", name));
        match &entry.value {
            TokenValue::Length(l) => l,
            other => panic!("token '{}' is not a Length ({:?})", name, other),
        }
    }

    fn px(l: &Length) -> f32 {
        match l {
            Length::Px(v) => *v,
            other => panic!("expected Px, got {:?}", other),
        }
    }

    fn make_theme(sm: f32, radius_md: f32, body_size: f32) -> IdeaThemeDefaults {
        let mut t = light_theme();
        t.spacing.sm = sm;
        t.radius.md = radius_md;
        t.typography.body_size = body_size;
        t
    }

    #[test]
    fn theme_tokens_include_spacing_radius_typography() {
        let theme = IdeaThemeRef::new(make_theme(8.0, 8.0, 14.0));
        let toks = theme.tokens();
        // Spacing.
        assert_eq!(px(find_length(&toks, "spacing-xs")), 4.0);
        assert_eq!(px(find_length(&toks, "spacing-sm")), 8.0);
        assert_eq!(px(find_length(&toks, "spacing-md")), 12.0);
        assert_eq!(px(find_length(&toks, "spacing-lg")), 16.0);
        assert_eq!(px(find_length(&toks, "spacing-xl")), 24.0);
        assert_eq!(px(find_length(&toks, "spacing-xxl")), 32.0);
        // Radius.
        assert_eq!(px(find_length(&toks, "radius-sm")), 4.0);
        assert_eq!(px(find_length(&toks, "radius-md")), 8.0);
        assert_eq!(px(find_length(&toks, "radius-lg")), 12.0);
        assert_eq!(px(find_length(&toks, "radius-pill")), 999.0);
        // Typography — one token per Typography variant.
        assert_eq!(px(find_length(&toks, "typography-display-size")), 56.0);
        assert_eq!(px(find_length(&toks, "typography-h1-size")), 36.0);
        assert_eq!(px(find_length(&toks, "typography-h2-size")), 28.0);
        assert_eq!(px(find_length(&toks, "typography-h3-size")), 20.0);
        assert_eq!(px(find_length(&toks, "typography-body-xl-size")), 20.0);
        assert_eq!(px(find_length(&toks, "typography-body-lg-size")), 18.0);
        assert_eq!(px(find_length(&toks, "typography-body-size")), 14.0);
        assert_eq!(px(find_length(&toks, "typography-body-sm-size")), 13.0);
        assert_eq!(px(find_length(&toks, "typography-caption-size")), 12.0);
        assert_eq!(px(find_length(&toks, "typography-overline-size")), 11.0);
    }

    #[test]
    fn custom_idea_theme_produces_different_tokens() {
        // Theme A: default-ish spacing/radius/typography.
        let a = IdeaThemeRef::new(make_theme(8.0, 8.0, 14.0));
        // Theme B: doubled values across all three categories.
        let b = IdeaThemeRef::new(make_theme(16.0, 16.0, 28.0));

        let a_toks = a.tokens();
        let b_toks = b.tokens();

        // Spacing.sm changes shows up in `spacing-sm`.
        assert_eq!(px(find_length(&a_toks, "spacing-sm")), 8.0);
        assert_eq!(px(find_length(&b_toks, "spacing-sm")), 16.0);

        // Radius.md changes shows up in `radius-md`.
        assert_eq!(px(find_length(&a_toks, "radius-md")), 8.0);
        assert_eq!(px(find_length(&b_toks, "radius-md")), 16.0);

        // Typography.body_size changes shows up in `typography-body-size`.
        assert_eq!(px(find_length(&a_toks, "typography-body-size")), 14.0);
        assert_eq!(px(find_length(&b_toks, "typography-body-size")), 28.0);
    }

    /// Regression test for the `intent_colors` `Box::leak` audit finding.
    /// Token names produced by `light_theme()` / `dark_theme()` must be
    /// compile-time `&'static str`s — repeated calls hand back the same
    /// pointer, not a freshly leaked allocation each time.
    ///
    /// 42 leaked strings per theme call × N theme constructions adds up
    /// over hot-reload / fixture-driven runs; with `concat!` the names
    /// are deduplicated by the compiler.
    #[test]
    fn intent_token_names_are_compile_time_constants() {
        let a = light_theme();
        let b = light_theme();
        let a_name = a.intents.primary.solid_bg.name().expect("token");
        let b_name = b.intents.primary.solid_bg.name().expect("token");
        assert_eq!(a_name, "intent-primary-solid-bg");
        assert_eq!(
            a_name.as_ptr(),
            b_name.as_ptr(),
            "intent-primary-solid-bg name must be the same compile-time &'static str \
             pointer across calls (Box::leak would produce different pointers)",
        );

        // Dark theme uses the same intent names — must share pointers
        // with light theme.
        let d = dark_theme();
        let d_name = d.intents.primary.solid_bg.name().expect("token");
        assert_eq!(
            a_name.as_ptr(),
            d_name.as_ptr(),
            "light and dark themes must share the compile-time name for \
             intent-primary-solid-bg",
        );
    }
}
