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
use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::{Color, Effect, FontFamily, Length, Tokenized};
use crate::theme_runtime::{install_theme, set_theme, ThemeTokens, TokenEntry, TokenValue};

thread_local! {
    /// Keepalive for [`install_idea_theme_reactive`]'s internal Effect when it's
    /// called outside a render scope (tests, top-level binaries). In an app the
    /// active scope owns the effect and this just holds an empty handle.
    /// Single-slot, so repeated installs supersede rather than leak — same
    /// posture as `theme_runtime::INSTALL_THEMES_KEEPALIVE`.
    static REACTIVE_THEME_KEEPALIVE: RefCell<Option<Effect>> = const { RefCell::new(None) };
}

/// The default body font for every idea-ui text surface.
///
/// This is a **system-sans stack**, not a bundled face. Its single
/// most important job is correctness on the **web** backend: a browser
/// with no `font-family` set falls back to its serif default (Times),
/// so a stock idea-ui app would render in serif. Naming a sans stack
/// here makes a fresh app render in the platform's UI sans on web,
/// matching native (where `UILabel`/`TextView` already default to a
/// system sans). The stack walks modern UI fonts on each OS before the
/// generic `sans-serif`, so it's a safe default everywhere and pulls in
/// no font binary.
///
/// Apps wanting a brand face override [`IdeaTheme::font_family`] (via
/// the `font` field on [`IdeaThemeDefaults`], the [`app_theme!`] wrapper,
/// or a hand-rolled `IdeaTheme` impl) with a registered
/// [`Typeface`](runtime_core::Typeface) or their own family string.
pub const DEFAULT_FONT_STACK: &str =
    "system-ui, -apple-system, BlinkMacSystemFont, \"Segoe UI\", Roboto, \
     \"Helvetica Neue\", Arial, sans-serif";

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

    /// The theme's default body font family. Applied as the base
    /// `font_family` on every idea-ui text surface (Typography, and
    /// any component whose sheet pulls it in), so a fresh app renders
    /// in a sans face instead of the browser serif fallback on web.
    ///
    /// Returns a [`FontFamily`] — either a free-form system stack
    /// (the default, [`DEFAULT_FONT_STACK`]) or a registered
    /// [`Typeface`](runtime_core::Typeface). The framework registers a
    /// `Typeface` with the backend on first observation; a `System`
    /// stack is passed verbatim to the platform font lookup.
    ///
    /// The default returns [`DEFAULT_FONT_STACK`]. Custom themes
    /// override this to ship a brand face.
    fn font_family(&self) -> FontFamily {
        FontFamily::System(DEFAULT_FONT_STACK.to_string())
    }
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
    fn font_family(&self) -> FontFamily {
        self.inner.font_family()
    }
}

// =============================================================================
// Canonical token names — the single source of truth
// =============================================================================
//
// idea-ui's stylesheets resolve theme colors by these *canonical* names
// (e.g. `Tokenized::token("color-surface", ..)` in `idea_ui::stylesheets`).
// They are the keys the token registry must be populated under, so they
// drive `ThemeTokens::tokens()` (the install path) below. They're also the
// only names `idea_color`/`color_token!`-built overrides effectively need:
// the value an override carries is what matters; its `name` argument is
// cosmetic because install keys off the field's canonical name regardless.

/// Canonical token names for the non-intent neutral colors, in the same
/// field order as [`Colors`]. The Nth entry is the canonical key for the
/// Nth `Colors` field.
pub const CANONICAL_NEUTRAL_TOKENS: [&str; 11] = [
    "color-background",
    "color-surface",
    "color-surface-alt",
    "color-text",
    "color-text-muted",
    "color-text-inverse",
    "color-border",
    "color-border-hover",
    "color-border-strong",
    "color-focus-ring",
    "color-overlay",
];

/// The seven built-in intent names, in [`Intents`] field order.
pub const INTENT_NAMES: [&str; 7] =
    ["primary", "secondary", "neutral", "success", "danger", "warning", "info"];

/// The six slot suffixes on each intent, in [`IntentColors`] field order.
/// Canonical intent token = `intent-<intent>-<slot>`.
pub const INTENT_SLOTS: [&str; 6] =
    ["solid-bg", "solid-text", "soft-bg", "soft-text", "fg", "border"];

