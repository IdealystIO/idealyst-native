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
//! Two ways to use it:
//!
//! ```ignore
//! // 90% case — install the defaults:
//! install_idea_theme(light_theme());
//!
//! // Extension case — implement IdeaTheme on your own struct:
//! struct MyTheme {
//!     base: IdeaThemeDefaults,
//!     brand: BrandColors,
//! }
//!
//! impl IdeaTheme for MyTheme {
//!     fn colors(&self) -> &Colors    { self.base.colors() }
//!     fn spacing(&self) -> &Spacing  { self.base.spacing() }
//!     fn radius(&self) -> &Radius    { self.base.radius() }
//!     fn typography(&self) -> &Typography { self.base.typography() }
//! }
//!
//! install_idea_theme(MyTheme {
//!     base: light_theme(),
//!     brand: BrandColors { hype: "#ff00aa".into() },
//! });
//! ```
//!
//! The trait only exposes the *minimum* idea-ui's own stylesheets
//! need. Apps that add fields keep them on the concrete type and
//! reach for them in their own stylesheets / components — those
//! don't need to go through the trait.

use std::any::Any;
use std::rc::Rc;

use framework_core::{
    install_theme, set_theme, Color, ThemeTokens, TokenEntry, TokenValue, Tokenized,
};

// =============================================================================
// Tokens — concrete value structs the trait exposes
// =============================================================================

/// Each color is a `Tokenized<Color>` — a `Token` reference whose
/// fallback is the current theme's literal value. Stylesheets close
/// over these directly (`t.colors().primary.clone()` returns a
/// `Tokenized<Color>`); the resulting `StyleRules` carry the token
/// name into the content key, so two themes that bind `color-primary`
/// to different colors produce the **same** minted CSS class. Theme
/// swap then only writes to the `:root` declaration block — no
/// `className` mutation on any node.
#[derive(Clone)]
pub struct Colors {
    pub background: Tokenized<Color>,
    pub surface: Tokenized<Color>,
    pub surface_alt: Tokenized<Color>,

    pub primary: Tokenized<Color>,
    pub primary_hover: Tokenized<Color>,
    pub primary_pressed: Tokenized<Color>,
    pub primary_text: Tokenized<Color>,

    pub danger: Tokenized<Color>,
    pub danger_hover: Tokenized<Color>,
    pub danger_pressed: Tokenized<Color>,
    pub danger_text: Tokenized<Color>,

    pub success: Tokenized<Color>,
    pub warning: Tokenized<Color>,

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
///
/// The trait deliberately exposes only the minimum surface — colors,
/// spacing, radius, typography. App-level extensions (custom intent
/// palettes, chart colors, etc.) live on the concrete type and are
/// accessed through the concrete type, not through this trait.
pub trait IdeaTheme: Any + 'static {
    fn colors(&self) -> &Colors;
    fn spacing(&self) -> &Spacing;
    fn radius(&self) -> &Radius;
    fn typography(&self) -> &Typography;
}

// =============================================================================
// IdeaThemeRef — the framework-side concrete carrier
// =============================================================================

/// Wrapper that gives the framework's `Rc<dyn Any>` theme storage a
/// concrete type to downcast to. Stylesheets close over
/// `IdeaThemeRef`; inside the closure, `.colors()` / `.spacing()`
/// dispatch through the inner trait object.
///
/// You almost never construct this directly — use
/// [`install_idea_theme`] / [`set_idea_theme`].
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
        // Helper: pull the static name + concrete value out of a
        // theme-field reference. Themes always populate fields with
        // `Tokenized::Token { .. }`; if a literal slipped in we'd get
        // a panic here flagging the misuse.
        fn entry(t: &Tokenized<Color>) -> TokenEntry {
            let name = t.name().expect("Colors fields must be Tokenized::Token");
            TokenEntry {
                name,
                value: TokenValue::Color(t.value().clone()),
            }
        }
        vec![
            entry(&c.background),
            entry(&c.surface),
            entry(&c.surface_alt),
            entry(&c.primary),
            entry(&c.primary_hover),
            entry(&c.primary_pressed),
            entry(&c.primary_text),
            entry(&c.danger),
            entry(&c.danger_hover),
            entry(&c.danger_pressed),
            entry(&c.danger_text),
            entry(&c.success),
            entry(&c.warning),
            entry(&c.text),
            entry(&c.text_muted),
            entry(&c.text_inverse),
            entry(&c.border),
            entry(&c.border_hover),
            entry(&c.border_strong),
            entry(&c.focus_ring),
            entry(&c.overlay),
        ]
    }
}

// =============================================================================
// Default theme implementation
// =============================================================================

