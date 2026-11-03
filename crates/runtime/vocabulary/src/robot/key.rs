//! The robot's `key` verb: press a key through the platform's real key
//! path ([`IntrospectionOps::native_key`]), or — on a backend that can't
//! synthesize key input — through the framework's own key paths, which
//! are what that platform's key handling calls.

use super::*;
use crate::caps::{KeyChord, KeyPress};
use runtime_shared::primitives::key::{AppKeyEvent, KeyDownHandler, KeyEvent, KeyOutcome, KeyPhase};

/// How [`Robot::press_key`] delivered the press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyMechanism {
    /// Real platform key events, through the toolkit's event path.
    Native,
    /// The framework's own key dispatch: the focused field's `on_key_down`,
    /// the app-level listeners (`on_key`), then the field's text edit,
    /// Tab's focus traversal and Escape's overlay dismissal. Used where the
    /// platform has no key injection (iOS, Linux GTK).
    Dispatch,
}

impl KeyMechanism {
    pub fn as_str(self) -> &'static str {
        match self {
            KeyMechanism::Native => "native",
            KeyMechanism::Dispatch => "dispatch",
        }
    }
}

/// What a text field gives the robot's dispatch fallback: its
/// `on_key_down` (scope-guarded, as the backend holds it) and whether
/// Enter types a newline.
#[derive(Clone)]
pub(crate) struct KeyTarget {
    pub on_key_down: Option<KeyDownHandler>,
    pub multiline: bool,
}

/// Resolves a chord with the backend's shortcut modifier and prepares the
/// backend's injection for it (built under the backend borrow, run after).
pub(crate) type NativeKey = Rc<dyn Fn(&KeyChord) -> (KeyPress, Option<NativeInjection>)>;

pub(crate) fn native_key_closure<H: IntrospectionOps>(backend: &Rc<RefCell<H>>, node: &H::Node) -> NativeKey {
    let b = backend.clone();
    let n = node.clone();
    Rc::new(move |chord: &KeyChord| {
        let backend = b.borrow();
        let press = chord.resolve(backend.shortcut_modifier_is_meta());
        let injection = backend.native_key(&n, &press);
        (press, injection)
    })
}

/// The platform's focus traversal in an element's window
/// ([`IntrospectionOps::move_focus`]): built under the backend borrow, run
/// after it is released.
pub(crate) type MoveFocus = Rc<dyn Fn(bool) -> Option<NativeInjection>>;

pub(crate) fn move_focus_closure<H: IntrospectionOps>(backend: &Rc<RefCell<H>>, node: &H::Node) -> MoveFocus {
    let b = backend.clone();
    let n = node.clone();
    Rc::new(move |forward: bool| b.borrow().move_focus(&n, forward))
}

impl Robot {
    /// Press a key, as a user at a keyboard would: a key down and up
    /// delivered to whatever holds keyboard focus. With `element`, focus it
    /// first (Playwright's `locator.press`) — a text field takes focus; an
    /// element that can't is left as it is and the key goes to the current
    /// focus. Without one, the key goes to the window (`page.keyboard.press`).
    ///
    /// The backend's [`native_key`](IntrospectionOps::native_key) sends it
    /// through the toolkit's real key path, so a field's own editing
    /// (Backspace deletes, Enter in a single-line field fires its
    /// `on_key_down`), Escape-to-dismiss and shortcuts behave as a user's
    /// press does ([`KeyMechanism::Native`]). Where the platform has no key
    /// injection, the press runs the framework's own key paths
    /// ([`KeyMechanism::Dispatch`]): the focused field's `on_key_down`,
    /// the app-level listeners (`on_key`), then — unless either prevented
    /// it — the field's text edit (a typed character appended, Backspace
    /// removing the last one, Enter a newline and Tab a tab character in a
    /// `text_area`: the caret is taken to be at the end), for any other
    /// Tab or Shift+Tab the platform's own focus traversal
    /// ([`IntrospectionOps::move_focus`]), for Escape the topmost overlay's
    /// `on_dismiss` ([`crate::escape_dismiss`]), and the release.
    pub fn press_key(&self, element: Option<&Element>, chord: &KeyChord) -> Result<KeyMechanism, RobotError> {
        const ACTION: &str = "key";
        let target = match element {
            Some(el) => {
                let actions = REGISTRY
                    .with(|r| r.borrow().get(el.id).map(|e| e.actions.clone()))
                    .ok_or(RobotError::ElementGone)?;
                refuse_if_disabled(&actions, ACTION)?;
                if let Some(focus) = actions.focus.clone() {
                    focus();
                    settle();
                }
                Some(el.id)
            }
            None => None,
        };
        // The window to send it to: the element's, or the first root's.
        let window = target.or_else(|| REGISTRY.with(|r| r.borrow().roots().first().copied()));
        let prepare = window.and_then(|id| REGISTRY.with(|r| r.borrow().get(id).and_then(|e| e.actions.native_key.clone())));
        let (press, injection) = match prepare {
            Some(prepare) => prepare(chord),
            None => (chord.resolve(runtime_shared::host::platform().is_apple()), None),
        };
        if let Some(inject) = injection {
            inject().map_err(RobotError::Native)?;
            settle();
            return Ok(KeyMechanism::Native);
        }
        let result = dispatch_key(target, window, &press);
        settle();
        result.map(|()| KeyMechanism::Dispatch)
    }
}

