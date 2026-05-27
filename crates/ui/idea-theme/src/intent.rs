//! Intent — the *semantic action vocabulary* shared across themed
//! components.
//!
//! An intent answers "what does this action mean?":
//!
//! - [`Primary`]   — the main action of a context.
//! - [`Secondary`] — a supporting action.
//! - [`Neutral`]   — no semantic weight; the default for non-call-to-action buttons.
//! - [`Success`]   — confirms a positive outcome.
//! - [`Danger`]    — destructive / irreversible.
//! - [`Warning`]   — caution required.
//! - [`Info`]      — informational.
//!
//! Intent is orthogonal to **visual treatment**: how an intent looks
//! is controlled by per-component `kind` props (Solid / Soft /
//! Outlined / Ghost on Button, Solid / Soft / Outlined on Badge,
//! etc.). The intent picks the *palette* the kind reaches into;
//! "Danger Outlined" is a red bordered button, "Danger Solid" is a
//! red filled button — same intent, different visual.
//!
//! # Adding a custom intent
//!
//! Implement [`Intent`] on a marker type. Hand it an `IntentColors`
//! computed however you like — typically from a theme extension on
//! your app's concrete theme.
//!
//! ```ignore
//! use idea_ui::{Intent, IntentColors, IdeaTheme};
//!
//! #[derive(Copy, Clone)]
//! pub struct Hype;
//!
//! impl Intent for Hype {
//!     fn colors<'a>(&self, theme: &'a dyn IdeaTheme) -> &'a IntentColors {
//!         // Either pull from a custom extension on the concrete
//!         // theme, or hold a static IntentColors block somewhere
//!         // and return a reference to it.
//!         theme_as_my_theme(theme).hype_intent()
//!     }
//!     fn cache_key(&self) -> u64 { 0x4859504522 /* 'HYPE"' */ }
//! }
//! ```

use std::rc::Rc;

use crate::theme::{IdeaTheme, IntentColors};

/// An intent — a semantic action vocabulary entry. Resolves to an
/// [`IntentColors`] block against the active theme.
///
/// The trait is intentionally narrow: it's a function from the active
/// theme to a colors block (plus a cache key for resolution memoization).
pub trait Intent: 'static {
    /// Return the colors this intent uses against the active theme.
    /// Built-ins return references to theme.intents().* directly;
    /// custom intents can hold their own static block and return a
    /// reference to it.
    fn colors<'a>(&self, theme: &'a dyn IdeaTheme) -> &'a IntentColors;

    /// A stable identifier used for resolution-cache keying. Two
    /// intents that produce the same palette under the same theme
    /// SHOULD return the same cache key; different intents MUST
    /// return different keys.
    fn cache_key(&self) -> u64;
}

// Let `Rc<dyn Intent>` itself act as an Intent — component code holds
// the Rc in a prop already; forwarding the trait through means the
// apply-style closure can call `.colors()` on it directly without
// un-Rc-ing first.
impl Intent for Rc<dyn Intent> {
    fn colors<'a>(&self, theme: &'a dyn IdeaTheme) -> &'a IntentColors {
        (**self).colors(theme)
    }
    fn cache_key(&self) -> u64 {
        (**self).cache_key()
    }
}

/// Convenience for turning a marker type into a `Rc<dyn Intent>`.
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
// Each is a zero-sized marker that resolves through `theme.intents()`.
// Apps that install a custom `IdeaTheme` get their theme's palette
// flowing into these intents automatically.

const KEY_PRIMARY: u64 = 0x0001;
const KEY_SECONDARY: u64 = 0x0002;
const KEY_NEUTRAL: u64 = 0x0003;
const KEY_SUCCESS: u64 = 0x0004;
const KEY_DANGER: u64 = 0x0005;
const KEY_WARNING: u64 = 0x0006;
const KEY_INFO: u64 = 0x0007;

#[derive(Copy, Clone, Default)]
pub struct Primary;
impl Intent for Primary {
    fn colors<'a>(&self, theme: &'a dyn IdeaTheme) -> &'a IntentColors {
        &theme.intents().primary
    }
    fn cache_key(&self) -> u64 {
        KEY_PRIMARY
    }
}

#[derive(Copy, Clone, Default)]
pub struct Secondary;
impl Intent for Secondary {
    fn colors<'a>(&self, theme: &'a dyn IdeaTheme) -> &'a IntentColors {
        &theme.intents().secondary
    }
    fn cache_key(&self) -> u64 {
        KEY_SECONDARY
    }
}

#[derive(Copy, Clone, Default)]
pub struct Neutral;
impl Intent for Neutral {
    fn colors<'a>(&self, theme: &'a dyn IdeaTheme) -> &'a IntentColors {
        &theme.intents().neutral
    }
    fn cache_key(&self) -> u64 {
        KEY_NEUTRAL
    }
}

#[derive(Copy, Clone, Default)]
pub struct Success;
impl Intent for Success {
    fn colors<'a>(&self, theme: &'a dyn IdeaTheme) -> &'a IntentColors {
        &theme.intents().success
    }
    fn cache_key(&self) -> u64 {
        KEY_SUCCESS
    }
}

#[derive(Copy, Clone, Default)]
pub struct Danger;
impl Intent for Danger {
    fn colors<'a>(&self, theme: &'a dyn IdeaTheme) -> &'a IntentColors {
        &theme.intents().danger
    }
    fn cache_key(&self) -> u64 {
        KEY_DANGER
    }
}

#[derive(Copy, Clone, Default)]
pub struct Warning;
impl Intent for Warning {
    fn colors<'a>(&self, theme: &'a dyn IdeaTheme) -> &'a IntentColors {
        &theme.intents().warning
    }
    fn cache_key(&self) -> u64 {
        KEY_WARNING
    }
}

#[derive(Copy, Clone, Default)]
pub struct Info;
impl Intent for Info {
    fn colors<'a>(&self, theme: &'a dyn IdeaTheme) -> &'a IntentColors {
        &theme.intents().info
    }
    fn cache_key(&self) -> u64 {
        KEY_INFO
    }
}