/// Map an (`intent`, `slot`) pair to its canonical `&'static str` token
/// name (`intent-<intent>-<slot>`). Returns a static string for every
/// built-in combination; panics on an unknown pair (only reachable from a
/// programming error inside this crate, never from author input).
///
/// A `match` over the fixed combinations avoids `Box::leak`/`format!`
/// allocation on the install hot path — the same compile-time-dedup
/// rationale as `intent_colors!`'s `concat!`.
fn intent_slot_token(intent: &str, slot: &str) -> &'static str {
    macro_rules! arm {
        ($i:literal, $s:literal) => {
            (concat!($i, "/", $s), concat!("intent-", $i, "-", $s))
        };
    }
    // (key, canonical) for every intent × slot. `key` is "intent/slot".
    const TABLE: &[(&str, &str)] = &[
        arm!("primary", "solid-bg"), arm!("primary", "solid-text"),
        arm!("primary", "soft-bg"), arm!("primary", "soft-text"),
        arm!("primary", "fg"), arm!("primary", "border"),
        arm!("secondary", "solid-bg"), arm!("secondary", "solid-text"),
        arm!("secondary", "soft-bg"), arm!("secondary", "soft-text"),
        arm!("secondary", "fg"), arm!("secondary", "border"),
        arm!("neutral", "solid-bg"), arm!("neutral", "solid-text"),
        arm!("neutral", "soft-bg"), arm!("neutral", "soft-text"),
        arm!("neutral", "fg"), arm!("neutral", "border"),
        arm!("success", "solid-bg"), arm!("success", "solid-text"),
        arm!("success", "soft-bg"), arm!("success", "soft-text"),
        arm!("success", "fg"), arm!("success", "border"),
        arm!("danger", "solid-bg"), arm!("danger", "solid-text"),
        arm!("danger", "soft-bg"), arm!("danger", "soft-text"),
        arm!("danger", "fg"), arm!("danger", "border"),
        arm!("warning", "solid-bg"), arm!("warning", "solid-text"),
        arm!("warning", "soft-bg"), arm!("warning", "soft-text"),
        arm!("warning", "fg"), arm!("warning", "border"),
        arm!("info", "solid-bg"), arm!("info", "solid-text"),
        arm!("info", "soft-bg"), arm!("info", "soft-text"),
        arm!("info", "fg"), arm!("info", "border"),
    ];
    let key_len = intent.len() + 1 + slot.len();
    for (key, canonical) in TABLE {
        // Cheap pre-filter on length, then compare the "intent/slot" key.
        if key.len() == key_len
            && key.as_bytes()[intent.len()] == b'/'
            && &key[..intent.len()] == intent
            && &key[intent.len() + 1..] == slot
        {
            return canonical;
        }
    }
    panic!("intent_slot_token: unknown intent/slot pair '{intent}/{slot}'");
}

/// Is `name` the canonical token key for an idea-ui theme color/length
/// field? This is the predicate that distinguishes a name idea-ui will
/// actually resolve from a free-form name that nothing reads.
///
/// The install path ([`ThemeTokens::tokens`]) no longer *depends* on the
/// author using a canonical name — it keys every field off its fixed
/// canonical name regardless — so a non-canonical `color_token!` override
/// now WORKS instead of silently no-opping. This predicate exists for
/// tooling/lints (and the regression test) that want to flag a token name
/// the registry would otherwise ignore.
pub fn is_canonical_token(name: &str) -> bool {
    if CANONICAL_NEUTRAL_TOKENS.contains(&name) {
        return true;
    }
    // intent-<intent>-<slot>
    if let Some(rest) = name.strip_prefix("intent-") {
        return INTENT_NAMES.iter().any(|intent| {
            rest.strip_prefix(intent)
                .and_then(|r| r.strip_prefix('-'))
                .is_some_and(|slot| INTENT_SLOTS.contains(&slot))
        });
    }
    // Spacing / radius / typography length tokens.
    matches!(
        name,
        "spacing-xs" | "spacing-sm" | "spacing-md" | "spacing-lg" | "spacing-xl" | "spacing-xxl"
            | "radius-sm" | "radius-md" | "radius-lg" | "radius-pill"
            | "typography-display-size" | "typography-h1-size" | "typography-h2-size"
            | "typography-h3-size" | "typography-body-xl-size" | "typography-body-lg-size"
            | "typography-body-size" | "typography-body-sm-size" | "typography-caption-size"
            | "typography-overline-size"
    )
}