/// [`KeyMechanism::Dispatch`]: the press through the framework's key paths.
/// `window` is the element whose window an unfocused press goes to.
fn dispatch_key(target: Option<ElementId>, window: Option<ElementId>, press: &KeyPress) -> Result<(), RobotError> {
    let actions_of = |id: ElementId| REGISTRY.with(|r| r.borrow().get(id).map(|e| e.actions.clone()));
    // The field the key lands in: the element just focused, when it is a
    // field, else whichever field reports focus.
    let field = target
        .and_then(actions_of)
        .filter(|a| a.keys.is_some())
        .or_else(|| {
            let fields: Vec<ElementActions> = REGISTRY.with(|r| {
                r.borrow().entries().filter(|(_, e)| e.actions.keys.is_some()).map(|(_, e)| e.actions.clone()).collect()
            });
            fields.into_iter().find(|a| a.state.as_ref().is_some_and(|s| s().focused == Some(true)))
        });
    let text_of = |a: &ElementActions| match a.state.as_ref().map(|s| s().value) {
        Some(Some(StateValue::Text(t))) => t,
        _ => String::new(),
    };

    let mut prevented = false;
    // The field's own key handler first, as a key reaches the focused
    // control before it bubbles to the app.
    if let Some(a) = &field {
        if let Some(on_key_down) = a.keys.as_ref().and_then(|k| k.on_key_down.clone()) {
            let caret = text_of(a).encode_utf16().count();
            let event = KeyEvent {
                key: press.key.clone(),
                shift: press.shift,
                ctrl: press.ctrl,
                alt: press.alt,
                meta: press.meta,
                selection_start: caret,
                selection_end: caret,
            };
            prevented |= on_key_down(&event) == KeyOutcome::PreventDefault;
        }
    }
    let down = AppKeyEvent {
        phase: KeyPhase::Down,
        key: press.key.clone(),
        code: press.code.clone(),
        repeat: false,
        shift: press.shift,
        ctrl: press.ctrl,
        alt: press.alt,
        meta: press.meta,
    };
    prevented |= runtime_shared::key_input::deliver(&down) == KeyOutcome::PreventDefault;
    let mut traversal = Ok(());
    if !prevented {
        let edited = match &field {
            Some(a) => edit_field(a, press, text_of(a)),
            None => false,
        };
        // Tab a field didn't take as text moves focus, in the platform's
        // order, within the focused field's window (an overlay may be a
        // window of its own) or else the target's.
        if !edited && press.key == "Tab" && !(press.ctrl || press.alt || press.meta) {
            let move_focus = field
                .as_ref()
                .and_then(|a| a.move_focus.clone())
                .or_else(|| window.and_then(actions_of).and_then(|a| a.move_focus));
            if let Some(inject) = move_focus.and_then(|m| m(!press.shift)) {
                traversal = inject().map_err(RobotError::Native);
            }
        }
        // The platform default every native key path has: an Escape nothing
        // consumed closes the topmost overlay with an `on_dismiss`.
        if press.key == "Escape" {
            crate::escape_dismiss::dismiss_topmost();
        }
    }
    settle();
    runtime_shared::key_input::deliver(&AppKeyEvent { phase: KeyPhase::Up, ..down });
    traversal
}

/// The text edit a key makes in a field with the caret at the end; whether
/// the key was the field's (it took Backspace on empty text too). A
/// multi-line field takes an unmodified Tab as a tab character, as
/// `UITextView` and GTK's `TextView` — the dispatch platforms' text areas
/// — do; a single-line field leaves Tab to focus traversal.
fn edit_field(actions: &ElementActions, press: &KeyPress, text: String) -> bool {
    let (Some(set_text), Some(keys)) = (actions.set_text.clone(), actions.keys.as_ref()) else { return false };
    if actions.state.as_ref().is_some_and(|s| s().is_disabled()) {
        return false;
    }
    let plain = !(press.ctrl || press.meta || press.alt);
    let edited = match press.key.as_str() {
        "Backspace" if !plain => return false,
        "Backspace" => {
            let mut t = text;
            if t.pop().is_none() {
                return true;
            }
            t
        }
        "Enter" if keys.multiline && !(press.ctrl || press.meta) => text + "\n",
        "Tab" if keys.multiline && plain && !press.shift => text + "\t",
        _ => match press.text() {
            Some(c) => {
                let mut t = text;
                t.push(c);
                t
            }
            None => return false,
        },
    };
    set_text(edited);
    true
}

/// Parse the `key` verb's `{key, modifiers?}` into a chord.
pub(crate) fn chord_from_args(args: &runtime_shared::__serde_json::Value) -> Result<KeyChord, String> {
    let key = args["key"].as_str().ok_or("missing 'key' argument (e.g. \"Enter\", \"Meta+K\")")?;
    let modifiers: Vec<&str> = match &args["modifiers"] {
        runtime_shared::__serde_json::Value::Null => Vec::new(),
        runtime_shared::__serde_json::Value::Array(items) => items
            .iter()
            .map(|m| m.as_str().ok_or_else(|| format!("'modifiers' must be strings, got {m}")))
            .collect::<Result<_, _>>()?,
        other => return Err(format!("'modifiers' must be an array of strings, got {other}")),
    };
    KeyChord::from_parts(key, &modifiers)
}
