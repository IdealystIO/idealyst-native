//! The robot's real key input on web (`IntrospectionOps::native_key`).
//!
//! The press is dispatched as DOM `KeyboardEvent`s on the focused element
//! (`document.activeElement`, else `<body>`), bubbling and cancelable, in
//! the order a browser sends them: `keydown`, `keypress` (keys that type
//! something, and Enter), `keyup`. Every listener the app has sees them as
//! it sees a user's: a field's `on_key_down`, the app-level key source on
//! `document`, a portal's Escape listener on `window`.
//!
//! Script-dispatched events are untrusted, and the browser performs no
//! default action for them — a synthetic `keydown` types nothing and moves
//! no focus. So when no listener prevented the key, this module performs
//! the defaults a user's press gets, through the same DOM paths the
//! browser would use:
//!
//! - in a focused `<input>` / `<textarea>`: a typed character replaces the
//!   selection, Backspace / Delete remove the selection or one character,
//!   Enter types a newline in a `<textarea>` (a single-line field does
//!   nothing more — Enter-to-submit is the field's `on_key_down`), the
//!   arrows / Home / End move the caret, Ctrl/⌘+A selects all. Every edit
//!   writes the value, places the caret, and fires the `input` event the
//!   text primitives listen to, so `on_change` runs as for typing.
//! - Tab / Shift+Tab move focus to the next / previous tabbable element in
//!   document order (with `.focus()`, so a portal's focus trap — a
//!   `focusin` listener — still pulls focus back inside it).
//! - Enter on a focused `<button>` or link clicks it at `keydown`; Space
//!   clicks a `<button>`, checkbox or radio at `keyup`.
//!
//! The edit computation is pure ([`field_default`]) and unit-tested.

use runtime_vocabulary::caps::{KeyPress, NativeInjection};
use web_glue::dom::{Element, HtmlInputElement, HtmlTextAreaElement, KeyboardEvent, KeyboardEventInit};
use web_glue::JsCast;

web_glue::import! {
    /// Move focus from the active element to the next (`back` = 0) or
    /// previous (`back` = 1) tabbable element in document order — enabled,
    /// rendered, not `tabindex="-1"` — wrapping at the ends. 1 when focus
    /// moved.
    fn js_tab_move(back: u32) -> u32 =
        "(back) => { \
           const sel = 'input:not([type=hidden]), textarea, select, button, a[href], [tabindex], [contenteditable=\"\"], [contenteditable=true]'; \
           const all = [...document.querySelectorAll(sel)].filter((e) => \
             !e.disabled && e.tabIndex >= 0 && e.getAttribute('tabindex') !== '-1' && \
             (typeof e.checkVisibility !== 'function' || e.checkVisibility({ visibilityProperty: true }))); \
           if (!all.length) return 0; \
           const i = all.indexOf(document.activeElement); \
           const n = all.length; \
           const next = i < 0 ? (back ? n - 1 : 0) : (i + (back ? n - 1 : 1)) % n; \
           all[next].focus(); \
           return 1; }";
}

/// See [`IntrospectionOps::native_key`](runtime_vocabulary::caps::IntrospectionOps::native_key).
pub(crate) fn native_key(press: &KeyPress) -> NativeInjection {
    let press = press.clone();
    Box::new(move || press_key(&press))
}

/// Whether the browser runs on an Apple OS, where shortcuts use ⌘ —
/// what a robot `ControlOrMeta` resolves to. `navigator.platform`
/// (`"MacIntel"`, `"iPhone"`, `"iPad"`), sampled once as the Ctrl-click
/// classification in `primitives/touch.rs` samples it.
pub(crate) fn shortcut_modifier_is_meta() -> bool {
    thread_local! {
        static APPLE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
    }
    APPLE.with(|c| {
        if let Some(v) = c.get() {
            return v;
        }
        let platform = web_glue::dom::window()
            .map(|w| w.navigator().platform().unwrap_or_default())
            .unwrap_or_default();
        let v = ["Mac", "iPhone", "iPad", "iPod"].iter().any(|p| platform.starts_with(p));
        c.set(Some(v));
        v
    })
}

