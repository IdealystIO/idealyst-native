//! Platform-agnostic input event vocabulary.
//!
//! Native shells (winit on desktop, browser DOM on web, UIKit on
//! iOS, View.OnTouchListener on Android) translate their native
//! event stream into these types and feed them to a render
//! backend through the [`crate::EventSink`] trait. Nothing in
//! this module depends on any platform — the same types flow
//! through every entry point.
//!
//! # Conventions
//!
//! - **Coordinates** are in logical CSS pixels (the same space
//!   the framework's layout engine computes in). Native shells
//!   are responsible for physical→logical conversion (divide by
//!   `scale_factor`).
//! - **Pointer IDs** are platform-stable for the duration of a
//!   pointer interaction (down→move…→up). Mouse uses a constant
//!   id; touch uses the OS-reported finger id. Multi-touch isn't
//!   wired yet but the field exists so we don't have to reshape
//!   the API.
//! - **Key text** is the IME-resolved character(s) for character
//!   keys. Named keys (Backspace, Escape, …) carry their
//!   semantic `Key` variant and typically have `text: None`.

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

/// Scroll input — a mouse wheel tick, a trackpad two-finger pan,
/// a kinetic-scroll OS event. `delta` is in logical CSS pixels
/// per axis; the shell is responsible for any platform-specific
/// unit conversion (winit's `LineDelta` lines are converted by
/// the winit shim, browser `WheelEvent.deltaY` likewise).
/// `position` is where the pointer is at the moment of the event
/// — the render side uses it to decide which scroll container
/// under the cursor receives the event.
#[derive(Clone, Copy, Debug)]
pub struct ScrollEvent {
    pub position: (f32, f32),
    pub delta: (f32, f32),
}

/// Keyboard input. `text` is filled when the press produced
/// printable text (after IME / dead-key processing). Named keys
/// carry their semantic identity in `key`.
///
/// Shells deliver BOTH presses and releases: the render side routes
/// releases to the app-level keyboard sink (held-key game controls
/// need them) and only presses to a focused text input.
#[derive(Clone, Debug)]
pub struct KeyEvent {
    pub key: Key,
    pub text: Option<String>,
    pub modifiers: KeyModifiers,
    /// `true` for a key-down, `false` for a key-up. A shell whose
    /// platform reports only presses must send a release right after
    /// each press (a tap) so the app never sees a key stuck down.
    pub pressed: bool,
    /// The PHYSICAL key in Web `KeyboardEvent.code` vocabulary
    /// (`"KeyW"`, `"Digit1"`, `"ArrowUp"`, `"Space"`, `"ShiftLeft"`),
    /// independent of layout and modifiers. Empty when the shell
    /// can't identify the key.
    pub code: String,
    /// `true` when the platform flags this press as an auto-repeat
    /// of a held key. Always `false` on a release. (The framework's
    /// key dispatcher also normalizes repeats, so a shell without a
    /// repeat flag can pass `false`.)
    pub repeat: bool,
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
/// keys get a discrete variant so the render side can switch on
/// intent instead of parsing key text.
///
/// Add variants here as more shells need them — the render side
/// should match exhaustively so a missing case fails loudly.
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
    PageUp,
    PageDown,
    Insert,
    Shift,
    Control,
    Alt,
    /// Command on macOS / Win key on Windows / Super on X11.
    Meta,
    CapsLock,
    /// Function key `F1`..=`F24` (the payload is the number).
    F(u8),
    /// Anything we haven't named yet. Shells should map exotic
    /// keys here rather than inventing private variants.
    Unknown,
}
