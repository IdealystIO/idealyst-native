//! Pure halves of the iOS app-level keyboard source (`imp/keyboard.rs`):
//! the `UIKeyboardHIDUsage` → Web `KeyboardEvent.code` / `.key` tables, the
//! `UIKey` → [`AppKeyEvent`] translation, and the per-press forwarding ledger
//! that keeps `pressesBegan:` / `pressesEnded:` consistent for UIKit.
//!
//! Un-gated (no `target_os`) so the regression tests run from any host; the
//! `UIResponder` subclass that feeds these is ios-only.

use std::collections::HashSet;

use runtime_shared::primitives::key::{AppKeyEvent, KeyPhase};

// `UIKeyModifierFlags` bits.
pub(crate) const FLAG_SHIFT: isize = 1 << 17;
pub(crate) const FLAG_CONTROL: isize = 1 << 18;
pub(crate) const FLAG_ALTERNATE: isize = 1 << 19;
pub(crate) const FLAG_COMMAND: isize = 1 << 20;

/// Physical key for a `UIKeyboardHIDUsage` (USB HID usage page 0x07,
/// "Keyboard/Keypad"), in Web `KeyboardEvent.code` vocabulary — the same
/// mapping the W3C UI Events code spec and Chromium use. `""` when unknown.
pub(crate) fn physical_code(usage: isize) -> &'static str {
    const LETTERS: [&str; 26] = [
        "KeyA", "KeyB", "KeyC", "KeyD", "KeyE", "KeyF", "KeyG", "KeyH", "KeyI", "KeyJ", "KeyK", "KeyL", "KeyM",
        "KeyN", "KeyO", "KeyP", "KeyQ", "KeyR", "KeyS", "KeyT", "KeyU", "KeyV", "KeyW", "KeyX", "KeyY", "KeyZ",
    ];
    // HID orders the digit row 1..9 then 0.
    const DIGITS: [&str; 10] =
        ["Digit1", "Digit2", "Digit3", "Digit4", "Digit5", "Digit6", "Digit7", "Digit8", "Digit9", "Digit0"];
    const F1_F12: [&str; 12] = ["F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12"];
    const F13_F24: [&str; 12] = ["F13", "F14", "F15", "F16", "F17", "F18", "F19", "F20", "F21", "F22", "F23", "F24"];
    // Keypad 1..9 then 0, like the digit row.
    const NUMPAD: [&str; 10] = [
        "Numpad1", "Numpad2", "Numpad3", "Numpad4", "Numpad5", "Numpad6", "Numpad7", "Numpad8", "Numpad9", "Numpad0",
    ];
    match usage {
        0x04..=0x1D => LETTERS[(usage - 0x04) as usize],
        0x1E..=0x27 => DIGITS[(usage - 0x1E) as usize],
        0x28 => "Enter",
        0x29 => "Escape",
        0x2A => "Backspace", // "Delete or Backspace"
        0x2B => "Tab",
        0x2C => "Space",
        0x2D => "Minus",
        0x2E => "Equal",
        0x2F => "BracketLeft",
        0x30 => "BracketRight",
        0x31 => "Backslash",
        0x32 => "Backslash", // Non-US # — same physical position (W3C)
        0x33 => "Semicolon",
        0x34 => "Quote",
        0x35 => "Backquote",
        0x36 => "Comma",
        0x37 => "Period",
        0x38 => "Slash",
        0x39 => "CapsLock",
        0x3A..=0x45 => F1_F12[(usage - 0x3A) as usize],
        0x46 => "PrintScreen",
        0x47 => "ScrollLock",
        0x48 => "Pause",
        0x49 => "Insert",
        0x4A => "Home",
        0x4B => "PageUp",
        0x4C => "Delete", // "Delete Forward"
        0x4D => "End",
        0x4E => "PageDown",
        0x4F => "ArrowRight",
        0x50 => "ArrowLeft",
        0x51 => "ArrowDown",
        0x52 => "ArrowUp",
        0x53 => "NumLock",
        0x54 => "NumpadDivide",
        0x55 => "NumpadMultiply",
        0x56 => "NumpadSubtract",
        0x57 => "NumpadAdd",
        0x58 => "NumpadEnter",
        0x59..=0x62 => NUMPAD[(usage - 0x59) as usize],
        0x63 => "NumpadDecimal",
        0x64 => "IntlBackslash",
        0x65 => "ContextMenu",
        0x66 => "Power",
        0x67 => "NumpadEqual",
        0x68..=0x73 => F13_F24[(usage - 0x68) as usize],
        0x7F => "AudioVolumeMute",
        0x80 => "AudioVolumeUp",
        0x81 => "AudioVolumeDown",
        0x85 => "NumpadComma",
        0x87 => "IntlRo",
        0x88 => "KanaMode",
        0x89 => "IntlYen",
        0x90 => "Lang1",
        0x91 => "Lang2",
        0xE0 => "ControlLeft",
        0xE1 => "ShiftLeft",
        0xE2 => "AltLeft",
        0xE3 => "MetaLeft",
        0xE4 => "ControlRight",
        0xE5 => "ShiftRight",
        0xE6 => "AltRight",
        0xE7 => "MetaRight",
        _ => "",
    }
}

