//! Platform-agnostic input event vocabulary.
//!
//! Native shells (winit on desktop, browser DOM on web, UIKit on
//! iOS, View.OnTouchListener on Android) translate their native
//! event stream into these types and feed them to [`crate::Host`].
//! Nothing in this module depends on any platform — the same types
//! flow through every entry point.
//!
//! # Conventions
//!
//! - **Coordinates** are in logical CSS pixels (the same space Taffy
//!   computes layout in). Native shells are responsible for
//!   physical→logical conversion (divide by `scale_factor`).
//! - **Pointer IDs** are platform-stable for the duration of a
//!   pointer interaction (down→move…→up). Mouse uses a constant id;
//!   touch uses the OS-reported finger id. Multi-touch isn't wired
//!   yet but the field exists so we don't have to reshape the API.
//! - **Key text** is the IME-resolved character(s) for character
//!   keys. Named keys (Backspace, Escape, …) carry `Key::Named` and
//!   typically have `text: None`.

/// Identifies a pointer interaction. Use [`PointerId::MOUSE`] for
/// the primary mouse pointer; touch shells should pass the OS's
/// finger id (Apple's `UITouch` identifier, browser's `pointerId`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PointerId(pub u64);

impl PointerId {
    pub const MOUSE: PointerId = PointerId(0);
}

/// Which physical / logical button the pointer reports. Touch
/// always uses `Primary`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerButton {
    Primary,
    Secondary,
    Middle,
    Other(u16),
}

#[derive(Clone, Copy, Debug)]
pub struct PointerEvent {
    pub id: PointerId,
    pub button: PointerButton,
    pub position: (f32, f32),
}

/// Keyboard input. `text` is filled when the press produced
/// printable text (after IME / dead-key processing). Named keys
/// carry their semantic identity in `key`.
#[derive(Clone, Debug)]
pub struct KeyEvent {
    pub key: Key,
    pub text: Option<String>,
    pub modifiers: KeyModifiers,
    /// `true` for a key-down, `false` for a key-up. Shells that
    /// only emit one of the two (e.g. UIKit's `pressesBegan`) can
    /// always set `true`.
    pub pressed: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct KeyModifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// Command on macOS / Win key on Windows / Meta on X11.
    pub meta: bool,
}

/// Normalized key identity. Character-producing keys arrive as
/// `Character` (the actual text is in [`KeyEvent::text`]); named
/// keys get a discrete variant so the host can switch on intent
/// instead of parsing key text.
///
/// Add variants here as more shells need them — the host should
/// match exhaustively so a missing case fails loudly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Character,
    Backspace,
    Delete,
    Enter,
    Escape,
    Tab,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    Home,
    End,
    /// Anything we haven't named yet. Shells should map exotic
    /// keys here rather than inventing private variants.
    Unknown,
}