fn press_key(press: &KeyPress) -> Result<(), String> {
    let doc = web_glue::dom::window()
        .and_then(|w| w.document())
        .ok_or("no document")?;
    let target: Element = match doc.active_element() {
        Some(e) => e,
        None => doc.body().ok_or("no focused element and no <body>")?.unchecked_into(),
    };
    let field = Field::of(&target);
    let typed = press.text();
    let mut go = fire(&target, "keydown", press)?;
    if go && (typed.is_some() || press.key == "Enter") {
        go = fire(&target, "keypress", press)?;
    }
    let mut space_click = false;
    if go {
        match (&field, press.key.as_str()) {
            (Some(field), _) => field.apply(press),
            (None, "Tab") if !(press.ctrl || press.meta || press.alt) => {
                crate::pending_focus::moving_focus(|| {
                    unsafe { js_tab_move(press.shift as u32) };
                });
            }
            (None, "Enter") if clicks_on_enter(&target) => clickable(&target).click(),
            (None, " ") => space_click = clicks_on_space(&target),
            _ => {}
        }
        // Tab out of a field moves focus too.
        if field.is_some() && press.key == "Tab" && !(press.ctrl || press.meta || press.alt) {
            crate::pending_focus::moving_focus(|| {
                unsafe { js_tab_move(press.shift as u32) };
            });
        }
    }
    // The release goes where the press went, even if focus moved since.
    let released = fire(&target, "keyup", press)?;
    if space_click && released {
        clickable(&target).click();
    }
    Ok(())
}

/// Dispatch one keyboard event; `true` when no listener prevented it.
fn fire(target: &Element, ty: &str, press: &KeyPress) -> Result<bool, String> {
    let init = KeyboardEventInit::new();
    init.set_bubbles(true);
    init.set_cancelable(true);
    init.set_composed(true);
    init.set_key(&press.key);
    init.set_code(&press.code);
    init.set_shift_key(press.shift);
    init.set_ctrl_key(press.ctrl);
    init.set_alt_key(press.alt);
    init.set_meta_key(press.meta);
    let ev = KeyboardEvent::new_with_keyboard_event_init_dict(ty, &init).map_err(|e| format!("{ty}: {e:?}"))?;
    target.dispatch_event(&ev).map_err(|e| format!("{ty}: {e:?}"))
}

fn tag(e: &Element) -> String {
    e.tag_name().to_ascii_uppercase()
}

fn input_type(e: &Element) -> Option<String> {
    e.clone().dyn_into::<HtmlInputElement>().ok().map(|i| i.type_().to_ascii_lowercase())
}

fn clicks_on_enter(e: &Element) -> bool {
    match tag(e).as_str() {
        "BUTTON" => true,
        "A" => e.has_attribute("href"),
        _ => false,
    }
}

fn clicks_on_space(e: &Element) -> bool {
    tag(e) == "BUTTON" || matches!(input_type(e).as_deref(), Some("checkbox" | "radio"))
}

fn clickable(e: &Element) -> web_glue::dom::HtmlElement {
    e.clone().unchecked_into()
}

/// A focused text control and the edits it takes.
enum Field {
    Input(HtmlInputElement),
    Area(HtmlTextAreaElement),
}

impl Field {
    fn of(e: &Element) -> Option<Field> {
        if let Ok(area) = e.clone().dyn_into::<HtmlTextAreaElement>() {
            return Some(Field::Area(area));
        }
        let input = e.clone().dyn_into::<HtmlInputElement>().ok()?;
        // Text-entry types only: a checkbox or range takes no typing.
        let ty = input.type_().to_ascii_lowercase();
        matches!(ty.as_str(), "" | "text" | "password" | "email" | "search" | "tel" | "url" | "number")
            .then_some(Field::Input(input))
    }

