//! Keyboard event surface shared between text-input primitives.
//!
//! `on_key_down` lets a parent intercept key presses on a `TextInput` or
//! `TextArea` *before* the platform's default handling runs. Typical
//! uses: insert literal Tab as 4 spaces in a code editor, treat Cmd-S
//! as "save" without losing focus, build vim-style keymaps on top of a
//! `TextArea`.
//!
//! ## Cross-platform contract
//!
//! Every backend fires `on_key_down` for every key press the user
//! makes while the input has focus, with the [`KeyEvent`] shape below:
//!
//! - **Web**: maps to a DOM `keydown` listener on the `<textarea>` /
//!   `<input>`. `KeyEvent::key` is the browser's `KeyboardEvent.key`
//!   value — that string vocabulary is the source of truth all
//!   backends conform to.
//! - **iOS**: a paired `UIKeyCommand` + `UITextViewDelegate.shouldChangeTextInRange:`
//!   (or `UITextFieldDelegate.shouldChangeCharactersInRange:`) bridge.
//!   UIKeyCommand handles named keys (Tab, Escape, Arrows, Enter);
//!   the delegate path covers printable input and emits one event per
//!   character. The backend normalises both sides to the same
//!   `KeyEvent::key` string vocabulary as web.
//! - **Android**: maps to `View.OnKeyListener` with the action filter
//!   `KeyEvent.ACTION_DOWN`. Android `KeyEvent` keycodes are
//!   normalised to the web string vocabulary in
//!   `crates/backend/android/.../RustKeyListener.kt`.
//!
//! ## What goes in `key`
//!
//! Match the [Web `KeyboardEvent.key` spec][mdn]. Examples:
//!
//! - Single printable key → the literal character: `"a"`, `"A"`,
//!   `"1"`, `" "` (space).
//! - Named non-printable key → the canonical name: `"Tab"`, `"Enter"`,
//!   `"Escape"`, `"Backspace"`, `"Delete"`, `"ArrowUp"`, `"ArrowDown"`,
//!   `"ArrowLeft"`, `"ArrowRight"`, `"Home"`, `"End"`, `"PageUp"`,
//!   `"PageDown"`.
//! - Modifier-only press → `"Shift"`, `"Control"`, `"Alt"`, `"Meta"`.
//!
//! Authors who need byte-for-byte spec compliance should consult MDN;
//! the backends cover the keys above and pass others through as
//! best-effort.
//!
//! [mdn]: https://developer.mozilla.org/en-US/docs/Web/API/UI_Events/Keyboard_event_key_values

/// One keyboard event delivered to a text-input primitive's
/// `on_key_down` handler.
///
/// Selection offsets are in **UTF-16 code units**, matching what each
/// platform's native API natively reports (`textarea.selectionStart`
/// on web, `UITextRange` on iOS, `EditText.getSelectionStart` on
/// Android). For ASCII text the code-unit count equals the byte count;
/// for non-BMP characters (emoji) one code point may occupy two code
/// units. Documented here so handlers that index into UTF-8 Rust
/// strings know to convert.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "remote-serde", derive(serde::Serialize, serde::Deserialize))]
pub struct KeyEvent {
    /// Spec-compliant key name. See module docs for the vocabulary.
    pub key: String,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
    /// Cursor anchor (UTF-16 code units). Equals `selection_end` when
    /// nothing is selected.
    pub selection_start: usize,
    /// Cursor end (UTF-16 code units). When the user has a range
    /// selected, `selection_end > selection_start`.
    pub selection_end: usize,
}

/// What the backend should do after the handler returns.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "remote-serde", derive(serde::Serialize, serde::Deserialize))]
pub enum KeyOutcome {
    /// Let the platform's default behaviour run (typing the character,
    /// moving focus on Tab, submitting on Enter, …).
    Default,
    /// Suppress the default. Use this after mutating the input
    /// imperatively via the primitive's handle — e.g. calling
    /// `TextAreaHandle::insert_text("    ")` for a Tab override.
    PreventDefault,
}

/// Shared handler type carried into the backend `create_text_*`
/// methods. Aliased so the Backend trait signature stays readable.
pub type KeyDownHandler = std::rc::Rc<dyn Fn(&KeyEvent) -> KeyOutcome>;

// ---------------------------------------------------------------------------
// App-level keyboard input (key down AND key up, regardless of focus).
// ---------------------------------------------------------------------------

/// Whether an [`AppKeyEvent`] is a press or a release.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "remote-serde", derive(serde::Serialize, serde::Deserialize))]
pub enum KeyPhase {
    /// The key went down (or auto-repeated while held — see
    /// [`AppKeyEvent::repeat`]).
    Down,
    /// The key came back up. Also synthesized for every held key when the
    /// app loses keyboard focus, so a game never sees a key stuck down.
    Up,
}