/// The concrete struct idea-ui's `light_theme()` / `dark_theme()`
/// return. Apps that need extra fields wrap an `IdeaThemeDefaults`
/// inside their own struct and forward the trait getters.
#[derive(Clone)]
pub struct IdeaThemeDefaults {
    pub colors: Colors,
    pub spacing: Spacing,
    pub radius: Radius,
    pub typography: Typography,
}

impl IdeaTheme for IdeaThemeDefaults {
    fn colors(&self) -> &Colors {
        &self.colors
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

/// Helper: build a `Tokenized<Color>` reference with the given token
/// name and a string fallback. Centralizes the `Color(_.into())` boilerplate.
fn tok_color(name: &'static str, fallback: &str) -> Tokenized<Color> {
    Tokenized::token(name, Color(fallback.into()))
}

pub fn light_theme() -> IdeaThemeDefaults {
    IdeaThemeDefaults {
        colors: Colors {
            background: tok_color("color-background", "#f7f8fb"),
            surface: tok_color("color-surface", "#ffffff"),
            surface_alt: tok_color("color-surface-alt", "#eef0f7"),

            primary: tok_color("color-primary", "#5b6cff"),
            primary_hover: tok_color("color-primary-hover", "#4a5cf0"),
            primary_pressed: tok_color("color-primary-pressed", "#3947d6"),
            primary_text: tok_color("color-primary-text", "#ffffff"),

            danger: tok_color("color-danger", "#e5484d"),
            danger_hover: tok_color("color-danger-hover", "#d63d42"),
            danger_pressed: tok_color("color-danger-pressed", "#bc2f33"),
            danger_text: tok_color("color-danger-text", "#ffffff"),

            success: tok_color("color-success", "#3ba55d"),
            warning: tok_color("color-warning", "#e0a82e"),

            text: tok_color("color-text", "#1a1a1f"),
            text_muted: tok_color("color-text-muted", "#6b7280"),
            text_inverse: tok_color("color-text-inverse", "#ffffff"),

            border: tok_color("color-border", "#e4e6ef"),
            border_hover: tok_color("color-border-hover", "#b9bdcc"),
            border_strong: tok_color("color-border-strong", "#9097a8"),

            focus_ring: tok_color("color-focus-ring", "#5b6cff"),
            overlay: tok_color("color-overlay", "rgba(15, 17, 21, 0.45)"),
        },
        spacing: DEFAULT_SPACING.clone(),
        radius: DEFAULT_RADIUS.clone(),
        typography: DEFAULT_TYPOGRAPHY.clone(),
    }
}

pub fn dark_theme() -> IdeaThemeDefaults {
    IdeaThemeDefaults {
        colors: Colors {
            background: tok_color("color-background", "#0f1115"),
            surface: tok_color("color-surface", "#1a1d24"),
            surface_alt: tok_color("color-surface-alt", "#262a35"),

            primary: tok_color("color-primary", "#8b9aff"),
            primary_hover: tok_color("color-primary-hover", "#9eabff"),
            primary_pressed: tok_color("color-primary-pressed", "#7383f5"),
            primary_text: tok_color("color-primary-text", "#0f1115"),

            danger: tok_color("color-danger", "#ff6369"),
            danger_hover: tok_color("color-danger-hover", "#ff7a80"),
            danger_pressed: tok_color("color-danger-pressed", "#e5484d"),
            danger_text: tok_color("color-danger-text", "#ffffff"),

            success: tok_color("color-success", "#4cc77c"),
            warning: tok_color("color-warning", "#f0b942"),

            text: tok_color("color-text", "#e8eaf0"),
            text_muted: tok_color("color-text-muted", "#9099a8"),
            text_inverse: tok_color("color-text-inverse", "#0f1115"),

            border: tok_color("color-border", "#2a2e3a"),
            border_hover: tok_color("color-border-hover", "#3d4252"),
            border_strong: tok_color("color-border-strong", "#525868"),

            focus_ring: tok_color("color-focus-ring", "#8b9aff"),
            overlay: tok_color("color-overlay", "rgba(0, 0, 0, 0.55)"),
        },
        spacing: DEFAULT_SPACING.clone(),
        radius: DEFAULT_RADIUS.clone(),
        typography: DEFAULT_TYPOGRAPHY.clone(),
    }
}

// =============================================================================
// Installation API
// =============================================================================

/// Install the given theme as the framework's active theme. Wraps it
/// in [`IdeaThemeRef`] so idea-ui stylesheets can downcast to one
/// concrete type regardless of which `IdeaTheme` impl the app uses.
pub fn install_idea_theme<T: IdeaTheme>(theme: T) {
    install_theme(IdeaThemeRef::new(theme));
}

/// Swap the active theme at runtime. Every styled effect re-fires;
/// no rebuild.
pub fn set_idea_theme<T: IdeaTheme>(theme: T) {
    set_theme(IdeaThemeRef::new(theme));
}