    fn apply(&self, press: &KeyPress) {
        let (value, start, end, multiline, locked) = match self {
            Field::Input(i) => (
                i.value(),
                i.selection_start().ok().flatten(),
                i.selection_end().ok().flatten(),
                false,
                i.disabled() || i.read_only(),
            ),
            Field::Area(a) => (
                a.value(),
                a.selection_start().ok().flatten(),
                a.selection_end().ok().flatten(),
                true,
                a.disabled() || a.read_only(),
            ),
        };
        // A field with no selection API (`type=number`, `email` in some
        // browsers) reports none: the caret is at the end.
        let len = value.encode_utf16().count() as u32;
        let (start, end) = (start.unwrap_or(len), end.unwrap_or(len));
        let Some(action) = field_default(&value, start, end, press, multiline) else { return };
        match action {
            FieldAction::Select { start, end } => self.select(start, end),
            FieldAction::Edit { .. } if locked => {}
            FieldAction::Edit { value, caret } => {
                match self {
                    Field::Input(i) => i.set_value(&value),
                    Field::Area(a) => a.set_value(&value),
                }
                self.select(caret, caret);
                let init = web_glue::dom::EventInit::new();
                init.set_bubbles(true);
                if let Ok(ev) = web_glue::dom::Event::new_with_event_init_dict("input", &init) {
                    let target: &web_glue::dom::EventTarget = match self {
                        Field::Input(i) => i.as_ref(),
                        Field::Area(a) => a.as_ref(),
                    };
                    let _ = target.dispatch_event(&ev);
                }
            }
        }
    }

    fn select(&self, start: u32, end: u32) {
        let _ = match self {
            Field::Input(i) => i.set_selection_range(start, end),
            Field::Area(a) => a.set_selection_range(start, end),
        };
    }
}

/// What a key does to a text field by default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FieldAction {
    /// New text, caret (UTF-16 offset) after the edit.
    Edit { value: String, caret: u32 },
    /// Selection change only (UTF-16 offsets).
    Select { start: u32, end: u32 },
}

