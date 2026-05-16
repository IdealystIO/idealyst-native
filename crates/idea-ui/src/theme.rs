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

use framework_core::{
    install_theme, set_theme, Color, ThemeTokens, TokenEntry, TokenValue, Tokenized,
};

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

#[derive(Clone)]
pub struct Typography {
    pub size_xs: f32,
    pub size_sm: f32,
    pub size_md: f32,
    pub size_lg: f32,
    pub size_xl: f32,
    pub size_xxl: f32,
    pub size_display: f32,
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
        fn entry(t: &Tokenized<Color>) -> TokenEntry {
            let name = t.name().expect("color fields must be Tokenized::Token");
            TokenEntry {
                name,
                value: TokenValue::Color(t.value().clone()),
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
    size_xs: 11.0,
    size_sm: 12.0,
    size_md: 14.0,
    size_lg: 16.0,
    size_xl: 20.0,
    size_xxl: 28.0,
    size_display: 36.0,
};

fn tok(name: &'static str, fallback: &str) -> Tokenized<Color> {
    Tokenized::token(name, Color(fallback.into()))
}

/// Helper: build an `IntentColors` from the 6 fields, with token
/// names auto-derived from the intent name (e.g. `"primary"` → tokens
/// `intent-primary-solid-bg`, `intent-primary-solid-text`, …).
///
/// Order: `solid_bg, solid_text, soft_bg, soft_text, fg, border`.
fn intent_colors(
    name: &'static str,
    solid_bg: &'static str,
    solid_text: &'static str,
    soft_bg: &'static str,
    soft_text: &'static str,
    fg: &'static str,
    border: &'static str,
) -> IntentColors {
    // Leak the token names as `'static str` — each theme function is
    // called at most once per app lifetime, and the resulting tokens
    // are referenced for the lifetime of the program anyway. Cheap.
    fn tn(s: String) -> &'static str {
        Box::leak(s.into_boxed_str())
    }
    IntentColors {
        solid_bg: tok(tn(format!("intent-{}-solid-bg", name)), solid_bg),
        solid_text: tok(tn(format!("intent-{}-solid-text", name)), solid_text),
        soft_bg: tok(tn(format!("intent-{}-soft-bg", name)), soft_bg),
        soft_text: tok(tn(format!("intent-{}-soft-text", name)), soft_text),
        fg: tok(tn(format!("intent-{}-fg", name)), fg),
        border: tok(tn(format!("intent-{}-border", name)), border),
    }
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
            primary: intent_colors(
                "primary",
                "#5b6cff", "#ffffff",
                "rgba(91, 108, 255, 0.12)", "#3947d6",
                "#3947d6", "#5b6cff",
            ),
            // secondary: slate gray
            secondary: intent_colors(
                "secondary",
                "#475569", "#ffffff",
                "rgba(71, 85, 105, 0.10)", "#334155",
                "#334155", "#475569",
            ),
            // neutral: solid is near-black (think "Cancel" buttons);
            // soft is the surface-alt wash.
            neutral: intent_colors(
                "neutral",
                "#1a1a1f", "#ffffff",
                "#eef0f7", "#1a1a1f",
                "#1a1a1f", "#e4e6ef",
            ),
            success: intent_colors(
                "success",
                "#3ba55d", "#ffffff",
                "rgba(59, 165, 93, 0.12)", "#1f6e3a",
                "#1f6e3a", "#3ba55d",
            ),
            danger: intent_colors(
                "danger",
                "#e5484d", "#ffffff",
                "rgba(229, 72, 77, 0.12)", "#a82127",
                "#a82127", "#e5484d",
            ),
            warning: intent_colors(
                "warning",
                "#e0a82e", "#1a1a1f",
                "rgba(224, 168, 46, 0.16)", "#7a5810",
                "#7a5810", "#e0a82e",
            ),
            // info: cyan — visually distinct from primary's indigo.
            info: intent_colors(
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
            primary: intent_colors(
                "primary",
                "#8b9aff", "#0f1115",
                "rgba(139, 154, 255, 0.18)", "#b8c2ff",
                "#b8c2ff", "#8b9aff",
            ),
            secondary: intent_colors(
                "secondary",
                "#64748b", "#ffffff",
                "rgba(148, 163, 184, 0.14)", "#cbd5e1",
                "#cbd5e1", "#64748b",
            ),
            neutral: intent_colors(
                "neutral",
                "#e8eaf0", "#0f1115",
                "#262a35", "#e8eaf0",
                "#e8eaf0", "#2a2e3a",
            ),
            success: intent_colors(
                "success",
                "#4cc77c", "#0f1115",
                "rgba(76, 199, 124, 0.16)", "#7fdfa1",
                "#7fdfa1", "#4cc77c",
            ),
            danger: intent_colors(
                "danger",
                "#ff6369", "#ffffff",
                "rgba(255, 99, 105, 0.18)", "#ff9ba0",
                "#ff9ba0", "#ff6369",
            ),
            warning: intent_colors(
                "warning",
                "#f0b942", "#0f1115",
                "rgba(240, 185, 66, 0.18)", "#f5cf7a",
                "#f5cf7a", "#f0b942",
            ),
            info: intent_colors(
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
}

pub fn set_idea_theme<T: IdeaTheme>(theme: T) {
    set_theme(IdeaThemeRef::new(theme));
}
