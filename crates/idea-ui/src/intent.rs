//! Intent — the *global vocabulary* of semantic colorings every
//! themed idea-ui component shares.
//!
//! An intent answers "what does this component mean?" — `Primary`,
//! `Success`, `Danger`, `Warning`, `Neutral`, or any app-defined
//! intent like `Hype` or `Beta`. Every component that has a themed
//! slot (Pressable, Badge, future Tag/Alert/Chip) consults the same
//! `Intent` trait, so an intent defined once works everywhere.
//!
//! # Defining a new intent
//!
//! Implement [`Intent`] on a marker type. The intent is responsible
//! for resolving a [`Palette`] against the active theme — so its
//! colors can either be hard-coded or pulled from a custom theme
//! extension the app installed.
//!
//! ```ignore
//! use idea_ui::{Intent, IntentPalette, IdeaTheme};
//!
//! #[derive(Copy, Clone)]
//! pub struct Hype;
//!
//! impl Intent for Hype {
//!     fn palette(&self, _theme: &dyn IdeaTheme) -> IntentPalette {
//!         IntentPalette {
//!             background:      "#ff00aa".into(),
//!             background_hover:"#ff44bb".into(),
//!             background_pressed:"#cc0088".into(),
//!             foreground:      "#ffffff".into(),
//!             border:          None,
//!         }
//!     }
//!
//!     fn cache_key(&self) -> u64 { 0xHYPE }
//! }
//!
//! ui! { Pressable(intent = Hype.boxed(), label = "Buy") }
//! ```

use std::rc::Rc;

use framework_core::{Color, Tokenized};

use crate::theme::IdeaTheme;

/// The resolved colors an [`Intent`] produces against a theme. The
/// trait that consumes this is responsible for picking which fields
/// matter — Pressable uses bg + hover + pressed + fg; Badge uses
/// bg + fg; future components may use the same struct's other slots.
///
/// Each color is a `Tokenized<Color>` — built-in intents pull theme
/// tokens by reference, so swapping themes updates every component
/// styled by an intent without re-minting any classes. Custom intents
/// can use `Tokenized::Literal(Color(...))` for hard-coded values.
#[derive(Clone)]
pub struct IntentPalette {
    pub background: Tokenized<Color>,
    pub background_hover: Tokenized<Color>,
    pub background_pressed: Tokenized<Color>,
    pub foreground: Tokenized<Color>,
    /// Optional border color. `None` means "no border for this
    /// intent" — useful for ghost / outlined intents that want a
    /// border without a fill.
    pub border: Option<Tokenized<Color>>,
}

impl IntentPalette {
    /// Helper for intents that don't differentiate hover / pressed
    /// from the base background. Components are free to ignore the
    /// hover/pressed slots if they don't use them.
    pub fn flat(background: Tokenized<Color>, foreground: Tokenized<Color>) -> Self {
        Self {
            background: background.clone(),
            background_hover: background.clone(),
            background_pressed: background,
            foreground,
            border: None,
        }
    }
}

/// An intent — a semantic coloring shared across every themed
/// component. Implement this on a marker type to add a new intent;
/// the same type then works in `Pressable`, `Badge`, and any future
/// intent-consuming component.
///
/// The trait is intentionally narrow: it's a function from the
/// active theme to a [`IntentPalette`]. Built-in intents pull from
/// `theme.colors()`; custom intents can hard-code values or reach
/// for extensions on the concrete theme type via `Any` downcast.
pub trait Intent: 'static {
    /// Resolve this intent's colors against the active theme.
    /// Called once per style resolution.
    fn palette(&self, theme: &dyn IdeaTheme) -> IntentPalette;

    /// A stable identifier used for resolution-cache keying. Two
    /// intents that produce the same palette under the same theme
    /// SHOULD return the same cache key; different intents MUST
    /// return different keys.
    ///
    /// Built-ins use small constants; user intents typically use a
    /// hash of their type name or a hand-picked constant.
    fn cache_key(&self) -> u64;
}

// Let `Rc<dyn Intent>` itself act as an Intent — component code that
// holds an intent in a prop already has the `Rc`, and forwarding the
// trait through means the apply-style closure can call `.palette()`
// on it directly without un-Rc-ing first.
impl Intent for Rc<dyn Intent> {
    fn palette(&self, theme: &dyn IdeaTheme) -> IntentPalette {
        (**self).palette(theme)
    }
    fn cache_key(&self) -> u64 {
        (**self).cache_key()
    }
}

/// Convenience for turning a marker type into a `Rc<dyn Intent>` —
/// the shape component props store internally.
pub trait IntoRcIntent {
    fn into_rc(self) -> Rc<dyn Intent>;
}

impl<I: Intent + 'static> IntoRcIntent for I {
    fn into_rc(self) -> Rc<dyn Intent> {
        Rc::new(self)
    }
}

// =============================================================================
// Built-in intents
// =============================================================================
//
// Each is a zero-sized marker that pulls its colors from the default
// theme tokens. Apps that install a custom `IdeaTheme` get their
// theme's primary/danger/etc. flowing into these intents
// automatically, because the palette is computed against the active
// theme at resolution time.

const KEY_PRIMARY:   u64 = 0x0001;
const KEY_SECONDARY: u64 = 0x0002;
const KEY_NEUTRAL:   u64 = 0x0003;
const KEY_GHOST:     u64 = 0x0004;
const KEY_SUCCESS:   u64 = 0x0005;
const KEY_WARNING:   u64 = 0x0006;
const KEY_DANGER:    u64 = 0x0007;

