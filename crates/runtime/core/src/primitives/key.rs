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