impl ThemeTokens for IdeaThemeRef {
    fn tokens(&self) -> Vec<TokenEntry> {
        let c = self.colors();
        let i = self.intents();
        let s = self.spacing();
        let r = self.radius();
        let ty = self.typography();
        // Register each color VALUE under the CANONICAL token name for
        // its field — *not* the free-form name the author passed to
        // `color_token!`. idea-ui's stylesheets reference colors by the
        // canonical name (`color-surface`, `intent-primary-solid-bg`, …,
        // see `CANONICAL_NEUTRAL_TOKENS` / `INTENT_SLOT_TOKENS`), and the
        // runtime resolves a `Tokenized::Token { name, .. }` by that name.
        //
        // Why ignore `t.name()`: a rebrand mutates theme fields with
        // `t.colors.surface = color_token!("ok-surface", "#fff")`. If we
        // registered under `"ok-surface"` (the field's `.name()`), the
        // value would land under a key nothing reads and the override
        // would silently no-op — the whole theme quietly falls back to
        // idea-ui defaults. Keying off the field's fixed canonical name
        // makes the override take effect regardless of the `name`
        // argument, eliminating that footgun (see field report +
        // `is_canonical_token`).
        fn entry(name: &'static str, t: &Tokenized<Color>) -> TokenEntry {
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
        // Helper: emit every field of an IntentColors block under the
        // intent's canonical slot names.
        fn intent_entries(intent: &str, ic: &IntentColors, out: &mut Vec<TokenEntry>) {
            for (slot, value) in [
                ("solid-bg", &ic.solid_bg),
                ("solid-text", &ic.solid_text),
                ("soft-bg", &ic.soft_bg),
                ("soft-text", &ic.soft_text),
                ("fg", &ic.fg),
                ("border", &ic.border),
            ] {
                out.push(TokenEntry {
                    name: intent_slot_token(intent, slot),
                    value: TokenValue::Color(value.value().clone()),
                });
            }
        }
        let mut out = vec![
            entry("color-background", &c.background),
            entry("color-surface", &c.surface),
            entry("color-surface-alt", &c.surface_alt),
            entry("color-text", &c.text),
            entry("color-text-muted", &c.text_muted),
            entry("color-text-inverse", &c.text_inverse),
            entry("color-border", &c.border),
            entry("color-border-hover", &c.border_hover),
            entry("color-border-strong", &c.border_strong),
            entry("color-focus-ring", &c.focus_ring),
            entry("color-overlay", &c.overlay),
        ];
        intent_entries("primary", &i.primary, &mut out);
        intent_entries("secondary", &i.secondary, &mut out);
        intent_entries("neutral", &i.neutral, &mut out);
        intent_entries("success", &i.success, &mut out);
        intent_entries("danger", &i.danger, &mut out);
        intent_entries("warning", &i.warning, &mut out);
        intent_entries("info", &i.info, &mut out);

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
    /// The default body font family. Defaults to a system-sans stack
    /// ([`DEFAULT_FONT_STACK`]) so web text isn't serif; swap in a
    /// `FontFamily::Typeface(...)` (built via `typeface!`) for a brand
    /// face, or another system stack string.
    pub font: FontFamily,
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
    fn font_family(&self) -> FontFamily {
        self.font.clone()
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
        font: FontFamily::System(DEFAULT_FONT_STACK.to_string()),
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
        font: FontFamily::System(DEFAULT_FONT_STACK.to_string()),
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

/// Install an idea theme whose choice is **reactive**: `select` re-runs whenever
/// a signal it reads changes, swapping the active theme — so a dark-mode toggle
/// (or any signal-driven theme) needs no hand-rolled `effect!` that calls
/// [`set_idea_theme`].
///
/// ```ignore
/// let dark = signal!(false);
/// install_idea_theme_reactive(move || if dark.get() { dark_theme() } else { light_theme() });
/// // flipping `dark` now re-themes the whole app.
/// ```
///
/// This is the closure-driven peer of [`install_themes`](crate::install_themes)
/// (which is keyed by a `Signal<String>`): reach for it when the source of truth
/// is a `bool`/enum signal you also read elsewhere, so you don't need a parallel
/// string signal. Component sheets are installed once up front; the internal
/// effect's first run applies the initial theme (subscribing to whatever `select`
/// reads) and later runs swap it.
pub fn install_idea_theme_reactive<T, F>(select: F)
where
    T: IdeaTheme + 'static,
    F: FnMut() -> T + 'static,
{
    install_default_idea_sheets();
    let mut select = select;
    let mut primed = false;
    let effect = Effect::new(move || {
        let theme = IdeaThemeRef::new(select());
        if primed {
            set_theme(theme);
        } else {
            primed = true;
            install_theme(theme);
        }
    });
    REACTIVE_THEME_KEEPALIVE.with(|k| *k.borrow_mut() = Some(effect));
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
    use runtime_core::{FontFamily, Length, Signal, TokenValue};
    use crate::theme_runtime::{active_theme_untracked, ThemeTokens};

    /// The active theme's `color-background` token value (the cheapest
    /// light-vs-dark discriminant) — read untracked so the test isn't itself a
    /// reactive subscriber.
    fn active_background() -> Color {
        let theme = active_theme_untracked();
        let idea = theme.downcast_ref::<IdeaThemeRef>().expect("active theme is an IdeaThemeRef");
        idea.tokens()
            .into_iter()
            .find(|e| e.name == "color-background")
            .and_then(|e| match e.value {
                TokenValue::Color(c) => Some(c),
                _ => None,
            })
            .expect("color-background token present")
    }

    fn theme_background(t: IdeaThemeDefaults) -> Color {
        IdeaThemeRef::new(t)
            .tokens()
            .into_iter()
            .find(|e| e.name == "color-background")
            .and_then(|e| match e.value {
                TokenValue::Color(c) => Some(c),
                _ => None,
            })
            .expect("color-background token present")
    }

    /// `install_idea_theme_reactive` applies the initial theme AND re-applies
    /// when a signal its selector reads changes — without the app hand-rolling an
    /// `effect!` that calls `set_idea_theme`. (`Signal::set` runs subscribed
    /// effects synchronously, so no flush is needed.)
    #[test]
    fn reactive_theme_swaps_when_its_signal_flips() {
        let light_bg = theme_background(light_theme());
        let dark_bg = theme_background(dark_theme());
        assert_ne!(light_bg.0, dark_bg.0, "light/dark backgrounds must differ for this test");

        let dark = Signal::new(false); // Copy: a clone moves into the selector
        install_idea_theme_reactive(move || if dark.get() { dark_theme() } else { light_theme() });
        assert_eq!(active_background().0, light_bg.0, "initial theme is light");

        dark.set(true);
        assert_eq!(active_background().0, dark_bg.0, "flipping the signal re-themes");

        dark.set(false);
        assert_eq!(active_background().0, light_bg.0, "and flips back");

        // Free the effect's arena slot before thread teardown (see the
        // INSTALL_THEMES_KEEPALIVE test for why).
        super::REACTIVE_THEME_KEEPALIVE.with(|k| *k.borrow_mut() = None);
    }

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

    /// Field report 3.1(b): a fresh idea-ui app must NOT render in the
    /// browser's serif fallback. The default theme's `font_family()`
    /// has to be a non-empty *sans* system stack — empty / serif here is
    /// the exact bug. This guards the default both ways: it's a `System`
    /// stack (not unset) and it ends in `sans-serif`, not a serif family.
    #[test]
    fn default_theme_font_is_non_empty_sans_not_serif() {
        for theme in [light_theme(), dark_theme()] {
            match theme.font_family() {
                FontFamily::System(stack) => {
                    assert!(
                        !stack.trim().is_empty(),
                        "default font stack must be non-empty (empty ⇒ browser serif fallback)"
                    );
                    assert!(
                        stack.contains("sans-serif"),
                        "default font stack must resolve to a sans family, got: {stack}"
                    );
                    assert!(
                        !stack.contains("serif,") && !stack.ends_with("serif")
                            || stack.ends_with("sans-serif"),
                        "default font stack must not fall back to serif, got: {stack}"
                    );
                }
                FontFamily::Typeface(_) => {
                    // A bundled sans typeface would also satisfy the
                    // "not serif" intent; the current default is a
                    // system stack, so flag if that changes silently.
                    panic!("default theme font is a Typeface; update this test if intentional");
                }
            }
        }
    }

    /// Field report 3.1(a): a custom theme can carry its own font. The
    /// `font` field flows through `IdeaTheme::font_family()`, including
    /// when the theme is wrapped in the framework's `IdeaThemeRef`
    /// carrier (the type stylesheets actually downcast to).
    #[test]
    fn custom_font_flows_through_theme_and_ref() {
        let mut t = light_theme();
        t.font = FontFamily::System("Courier New, monospace".to_string());
        match t.font_family() {
            FontFamily::System(s) => assert_eq!(s, "Courier New, monospace"),
            _ => panic!("expected System font"),
        }
        // Through the IdeaThemeRef carrier (what install_idea_theme wraps in).
        let r = IdeaThemeRef::new(t);
        match r.font_family() {
            FontFamily::System(s) => assert_eq!(s, "Courier New, monospace"),
            _ => panic!("expected System font through IdeaThemeRef"),
        }
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

    fn find_color<'a>(toks: &'a [TokenEntry], name: &str) -> &'a runtime_core::Color {
        let entry = toks
            .iter()
            .find(|e| e.name == name)
            .unwrap_or_else(|| panic!("token '{}' not emitted", name));
        match &entry.value {
            TokenValue::Color(c) => c,
            other => panic!("token '{}' is not a Color ({:?})", name, other),
        }
    }

    /// Regression for the silent-no-op footgun: a rebrand override written
    /// with a NON-canonical token name (the `color_token!` first arg is
    /// free-form) must still take effect. `install_idea_theme` keys the
    /// install registry off each field's CANONICAL name, so the override's
    /// VALUE lands under `color-surface` / `intent-primary-solid-bg` — the
    /// keys idea-ui's stylesheets actually resolve — regardless of the
    /// bogus name. Before the fix the value was registered under the bogus
    /// name and the whole theme silently fell back to idea-ui defaults.
    #[test]
    fn noncanonical_override_name_still_takes_effect() {
        let mut t = light_theme();
        // Deliberately WRONG names — what a footgunned author would write.
        t.colors.surface = Tokenized::token("ok-surface", Color("#abcdef".into()));
        t.intents.primary.solid_bg = Tokenized::token("ok-primary-bg", Color("#3f73e3".into()));

        let toks = IdeaThemeRef::new(t).tokens();

        // The value must be reachable under the CANONICAL key idea-ui reads…
        assert_eq!(
            find_color(&toks, "color-surface").0,
            "#abcdef",
            "non-canonical surface override must register under canonical 'color-surface'"
        );
        assert_eq!(
            find_color(&toks, "intent-primary-solid-bg").0,
            "#3f73e3",
            "non-canonical intent override must register under canonical \
             'intent-primary-solid-bg'"
        );
        // …and NOT under the bogus name the author typed (nothing reads it).
        assert!(
            !toks.iter().any(|e| e.name == "ok-surface" || e.name == "ok-primary-bg"),
            "the free-form color_token! name must not appear as a registered key — \
             install keys off the field's canonical name"
        );
    }

    /// `is_canonical_token` is the pure decision fn behind the footgun:
    /// it classifies which names idea-ui will actually resolve. The
    /// canonical names a theme emits must all pass; the free-form names an
    /// author might pass to `color_token!` must fail.
    #[test]
    fn is_canonical_token_classifies_emitted_names() {
        // Every name the default theme emits is canonical by construction.
        for entry in IdeaThemeRef::new(light_theme()).tokens() {
            assert!(
                is_canonical_token(entry.name),
                "emitted token '{}' must be classified canonical",
                entry.name
            );
        }
        // Spot-check representative canonical names across categories.
        for name in [
            "color-surface",
            "color-focus-ring",
            "intent-primary-solid-bg",
            "intent-info-border",
            "spacing-md",
            "radius-pill",
            "typography-body-size",
        ] {
            assert!(is_canonical_token(name), "'{name}' should be canonical");
        }
        // Free-form / typo'd names must be rejected.
        for name in [
            "ok-surface",
            "ok-primary-bg",
            "surface",
            "color-surfce",
            "intent-primary-solidbg",
            "intent-bogus-fg",
            "intent-primary-glow",
        ] {
            assert!(!is_canonical_token(name), "'{name}' should NOT be canonical");
        }
    }
}