/// Web `KeyboardEvent.key` for the keys whose `key` is a NAME rather than the
/// character they type. Printable keys return `None` and use the `UIKey`'s
/// `charactersIgnoringModifiers`. Modifiers MUST be named here (their
/// characters are empty), and so must arrows / function keys (UIKit reports
/// them as `"UIKeyInputUpArrow"` / `"UIKeyInputF1"` strings).
pub(crate) fn key_name(usage: isize) -> Option<&'static str> {
    Some(match usage {
        0x50 => "ArrowLeft",
        0x4F => "ArrowRight",
        0x52 => "ArrowUp",
        0x51 => "ArrowDown",
        0x28 | 0x58 => "Enter",
        0x2B => "Tab",
        0x2C => " ",
        0x2A => "Backspace",
        0x29 => "Escape",
        0x4C => "Delete",
        0x49 => "Insert",
        0x4A => "Home",
        0x4D => "End",
        0x4B => "PageUp",
        0x4E => "PageDown",
        0x39 => "CapsLock",
        0x46 => "PrintScreen",
        0x47 => "ScrollLock",
        0x48 => "Pause",
        0x53 => "NumLock",
        0x65 => "ContextMenu",
        0xE0 | 0xE4 => "Control",
        0xE1 | 0xE5 => "Shift",
        0xE2 | 0xE6 => "Alt",
        0xE3 | 0xE7 => "Meta",
        0x3A..=0x45 | 0x68..=0x73 => return Some(physical_code(usage)), // "F1".."F24"
        _ => return None,
    })
}

/// Translate one `UIKey` (its HID usage, modifier flags and
/// `charactersIgnoringModifiers`) into an [`AppKeyEvent`]. iOS doesn't flag
/// auto-repeats, so `repeat` is always `false` here — the dispatcher marks a
/// down for an already-held key as a repeat.
pub(crate) fn translate(phase: KeyPhase, usage: isize, flags: isize, chars: Option<&str>) -> AppKeyEvent {
    let key = match key_name(usage) {
        Some(name) => name.to_string(),
        None => match chars {
            // UIKit's named-key strings ("UIKeyInputF13"…) aren't `key`
            // values a web listener would match.
            Some(s) if s.starts_with("UIKeyInput") => "Unidentified".to_string(),
            Some(s) if !s.is_empty() => s.to_string(),
            _ => "Unidentified".to_string(),
        },
    };
    AppKeyEvent {
        phase,
        key,
        code: physical_code(usage).to_string(),
        repeat: false,
        shift: flags & FLAG_SHIFT != 0,
        ctrl: flags & FLAG_CONTROL != 0,
        alt: flags & FLAG_ALTERNATE != 0,
        meta: flags & FLAG_COMMAND != 0,
    }
}

/// Which presses the responder swallowed at `pressesBegan:`, so the rest of
/// each press's lifecycle is routed the same way.
///
/// UIKit's rule for a responder that overrides the `presses*` methods: if it
/// doesn't forward a press's `began` to `super`, it must not forward that
/// press's `ended`/`cancelled` either (and vice versa) — the next responder
/// would otherwise see an end for a press it never saw begin, or a begin that
/// never ends. A `UIPress` object is stable for its whole lifecycle, so it is
/// keyed by its address.
#[derive(Default, Debug)]
pub(crate) struct PressLedger {
    swallowed: HashSet<usize>,
}

impl PressLedger {
    /// Record a press's `began`. Returns whether to FORWARD it to `super`.
    pub(crate) fn began(&mut self, press: usize, swallow: bool) -> bool {
        if swallow {
            self.swallowed.insert(press);
            false
        } else {
            self.swallowed.remove(&press);
            true
        }
    }

    /// A press ended or was cancelled. Returns whether to FORWARD it to
    /// `super` — exactly when its `began` was forwarded.
    pub(crate) fn ended(&mut self, press: usize) -> bool {
        !self.swallowed.remove(&press)
    }

