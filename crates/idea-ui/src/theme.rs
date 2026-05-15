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

use framework_core::{install_theme, set_theme};

// =============================================================================
// Tokens — concrete value structs the trait exposes
// =============================================================================

#[derive(Clone)]
pub struct Colors {
    pub background: String,
    pub surface: String,
    pub surface_alt: String,

    pub primary: String,
    pub primary_hover: String,
    pub primary_pressed: String,
    pub primary_text: String,

    pub danger: String,
    pub danger_hover: String,
    pub danger_pressed: String,
    pub danger_text: String,

    pub success: String,
    pub warning: String,

    pub text: String,
    pub text_muted: String,
    pub text_inverse: String,

    pub border: String,
    pub border_hover: String,
    pub border_strong: String,

    pub focus_ring: String,
    pub overlay: String,
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

pub fn light_theme() -> IdeaThemeDefaults {
    IdeaThemeDefaults {
        colors: Colors {
            background: "#f7f8fb".into(),
            surface: "#ffffff".into(),
            surface_alt: "#eef0f7".into(),

            primary: "#5b6cff".into(),
            primary_hover: "#4a5cf0".into(),
            primary_pressed: "#3947d6".into(),
            primary_text: "#ffffff".into(),

            danger: "#e5484d".into(),
            danger_hover: "#d63d42".into(),
            danger_pressed: "#bc2f33".into(),
            danger_text: "#ffffff".into(),

            success: "#3ba55d".into(),
            warning: "#e0a82e".into(),

            text: "#1a1a1f".into(),
            text_muted: "#6b7280".into(),
            text_inverse: "#ffffff".into(),

            border: "#e4e6ef".into(),
            border_hover: "#b9bdcc".into(),
            border_strong: "#9097a8".into(),

            focus_ring: "#5b6cff".into(),
            overlay: "rgba(15, 17, 21, 0.45)".into(),
        },
        spacing: DEFAULT_SPACING.clone(),
        radius: DEFAULT_RADIUS.clone(),
        typography: DEFAULT_TYPOGRAPHY.clone(),
    }
}

pub fn dark_theme() -> IdeaThemeDefaults {
    IdeaThemeDefaults {
        colors: Colors {
            background: "#0f1115".into(),
            surface: "#1a1d24".into(),
            surface_alt: "#262a35".into(),

            primary: "#8b9aff".into(),
            primary_hover: "#9eabff".into(),
            primary_pressed: "#7383f5".into(),
            primary_text: "#0f1115".into(),

            danger: "#ff6369".into(),
            danger_hover: "#ff7a80".into(),
            danger_pressed: "#e5484d".into(),
            danger_text: "#ffffff".into(),

            success: "#4cc77c".into(),
            warning: "#f0b942".into(),

            text: "#e8eaf0".into(),
            text_muted: "#9099a8".into(),
            text_inverse: "#0f1115".into(),

            border: "#2a2e3a".into(),
            border_hover: "#3d4252".into(),
            border_strong: "#525868".into(),

            focus_ring: "#8b9aff".into(),
            overlay: "rgba(0, 0, 0, 0.55)".into(),
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