/// One app-level keyboard event — what [`crate::key_input`] listeners
/// receive. Unlike [`KeyEvent`] (a focused text input's key-down), this
/// fires for presses AND releases anywhere in the app.
///
/// ## `key` vs `code`
///
/// - [`key`](Self::key) is the *meaning* of the key under the user's layout
///   and modifiers — Web `KeyboardEvent.key` (`"a"`, `"A"`, `"ArrowUp"`,
///   `" "`). Use it for shortcuts and text-ish handling.
/// - [`code`](Self::code) is the *physical* key, independent of layout and
///   modifiers — Web `KeyboardEvent.code` (`"KeyW"`, `"Digit1"`,
///   `"ArrowUp"`, `"Space"`, `"ShiftLeft"`). Use it for game controls:
///   WASD stays where it is on AZERTY, and a key pressed with Shift and
///   released without it still has the same `code` (its `key` changed from
///   `"W"` to `"w"`). Empty when the platform can't identify the key.
///
/// [mdn-code]: https://developer.mozilla.org/en-US/docs/Web/API/UI_Events/Keyboard_event_code_values
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "remote-serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AppKeyEvent {
    /// Press or release.
    pub phase: KeyPhase,
    /// Web `KeyboardEvent.key` — see the type docs.
    pub key: String,
    /// Web `KeyboardEvent.code` — the physical key. See the type docs.
    pub code: String,
    /// `true` for an auto-repeat key-down while the key is held. Always
    /// `false` on [`KeyPhase::Up`]. Normalized by the dispatcher: a down
    /// for a key that is already down is a repeat on every backend, even
    /// where the platform doesn't flag repeats itself.
    pub repeat: bool,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

impl AppKeyEvent {
    /// A key-down with no modifiers. Convenience for backends and tests.
    pub fn down(key: impl Into<String>, code: impl Into<String>) -> Self {
        AppKeyEvent {
            phase: KeyPhase::Down,
            key: key.into(),
            code: code.into(),
            repeat: false,
            shift: false,
            ctrl: false,
            alt: false,
            meta: false,
        }
    }

    /// A key-up with no modifiers. Convenience for backends and tests.
    pub fn up(key: impl Into<String>, code: impl Into<String>) -> Self {
        AppKeyEvent { phase: KeyPhase::Up, ..AppKeyEvent::down(key, code) }
    }

    /// `true` for a fresh press (down, not an auto-repeat).
    pub fn is_press(&self) -> bool {
        self.phase == KeyPhase::Down && !self.repeat
    }

    /// The text-input-shaped [`KeyEvent`] view of this event (no selection).
    /// Bridges the single-slot `set_app_key_handler` API, which predates
    /// key-up delivery.
    pub fn to_key_event(&self) -> KeyEvent {
        KeyEvent {
            key: self.key.clone(),
            shift: self.shift,
            ctrl: self.ctrl,
            alt: self.alt,
            meta: self.meta,
            selection_start: 0,
            selection_end: 0,
        }
    }
}

/// What a backend's app-level key source delivers into. Installed by
/// `AppEnvOps::set_keyboard_sink` while at least one app-level key listener
/// is live; removed (`None`) when the last one goes.
///
/// A backend calls [`key`](Self::key) for every key down/up it observes
/// app-wide and honors the returned [`KeyOutcome`] where the platform lets
/// it (`PreventDefault` → swallow the native event: no beep on macOS, no
/// page scroll on web for arrow/space). It calls
/// [`focus_lost`](Self::focus_lost) when the app/window stops receiving
/// keys (window blur, app deactivation): the platform will NOT deliver the
/// key-ups for keys still held at that moment, so the dispatcher
/// synthesizes them.
#[derive(Clone)]
pub struct KeyboardSink {
    key: std::rc::Rc<dyn Fn(&AppKeyEvent) -> KeyOutcome>,
    focus_lost: std::rc::Rc<dyn Fn()>,
}

impl KeyboardSink {
    /// Build a sink from its two entry points. The framework builds the
    /// real one ([`crate::key_input`]); backends only call it. Public so
    /// backend tests can build a recording sink.
    pub fn new(
        key: impl Fn(&AppKeyEvent) -> KeyOutcome + 'static,
        focus_lost: impl Fn() + 'static,
    ) -> Self {
        KeyboardSink { key: std::rc::Rc::new(key), focus_lost: std::rc::Rc::new(focus_lost) }
    }

    /// Deliver one key event.
    pub fn key(&self, event: &AppKeyEvent) -> KeyOutcome {
        (self.key)(event)
    }

    /// The app stopped receiving keys; release everything held.
    pub fn focus_lost(&self) {
        (self.focus_lost)()
    }
}

impl std::fmt::Debug for KeyboardSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("KeyboardSink")
    }
}