/// The default action of `press` in a text field holding `value` with the
/// selection `start..end` (UTF-16 code units, as the DOM reports them).
/// `None` = the key does nothing to the field. Word-wise moves and deletes
/// (Alt/Ctrl+arrow, Alt+Backspace) and Shift-extended selections are not
/// modelled.
pub(crate) fn field_default(value: &str, start: u32, end: u32, press: &KeyPress, multiline: bool) -> Option<FieldAction> {
    let units: Vec<u16> = value.encode_utf16().collect();
    let len = units.len() as u32;
    let (start, end) = (start.min(len), end.min(len));
    let (start, end) = (start.min(end), start.max(end));
    let shortcut = press.ctrl || press.meta;
    let replace = |with: &str| {
        let mut out: Vec<u16> = units[..start as usize].to_vec();
        out.extend(with.encode_utf16());
        let caret = out.len() as u32;
        out.extend_from_slice(&units[end as usize..]);
        FieldAction::Edit { value: String::from_utf16_lossy(&out), caret }
    };
    let remove = |from: u32, to: u32| {
        let mut out: Vec<u16> = units[..from as usize].to_vec();
        out.extend_from_slice(&units[to as usize..]);
        FieldAction::Edit { value: String::from_utf16_lossy(&out), caret: from }
    };
    // One character back / forward from `at`, stepping over a surrogate pair.
    let prev = |at: u32| {
        if at >= 2 && (0xDC00..0xE000).contains(&units[at as usize - 1]) && (0xD800..0xDC00).contains(&units[at as usize - 2]) {
            at - 2
        } else {
            at.saturating_sub(1)
        }
    };
    let next = |at: u32| {
        if at + 2 <= len && (0xD800..0xDC00).contains(&units[at as usize]) {
            at + 2
        } else {
            (at + 1).min(len)
        }
    };
    let line_start = |at: u32| units[..at as usize].iter().rposition(|&u| u == u16::from(b'\n')).map_or(0, |i| i as u32 + 1);
    let line_end = |at: u32| units[at as usize..].iter().position(|&u| u == u16::from(b'\n')).map_or(len, |i| at + i as u32);
    let caret = |at: u32| Some(FieldAction::Select { start: at, end: at });

    if let Some(c) = press.text() {
        return Some(replace(c.encode_utf8(&mut [0; 4])));
    }
    if press.shift && press.key.starts_with("Arrow") || press.shift && matches!(press.key.as_str(), "Home" | "End") {
        return None;
    }
    match press.key.as_str() {
        "a" | "A" if shortcut => Some(FieldAction::Select { start: 0, end: len }),
        _ if shortcut || press.alt => None,
        "Enter" if multiline => Some(replace("\n")),
        "Backspace" if start != end => Some(remove(start, end)),
        "Backspace" if start > 0 => Some(remove(prev(start), start)),
        "Delete" if start != end => Some(remove(start, end)),
        "Delete" if start < len => Some(remove(start, next(start))),
        "ArrowLeft" if start != end => caret(start),
        "ArrowLeft" => caret(prev(start)),
        "ArrowRight" if start != end => caret(end),
        "ArrowRight" => caret(next(end)),
        "Home" => caret(if multiline { line_start(start) } else { 0 }),
        "End" => caret(if multiline { line_end(end) } else { len }),
        // A single-line field: up/down go to the ends, as browsers do.
        "ArrowUp" if !multiline => caret(0),
        "ArrowDown" if !multiline => caret(len),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_vocabulary::caps::KeyChord;

    fn press(spec: &str) -> KeyPress {
        KeyChord::parse(spec).unwrap().resolve(true)
    }

    fn edit(value: &str, caret: u32) -> Option<FieldAction> {
        Some(FieldAction::Edit { value: value.into(), caret })
    }

    fn at(start: u32, end: u32) -> Option<FieldAction> {
        Some(FieldAction::Select { start, end })
    }

    #[test]
    fn typing_replaces_the_selection() {
        assert_eq!(field_default("ac", 1, 1, &press("b"), false), edit("abc", 2));
        assert_eq!(field_default("hello", 0, 5, &press("X"), false), edit("X", 1));
        assert_eq!(field_default("a", 1, 1, &press("Space"), false), edit("a ", 2));
        assert_eq!(field_default("a", 1, 1, &press("Meta+K"), false), None, "a shortcut types nothing");
    }

    #[test]
    fn backspace_and_delete_remove_one_character_or_the_selection() {
        assert_eq!(field_default("abc", 3, 3, &press("Backspace"), false), edit("ab", 2));
        assert_eq!(field_default("abc", 0, 0, &press("Backspace"), false), None);
        assert_eq!(field_default("abc", 1, 3, &press("Backspace"), false), edit("a", 1));
        assert_eq!(field_default("abc", 0, 0, &press("Delete"), false), edit("bc", 0));
        assert_eq!(field_default("abc", 3, 3, &press("Delete"), false), None);
        assert_eq!(field_default("a😀", 3, 3, &press("Backspace"), false), edit("a", 1), "a surrogate pair is one character");
    }

    #[test]
    fn enter_types_a_newline_only_in_a_text_area() {
        assert_eq!(field_default("ab", 1, 1, &press("Enter"), true), edit("a\nb", 2));
        assert_eq!(field_default("ab", 1, 1, &press("Enter"), false), None);
    }

    #[test]
    fn arrows_home_end_and_select_all_move_the_selection() {
        assert_eq!(field_default("abc", 2, 2, &press("ArrowLeft"), false), at(1, 1));
        assert_eq!(field_default("abc", 1, 2, &press("ArrowRight"), false), at(2, 2));
        assert_eq!(field_default("abc", 2, 2, &press("Home"), false), at(0, 0));
        assert_eq!(field_default("abc", 0, 0, &press("End"), false), at(3, 3));
        assert_eq!(field_default("ab\ncd", 4, 4, &press("Home"), true), at(3, 3));
        assert_eq!(field_default("ab\ncd", 1, 1, &press("End"), true), at(2, 2));
        assert_eq!(field_default("abc", 1, 1, &press("ArrowDown"), false), at(3, 3));
        assert_eq!(field_default("abc", 1, 1, &press("ControlOrMeta+a"), false), at(0, 3));
        assert_eq!(field_default("abc", 1, 1, &press("Tab"), false), None);
        assert_eq!(field_default("abc", 1, 1, &press("Escape"), false), None);
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod browser_tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use runtime_vocabulary::caps::KeyChord;
    use wasm_bindgen_test::*;
    use web_glue::JsCast;

    web_glue::import! {
        fn js_is_active(n: u32) -> u32 = "(n) => document.activeElement === G.get(n) ? 1 : 0";
    }

    fn press(spec: &str) {
        super::native_key(&KeyChord::parse(spec).unwrap().resolve(false))().unwrap();
    }

    /// The robot's native key on web: the field's and the window's
    /// listeners hear keydown / keyup; an unprevented character types and
    /// fires `input`; Backspace deletes; Enter in a single-line field
    /// types nothing; Escape reaches a window listener; Tab moves focus.
    #[wasm_bindgen_test]
    fn a_native_key_reaches_listeners_and_performs_the_default() {
        let doc = web_glue::dom::window().unwrap().document().unwrap();
        let root = doc.create_element("div").unwrap();
        doc.body().unwrap().append_child(&root).unwrap();
        let first: web_glue::dom::HtmlInputElement = doc.create_element("input").unwrap().unchecked_into();
        let second: web_glue::dom::HtmlInputElement = doc.create_element("input").unwrap().unchecked_into();
        root.append_child(&first).unwrap();
        root.append_child(&second).unwrap();

        let seen: Rc<RefCell<Vec<String>>> = Rc::default();
        let mut keep = Vec::new();
        for kind in ["keydown", "keyup", "input"] {
            let seen = seen.clone();
            let cb = web_glue::Closure::new(move |e: web_glue::JsValue| {
                let key = e.dyn_into::<web_glue::dom::KeyboardEvent>().map(|k| k.key()).unwrap_or_default();
                seen.borrow_mut().push(format!("{kind}:{key}"));
            });
            let t: &web_glue::dom::EventTarget = first.as_ref();
            t.add_event_listener(kind, &cb, web_glue::dom::ListenerOptions::default());
            keep.push(cb);
        }
        let escapes = Rc::new(RefCell::new(0));
        let on_window = {
            let escapes = escapes.clone();
            web_glue::Closure::new(move |e: web_glue::JsValue| {
                if e.unchecked_into::<web_glue::dom::KeyboardEvent>().key() == "Escape" {
                    *escapes.borrow_mut() += 1;
                }
            })
        };
        let window = web_glue::dom::window().unwrap();
        let w: &web_glue::dom::EventTarget = window.as_ref();
        w.add_event_listener("keydown", &on_window, web_glue::dom::ListenerOptions::default());

        first.focus().unwrap();
        press("h");
        press("i");
        press("Backspace");
        press("Enter");
        assert_eq!(first.value(), "h");
        assert_eq!(
            seen.borrow().as_slice(),
            [
                "keydown:h", "input:", "keyup:h", "keydown:i", "input:", "keyup:i", "keydown:Backspace", "input:",
                "keyup:Backspace", "keydown:Enter", "keyup:Enter"
            ]
        );
        press("Escape");
        assert_eq!(*escapes.borrow(), 1, "Escape bubbles to the window");
        press("Tab");
        let raw = AsRef::<web_glue::JsValue>::as_ref(&second).raw();
        assert_eq!(unsafe { js_is_active(raw) }, 1, "Tab moves focus to the next field");
        root.remove();
    }
}
