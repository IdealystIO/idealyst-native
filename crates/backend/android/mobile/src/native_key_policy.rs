//! The robot's key press (`caps::KeyPress`) → the Android `KeyEvent` a
//! hardware keyboard would deliver: its `KEYCODE_*` and `metaState`, or —
//! for a character no key types (`'é'`) — the `ACTION_MULTIPLE` characters
//! event the platform uses for committed text. Pure, so the tables are
//! tested on the host; the JNI delivery lives in `imp/native_key.rs`.
//!
//! The keycode comes from inverting [`keycode_code_name`], the table the
//! app-level key source reads, so a press round-trips: the robot's `code`
//! becomes the keycode whose `code` the app then reports.

use crate::app_key_policy::keycode_code_name;
use runtime_vocabulary::caps::KeyPress;

// KeyEvent.META_* (KeyEvent.java). A real keyboard sets the side bit with
// the general one; `getMetaState()` consumers test either.
const META_SHIFT_ON: i32 = 0x1;
const META_SHIFT_LEFT_ON: i32 = 0x40;
const META_ALT_ON: i32 = 0x2;
const META_ALT_LEFT_ON: i32 = 0x10;
const META_CTRL_ON: i32 = 0x1000;
const META_CTRL_LEFT_ON: i32 = 0x2000;
const META_META_ON: i32 = 0x10000;
const META_META_LEFT_ON: i32 = 0x20000;

/// Highest `KEYCODE_*` the inversion searches (`keycode_code_name` names
/// nothing above `KEYCODE_NUMPAD_RIGHT_PAREN` = 163).
const MAX_KEYCODE: i32 = 300;

/// How a press is delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KeyPlan {
    /// `ACTION_DOWN` then `ACTION_UP` of `key_code` with `meta_state`.
    Key { key_code: i32, meta_state: i32 },
    /// One `KeyEvent(time, characters, deviceId, flags)` (`ACTION_MULTIPLE`,
    /// `KEYCODE_UNKNOWN`): text with no key behind it, which a focused
    /// `EditText` inserts.
    Characters(String),
}

pub(crate) fn meta_state(press: &KeyPress) -> i32 {
    let mut m = 0;
    if press.shift {
        m |= META_SHIFT_ON | META_SHIFT_LEFT_ON;
    }
    if press.alt {
        m |= META_ALT_ON | META_ALT_LEFT_ON;
    }
    if press.ctrl {
        m |= META_CTRL_ON | META_CTRL_LEFT_ON;
    }
    if press.meta {
        m |= META_META_ON | META_META_LEFT_ON;
    }
    m
}

/// The `KEYCODE_*` whose Web `code` is `code`.
pub(crate) fn key_code_for(code: &str) -> Option<i32> {
    if code.is_empty() {
        return None;
    }
    (0..=MAX_KEYCODE).find(|&k| keycode_code_name(k) == Some(code))
}

/// `None` when the press has neither a key nor text Android can deliver
/// (a named key with no keycode, e.g. `F13`).
pub(crate) fn plan(press: &KeyPress) -> Option<KeyPlan> {
    if let Some(key_code) = key_code_for(&press.code) {
        return Some(KeyPlan::Key { key_code, meta_state: meta_state(press) });
    }
    press.text().map(|c| KeyPlan::Characters(c.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_vocabulary::caps::KeyChord;

    fn plan_of(spec: &str) -> Option<KeyPlan> {
        plan(&KeyChord::parse(spec).unwrap().resolve(false))
    }

    #[test]
    fn named_keys_map_to_their_keycodes() {
        let key = |spec, key_code| assert_eq!(plan_of(spec), Some(KeyPlan::Key { key_code, meta_state: 0 }), "{spec}");
        key("Enter", 66);
        key("Escape", 111);
        key("Tab", 61);
        key("Backspace", 67); // KEYCODE_DEL
        key("Delete", 112); // KEYCODE_FORWARD_DEL
        key("ArrowUp", 19);
        key("ArrowDown", 20);
        key("ArrowLeft", 21);
        key("ArrowRight", 22);
        key("Home", 122);
        key("End", 123);
        key("PageUp", 92);
        key("PageDown", 93);
        key("Space", 62);
        key("F1", 131);
        key("F12", 142);
        key("a", 29);
        key("z", 54);
        key("0", 7);
        key("9", 16);
        key("/", 76);
    }

    #[test]
    fn modifiers_set_the_general_and_left_meta_bits() {
        assert_eq!(
            plan_of("Meta+K"),
            Some(KeyPlan::Key { key_code: 39, meta_state: META_META_ON | META_META_LEFT_ON })
        );
        // A capital or a shifted symbol carries Shift, as the keyboard types it.
        assert_eq!(plan_of("A"), Some(KeyPlan::Key { key_code: 29, meta_state: META_SHIFT_ON | META_SHIFT_LEFT_ON }));
        assert_eq!(plan_of("!"), Some(KeyPlan::Key { key_code: 8, meta_state: META_SHIFT_ON | META_SHIFT_LEFT_ON }));
        let all = plan(&KeyChord::parse("Control+Alt+Shift+Meta+x").unwrap().resolve(false)).unwrap();
        let KeyPlan::Key { meta_state, .. } = all else { panic!("{all:?}") };
        for bit in [META_SHIFT_ON, META_ALT_ON, META_CTRL_ON, META_META_ON] {
            assert_ne!(meta_state & bit, 0);
        }
    }

    #[test]
    fn every_key_round_trips_through_the_app_key_table() {
        for spec in ["Enter", "Escape", "q", "7", "`", "Shift", "Control", "Meta", "Alt", "Insert", "CapsLock"] {
            let press = KeyChord::parse(spec).unwrap().resolve(false);
            let Some(KeyPlan::Key { key_code, .. }) = plan(&press) else { panic!("{spec} has no keycode") };
            assert_eq!(keycode_code_name(key_code), Some(press.code.as_str()), "{spec}");
        }
    }

    #[test]
    fn characters_without_a_key_are_committed_as_text() {
        assert_eq!(plan_of("é"), Some(KeyPlan::Characters("é".into())));
        assert_eq!(plan_of("F13"), None, "no keycode and no text");
    }
}