    /// The responder lost first-responder status: the ends of presses it
    /// swallowed will go to whichever responder UIKit picks next, not here.
    pub(crate) fn clear(&mut self) {
        self.swallowed.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_code_table_matches_web_code_vocabulary() {
        for (usage, code) in [
            (0x04, "KeyA"),
            (0x1A, "KeyW"),
            (0x16, "KeyS"),
            (0x07, "KeyD"),
            (0x1D, "KeyZ"),
            (0x1E, "Digit1"),
            (0x26, "Digit9"),
            (0x27, "Digit0"),
            (0x28, "Enter"),
            (0x29, "Escape"),
            (0x2A, "Backspace"),
            (0x2B, "Tab"),
            (0x2C, "Space"),
            (0x2D, "Minus"),
            (0x2E, "Equal"),
            (0x2F, "BracketLeft"),
            (0x30, "BracketRight"),
            (0x31, "Backslash"),
            (0x33, "Semicolon"),
            (0x34, "Quote"),
            (0x35, "Backquote"),
            (0x36, "Comma"),
            (0x37, "Period"),
            (0x38, "Slash"),
            (0x39, "CapsLock"),
            (0x3A, "F1"),
            (0x45, "F12"),
            (0x68, "F13"),
            (0x73, "F24"),
            (0x49, "Insert"),
            (0x4A, "Home"),
            (0x4B, "PageUp"),
            (0x4C, "Delete"),
            (0x4D, "End"),
            (0x4E, "PageDown"),
            (0x4F, "ArrowRight"),
            (0x50, "ArrowLeft"),
            (0x51, "ArrowDown"),
            (0x52, "ArrowUp"),
            (0x57, "NumpadAdd"),
            (0x58, "NumpadEnter"),
            (0x59, "Numpad1"),
            (0x61, "Numpad9"),
            (0x62, "Numpad0"),
            (0x63, "NumpadDecimal"),
            (0xE0, "ControlLeft"),
            (0xE1, "ShiftLeft"),
            (0xE2, "AltLeft"),
            (0xE3, "MetaLeft"),
            (0xE4, "ControlRight"),
            (0xE5, "ShiftRight"),
            (0xE6, "AltRight"),
            (0xE7, "MetaRight"),
        ] {
            assert_eq!(physical_code(usage), code, "usage 0x{usage:02X}");
        }
        assert_eq!(physical_code(0x00), "");
        assert_eq!(physical_code(0x03), "");
        assert_eq!(physical_code(0xFF), "");
    }

    /// REGRESSION (game controls): the app-level hook was key-down only with
    /// no physical code. A press and its release now translate to Down/Up with
    /// the same `code`, even when the character changed with Shift.
    #[test]
    fn regression_press_and_release_carry_physical_code() {
        let down = translate(KeyPhase::Down, 0x1A, FLAG_SHIFT, Some("W"));
        assert_eq!((down.phase, down.key.as_str(), down.code.as_str()), (KeyPhase::Down, "W", "KeyW"));
        assert!(down.shift && !down.repeat);
        let up = translate(KeyPhase::Up, 0x1A, 0, Some("w"));
        assert_eq!((up.phase, up.key.as_str(), up.code.as_str()), (KeyPhase::Up, "w", "KeyW"));
        assert!(!up.shift);
    }

    /// REGRESSION (game controls): modifier keys must come through as keys
    /// in their own right so Shift works as a held "run" key. Their UIKey
    /// characters are empty, so the name has to come from the table.
    #[test]
    fn regression_modifier_keys_are_named_keys() {
        for (usage, key, code) in [
            (0xE1, "Shift", "ShiftLeft"),
            (0xE5, "Shift", "ShiftRight"),
            (0xE0, "Control", "ControlLeft"),
            (0xE2, "Alt", "AltLeft"),
            (0xE3, "Meta", "MetaLeft"),
            (0xE7, "Meta", "MetaRight"),
        ] {
            let e = translate(KeyPhase::Down, usage, 0, Some(""));
            assert_eq!((e.key.as_str(), e.code.as_str()), (key, code));
        }
    }

    #[test]
    fn named_keys_and_uikit_named_strings() {
        let space = translate(KeyPhase::Down, 0x2C, 0, Some(" "));
        assert_eq!((space.key.as_str(), space.code.as_str()), (" ", "Space"));
        let arrow = translate(KeyPhase::Down, 0x52, 0, Some("UIKeyInputUpArrow"));
        assert_eq!((arrow.key.as_str(), arrow.code.as_str()), ("ArrowUp", "ArrowUp"));
        let f5 = translate(KeyPhase::Down, 0x3E, 0, Some("UIKeyInputF5"));
        assert_eq!((f5.key.as_str(), f5.code.as_str()), ("F5", "F5"));
        let odd = translate(KeyPhase::Down, 0x01, 0, Some("UIKeyInputSomething"));
        assert_eq!((odd.key.as_str(), odd.code.as_str()), ("Unidentified", ""));
        let keypad_enter = translate(KeyPhase::Down, 0x58, 0, Some("\r"));
        assert_eq!((keypad_enter.key.as_str(), keypad_enter.code.as_str()), ("Enter", "NumpadEnter"));
        let modded = translate(KeyPhase::Down, 0x06, FLAG_COMMAND | FLAG_ALTERNATE, Some("c"));
        assert!(modded.meta && modded.alt && !modded.ctrl && !modded.shift);
    }

    /// A swallowed `began` swallows its `ended`; a forwarded one forwards its
    /// `ended` — UIKit must never see an end without a begin (or a begin that
    /// never ends) for the same press.
    #[test]
    fn press_ledger_routes_end_like_its_begin() {
        let mut ledger = PressLedger::default();
        assert!(!ledger.began(1, true), "swallowed begin");
        assert!(ledger.began(2, false), "forwarded begin");
        assert!(!ledger.ended(1), "swallowed press's end is swallowed too");
        assert!(ledger.ended(2), "forwarded press's end is forwarded");
        assert!(ledger.ended(1), "entry consumed — an unknown press forwards");
        ledger.began(3, true);
        ledger.clear();
        assert!(ledger.ended(3), "cleared on resign");
    }
}