#[derive(Copy, Clone, Default)]
pub struct Primary;
impl Intent for Primary {
    fn palette(&self, theme: &dyn IdeaTheme) -> IntentPalette {
        let c = theme.colors();
        IntentPalette {
            background:         c.primary.clone(),
            background_hover:   c.primary_hover.clone(),
            background_pressed: c.primary_pressed.clone(),
            foreground:         c.primary_text.clone(),
            border:             None,
        }
    }
    fn cache_key(&self) -> u64 { KEY_PRIMARY }
}

/// A muted secondary action — uses surface_alt as background.
#[derive(Copy, Clone, Default)]
pub struct Secondary;
impl Intent for Secondary {
    fn palette(&self, theme: &dyn IdeaTheme) -> IntentPalette {
        let c = theme.colors();
        IntentPalette {
            background:         c.surface_alt.clone(),
            background_hover:   c.surface_alt.clone(),
            background_pressed: c.border.clone(),
            foreground:         c.text.clone(),
            border:             None,
        }
    }
    fn cache_key(&self) -> u64 { KEY_SECONDARY }
}

/// Outlined / neutral — transparent background with a border. The
/// closest match for the old `Pressable.kind = Secondary` look.
#[derive(Copy, Clone, Default)]
pub struct Neutral;
impl Intent for Neutral {
    fn palette(&self, theme: &dyn IdeaTheme) -> IntentPalette {
        let c = theme.colors();
        IntentPalette {
            background:         Tokenized::Literal(Color("transparent".into())),
            background_hover:   c.surface_alt.clone(),
            background_pressed: c.border.clone(),
            foreground:         c.text.clone(),
            border:             Some(c.border.clone()),
        }
    }
    fn cache_key(&self) -> u64 { KEY_NEUTRAL }
}

/// Borderless, transparent — minimal-chrome action.
#[derive(Copy, Clone, Default)]
pub struct Ghost;
impl Intent for Ghost {
    fn palette(&self, theme: &dyn IdeaTheme) -> IntentPalette {
        let c = theme.colors();
        IntentPalette {
            background:         Tokenized::Literal(Color("transparent".into())),
            background_hover:   c.surface_alt.clone(),
            background_pressed: c.border.clone(),
            foreground:         c.text.clone(),
            border:             None,
        }
    }
    fn cache_key(&self) -> u64 { KEY_GHOST }
}

#[derive(Copy, Clone, Default)]
pub struct Success;
impl Intent for Success {
    fn palette(&self, theme: &dyn IdeaTheme) -> IntentPalette {
        let c = theme.colors();
        IntentPalette {
            background:         c.success.clone(),
            background_hover:   c.success.clone(),
            background_pressed: c.success.clone(),
            foreground:         Tokenized::Literal(Color("#ffffff".into())),
            border:             None,
        }
    }
    fn cache_key(&self) -> u64 { KEY_SUCCESS }
}

#[derive(Copy, Clone, Default)]
pub struct Warning;
impl Intent for Warning {
    fn palette(&self, theme: &dyn IdeaTheme) -> IntentPalette {
        let c = theme.colors();
        IntentPalette {
            background:         c.warning.clone(),
            background_hover:   c.warning.clone(),
            background_pressed: c.warning.clone(),
            foreground:         Tokenized::Literal(Color("#1a1a1f".into())),
            border:             None,
        }
    }
    fn cache_key(&self) -> u64 { KEY_WARNING }
}

#[derive(Copy, Clone, Default)]
pub struct Danger;
impl Intent for Danger {
    fn palette(&self, theme: &dyn IdeaTheme) -> IntentPalette {
        let c = theme.colors();
        IntentPalette {
            background:         c.danger.clone(),
            background_hover:   c.danger_hover.clone(),
            background_pressed: c.danger_pressed.clone(),
            foreground:         c.danger_text.clone(),
            border:             None,
        }
    }
    fn cache_key(&self) -> u64 { KEY_DANGER }
}

// =============================================================================
// Applying a palette to a StyleApplication
// =============================================================================

/// Merge an [`IntentPalette`] into a [`framework_core::StyleApplication`]'s
/// overrides. The palette's background → `override_background`,
/// foreground → `override_color`, and (if `palette.border` is set)
/// a 1px border on all four sides with the palette's border color.
///
/// State-aware intent shades (hover, pressed) live on `IntentPalette`
/// but aren't applied today — the framework's `StyleApplication`
/// override mechanism is state-agnostic. A future `state_overrides`
/// field would unlock per-state intent shades; for now intents apply
/// a single static coloring and the stylesheet's `state hovered/pressed`
/// blocks (if any) still drive interaction feedback.
pub fn apply_palette(
    mut style: framework_core::StyleApplication,
    palette: &IntentPalette,
) -> framework_core::StyleApplication {
    style = style
        .override_background(palette.background.clone())
        .override_color(palette.foreground.clone());

    if let Some(border) = &palette.border {
        // No `override_border_color` builder yet — poke `overrides`
        // directly. (Framework follow-up: add per-side and
        // shorthand builders for border color + width.)
        let r = &mut style.overrides;
        r.border_top_color = Some(border.clone());
        r.border_right_color = Some(border.clone());
        r.border_bottom_color = Some(border.clone());
        r.border_left_color = Some(border.clone());
        r.border_top_width = Some(Tokenized::Literal(1.0));
        r.border_right_width = Some(Tokenized::Literal(1.0));
        r.border_bottom_width = Some(Tokenized::Literal(1.0));
        r.border_left_width = Some(Tokenized::Literal(1.0));
    }

    style
}
