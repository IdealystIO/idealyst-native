//! Android `KeyEvent` → framework key vocabulary: the Web
//! `KeyboardEvent.key` name ([`key_name`]), the physical Web
//! `KeyboardEvent.code` ([`physical_code`]), the modifier bits
//! ([`Modifiers`]) and the whole app-level [`AppKeyEvent`]
//! ([`app_key_event`]). Pure functions over the integers the Kotlin
//! listeners pass across JNI, un-gated like `sticky_compute` so the tables'
//! tests run on the host. The JNI trampolines that call them live in
//! `imp/jni_exports.rs` (`nativeGlobalKey` for the app-level source,
//! `nativeKey` for a focused `EditText`).
//!
//! # Where `code` comes from — scan code first, keycode second
//!
//! `code` must name the PHYSICAL key, independent of the keyboard layout
//! (WASD stays put on AZERTY). Android's `KeyEvent.getKeyCode()` is NOT
//! reliably layout-independent: the per-language keyboard layouts selected
//! in Settings are key character map overlays that may contain
//! `map key <scancode> <KEYCODE>` lines, and the French/German/… layouts do
//! (on AZERTY the physical Q key reports `KEYCODE_A`). The raw
//! `KeyEvent.getScanCode()` is the Linux evdev code
//! (`input-event-codes.h`), which the kernel derives from the HID usage
//! before any Android layout runs — that is the physical key. So:
//!
//! 1. scan code known to [`scan_code_name`] → that name;
//! 2. otherwise (scan code 0 — an injected `adb input keyevent`, an IME- or
//!    accessibility-synthesized event — or a scan code outside the keyboard
//!    block, e.g. a gamepad `BTN_*`) → the keycode table
//!    [`keycode_code_name`], which is right for US-QWERTY and for every
//!    layout-invariant key (arrows, modifiers, F-keys, Enter, …);
//! 3. otherwise `""` (the dispatcher then tracks the key by `key`).

use runtime_shared::primitives::key::{AppKeyEvent, KeyPhase};

/// `KeyEvent.META_*` bits (stable ABI, `KeyEvent.java`). The `*_ON` masks
/// cover either the left or right variant of the modifier.
const META_SHIFT_ON: i32 = 0x1;
const META_ALT_ON: i32 = 0x2;
const META_CTRL_ON: i32 = 0x1000;
const META_META_ON: i32 = 0x10000;

/// Modifier state decoded from `KeyEvent.getMetaState()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Modifiers {
    pub(crate) shift: bool,
    pub(crate) ctrl: bool,
    pub(crate) alt: bool,
    pub(crate) meta: bool,
}

impl Modifiers {
    pub(crate) fn from_meta_state(meta_state: i32) -> Self {
        Modifiers {
            shift: meta_state & META_SHIFT_ON != 0,
            ctrl: meta_state & META_CTRL_ON != 0,
            alt: meta_state & META_ALT_ON != 0,
            meta: meta_state & META_META_ON != 0,
        }
    }
}

/// Build the app-level event the `RustGlobalKeyListener` trampoline hands
/// to the `KeyboardSink`. `repeat` is Android's `repeatCount > 0`; it is
/// forced `false` on a release (the contract: never a repeating up).
pub(crate) fn app_key_event(
    up: bool,
    repeat: bool,
    key_code: i32,
    scan_code: i32,
    meta_state: i32,
    unicode_char: i32,
) -> AppKeyEvent {
    let m = Modifiers::from_meta_state(meta_state);
    AppKeyEvent {
        phase: if up { KeyPhase::Up } else { KeyPhase::Down },
        key: key_name(key_code, unicode_char, meta_state),
        code: physical_code(key_code, scan_code).to_string(),
        repeat: repeat && !up,
        shift: m.shift,
        ctrl: m.ctrl,
        alt: m.alt,
        meta: m.meta,
    }
}

/// Web `KeyboardEvent.key` for an Android key event.
///
/// Named (non-printing) keys come from the keycode. Printable keys use
/// `unicodeChar`, which already folds in Shift / CapsLock (so a shifted
/// letter is `"A"`); it is available on `ACTION_UP` as well as
/// `ACTION_DOWN`. When `unicodeChar` is 0 for a key that still has a
/// character on the web (Android's key character map yields no character
/// for most Ctrl/Meta chords, where the web reports `key: "a"` with
/// `ctrlKey`), the US-layout character is derived from the keycode.
pub(crate) fn key_name(key_code: i32, unicode_char: i32, meta_state: i32) -> String {
    if let Some(name) = named_key(key_code) {
        return name.to_string();
    }
    if unicode_char > 0 {
        if let Some(c) = char::from_u32(unicode_char as u32) {
            return c.to_string();
        }
    }
    chord_fallback_char(key_code, meta_state)
        .map(|c| c.to_string())
        .unwrap_or_default()
}

/// `KEYCODE_*` → Web `key` for keys whose `key` is a name, not a character.
fn named_key(key_code: i32) -> Option<&'static str> {
    Some(match key_code {
        61 => "Tab",
        66 | 160 => "Enter", // ENTER, NUMPAD_ENTER
        111 => "Escape",
        67 => "Backspace", // KEYCODE_DEL is Android's Backspace
        112 => "Delete",   // KEYCODE_FORWARD_DEL
        19 => "ArrowUp",
        20 => "ArrowDown",
        21 => "ArrowLeft",
        22 => "ArrowRight",
        122 => "Home",
        123 => "End",
        92 => "PageUp",
        93 => "PageDown",
        124 => "Insert",
        59 | 60 => "Shift",
        57 | 58 => "Alt",
        113 | 114 => "Control",
        117 | 118 => "Meta",
        115 => "CapsLock",
        143 => "NumLock",
        116 => "ScrollLock",
        120 => "PrintScreen", // KEYCODE_SYSRQ
        121 => "Pause",       // KEYCODE_BREAK
        82 => "ContextMenu",  // KEYCODE_MENU
        119 => "Fn",          // KEYCODE_FUNCTION
        131..=142 => F_KEYS[(key_code - 131) as usize],
        _ => return None,
    })
}

const F_KEYS: [&str; 12] = [
    "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12",
];

/// The character a letter / digit / space key types on a US layout, for
/// chords where Android reports `unicodeChar == 0`.
fn chord_fallback_char(key_code: i32, meta_state: i32) -> Option<char> {
    let shift = meta_state & META_SHIFT_ON != 0;
    match key_code {
        29..=54 => {
            let c = (b'a' + (key_code - 29) as u8) as char;
            Some(if shift { c.to_ascii_uppercase() } else { c })
        }
        7..=16 => Some((b'0' + (key_code - 7) as u8) as char),
        62 => Some(' '),
        _ => None,
    }
}

/// Web `KeyboardEvent.code` (the physical key) — see the module docs for
/// why the scan code wins over the keycode. `""` when neither identifies it.
pub(crate) fn physical_code(key_code: i32, scan_code: i32) -> &'static str {
    scan_code_name(scan_code)
        .or_else(|| keycode_code_name(key_code))
        .unwrap_or("")
}

/// Linux evdev `KEY_*` (`input-event-codes.h`, what `KeyEvent.getScanCode()`
/// returns for a hardware keyboard) → Web `code`. Only the keyboard block;
/// `0` (`KEY_RESERVED`, also "no scan code") and anything unlisted is `None`.
pub(crate) fn scan_code_name(scan_code: i32) -> Option<&'static str> {
    Some(match scan_code {
        1 => "Escape",
        2 => "Digit1",
        3 => "Digit2",
        4 => "Digit3",
        5 => "Digit4",
        6 => "Digit5",
        7 => "Digit6",
        8 => "Digit7",
        9 => "Digit8",
        10 => "Digit9",
        11 => "Digit0",
        12 => "Minus",
        13 => "Equal",
        14 => "Backspace",
        15 => "Tab",
        16 => "KeyQ",
        17 => "KeyW",
        18 => "KeyE",
        19 => "KeyR",
        20 => "KeyT",
        21 => "KeyY",
        22 => "KeyU",
        23 => "KeyI",
        24 => "KeyO",
        25 => "KeyP",
        26 => "BracketLeft",
        27 => "BracketRight",
        28 => "Enter",
        29 => "ControlLeft",
        30 => "KeyA",
        31 => "KeyS",
        32 => "KeyD",
        33 => "KeyF",
        34 => "KeyG",
        35 => "KeyH",
        36 => "KeyJ",
        37 => "KeyK",
        38 => "KeyL",
        39 => "Semicolon",
        40 => "Quote",
        41 => "Backquote",
        42 => "ShiftLeft",
        43 => "Backslash",
        44 => "KeyZ",
        45 => "KeyX",
        46 => "KeyC",
        47 => "KeyV",
        48 => "KeyB",
        49 => "KeyN",
        50 => "KeyM",
        51 => "Comma",
        52 => "Period",
        53 => "Slash",
        54 => "ShiftRight",
        55 => "NumpadMultiply",
        56 => "AltLeft",
        57 => "Space",
        58 => "CapsLock",
        59..=68 => F_KEYS[(scan_code - 59) as usize], // F1..F10
        69 => "NumLock",
        70 => "ScrollLock",
        71 => "Numpad7",
        72 => "Numpad8",
        73 => "Numpad9",
        74 => "NumpadSubtract",
        75 => "Numpad4",
        76 => "Numpad5",
        77 => "Numpad6",
        78 => "NumpadAdd",
        79 => "Numpad1",
        80 => "Numpad2",
        81 => "Numpad3",
        82 => "Numpad0",
        83 => "NumpadDecimal",
        86 => "IntlBackslash", // KEY_102ND
        87 => "F11",
        88 => "F12",
        89 => "IntlRo",
        96 => "NumpadEnter",
        97 => "ControlRight",
        98 => "NumpadDivide",
        99 => "PrintScreen", // KEY_SYSRQ
        100 => "AltRight",
        102 => "Home",
        103 => "ArrowUp",
        104 => "PageUp",
        105 => "ArrowLeft",
        106 => "ArrowRight",
        107 => "End",
        108 => "ArrowDown",
        109 => "PageDown",
        110 => "Insert",
        111 => "Delete",
        117 => "NumpadEqual",
        119 => "Pause",
        121 => "NumpadComma",
        124 => "IntlYen",
        125 => "MetaLeft",
        126 => "MetaRight",
        127 => "ContextMenu", // KEY_COMPOSE
        _ => return None,
    })
}

/// `KeyEvent.KEYCODE_*` → Web `code`, the fallback when there is no usable
/// scan code. Letter / punctuation keycodes are named after their US-QWERTY
/// position (see the module docs for when that is wrong).
pub(crate) fn keycode_code_name(key_code: i32) -> Option<&'static str> {
    const LETTERS: [&str; 26] = [
        "KeyA", "KeyB", "KeyC", "KeyD", "KeyE", "KeyF", "KeyG", "KeyH", "KeyI", "KeyJ", "KeyK",
        "KeyL", "KeyM", "KeyN", "KeyO", "KeyP", "KeyQ", "KeyR", "KeyS", "KeyT", "KeyU", "KeyV",
        "KeyW", "KeyX", "KeyY", "KeyZ",
    ];
    const DIGITS: [&str; 10] = [
        "Digit0", "Digit1", "Digit2", "Digit3", "Digit4", "Digit5", "Digit6", "Digit7", "Digit8",
        "Digit9",
    ];
    const NUMPAD: [&str; 10] = [
        "Numpad0", "Numpad1", "Numpad2", "Numpad3", "Numpad4", "Numpad5", "Numpad6", "Numpad7",
        "Numpad8", "Numpad9",
    ];
    Some(match key_code {
        7..=16 => DIGITS[(key_code - 7) as usize],
        29..=54 => LETTERS[(key_code - 29) as usize],
        144..=153 => NUMPAD[(key_code - 144) as usize],
        131..=142 => F_KEYS[(key_code - 131) as usize],
        19 => "ArrowUp",
        20 => "ArrowDown",
        21 => "ArrowLeft",
        22 => "ArrowRight",
        55 => "Comma",
        56 => "Period",
        57 => "AltLeft",
        58 => "AltRight",
        59 => "ShiftLeft",
        60 => "ShiftRight",
        61 => "Tab",
        62 => "Space",
        66 => "Enter",
        67 => "Backspace",
        68 => "Backquote", // KEYCODE_GRAVE
        69 => "Minus",
        70 => "Equal",
        71 => "BracketLeft",
        72 => "BracketRight",
        73 => "Backslash",
        74 => "Semicolon",
        75 => "Quote", // KEYCODE_APOSTROPHE
        76 => "Slash",
        82 => "ContextMenu", // KEYCODE_MENU
        92 => "PageUp",
        93 => "PageDown",
        111 => "Escape",
        112 => "Delete", // KEYCODE_FORWARD_DEL
        113 => "ControlLeft",
        114 => "ControlRight",
        115 => "CapsLock",
        116 => "ScrollLock",
        117 => "MetaLeft",
        118 => "MetaRight",
        120 => "PrintScreen", // KEYCODE_SYSRQ
        121 => "Pause",       // KEYCODE_BREAK
        122 => "Home",        // KEYCODE_MOVE_HOME
        123 => "End",         // KEYCODE_MOVE_END
        124 => "Insert",
        143 => "NumLock",
        154 => "NumpadDivide",
        155 => "NumpadMultiply",
        156 => "NumpadSubtract",
        157 => "NumpadAdd",
        158 => "NumpadDecimal", // KEYCODE_NUMPAD_DOT
        159 => "NumpadComma",
        160 => "NumpadEnter",
        161 => "NumpadEqual",
        162 => "NumpadParenLeft",
        163 => "NumpadParenRight",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Android KEYCODE_* values used below (KeyEvent.java).
    const KEYCODE_0: i32 = 7;
    const KEYCODE_9: i32 = 16;
    const KEYCODE_A: i32 = 29;
    const KEYCODE_Q: i32 = 45;
    const KEYCODE_W: i32 = 51;
    const KEYCODE_Z: i32 = 54;
    const KEYCODE_SPACE: i32 = 62;
    const KEYCODE_SHIFT_LEFT: i32 = 59;
    const KEYCODE_DPAD_UP: i32 = 19;
    // Linux evdev KEY_* values (input-event-codes.h).
    const KEY_Q: i32 = 16;
    const KEY_A: i32 = 30;
    const KEY_W: i32 = 17;

    #[test]
    fn keycode_table_covers_letters_digits_numpad_and_fkeys() {
        assert_eq!(keycode_code_name(KEYCODE_A), Some("KeyA"));
        assert_eq!(keycode_code_name(KEYCODE_Z), Some("KeyZ"));
        assert_eq!(keycode_code_name(KEYCODE_W), Some("KeyW"));
        assert_eq!(keycode_code_name(KEYCODE_0), Some("Digit0"));
        assert_eq!(keycode_code_name(KEYCODE_9), Some("Digit9"));
        assert_eq!(keycode_code_name(144), Some("Numpad0"));
        assert_eq!(keycode_code_name(153), Some("Numpad9"));
        assert_eq!(keycode_code_name(131), Some("F1"));
        assert_eq!(keycode_code_name(142), Some("F12"));
        // Every letter keycode maps to a distinct KeyX in alphabet order.
        for (i, kc) in (29..=54).enumerate() {
            let expected = format!("Key{}", (b'A' + i as u8) as char);
            assert_eq!(keycode_code_name(kc), Some(expected.as_str()));
        }
    }

    #[test]
    fn keycode_table_names_game_and_modifier_keys() {
        let cases = [
            (19, "ArrowUp"),
            (20, "ArrowDown"),
            (21, "ArrowLeft"),
            (22, "ArrowRight"),
            (62, "Space"),
            (66, "Enter"),
            (111, "Escape"),
            (61, "Tab"),
            (67, "Backspace"),
            (112, "Delete"),
            (59, "ShiftLeft"),
            (60, "ShiftRight"),
            (113, "ControlLeft"),
            (114, "ControlRight"),
            (57, "AltLeft"),
            (58, "AltRight"),
            (117, "MetaLeft"),
            (118, "MetaRight"),
            (115, "CapsLock"),
            (122, "Home"),
            (123, "End"),
            (92, "PageUp"),
            (93, "PageDown"),
            (124, "Insert"),
            (160, "NumpadEnter"),
            (157, "NumpadAdd"),
            (69, "Minus"),
            (70, "Equal"),
            (71, "BracketLeft"),
            (72, "BracketRight"),
            (73, "Backslash"),
            (74, "Semicolon"),
            (75, "Quote"),
            (68, "Backquote"),
            (55, "Comma"),
            (56, "Period"),
            (76, "Slash"),
        ];
        for (kc, code) in cases {
            assert_eq!(keycode_code_name(kc), Some(code), "KEYCODE {kc}");
        }
        assert_eq!(keycode_code_name(0), None, "KEYCODE_UNKNOWN");
        assert_eq!(keycode_code_name(23), None, "DPAD_CENTER has no web code");
    }

    #[test]
    fn scan_code_table_matches_evdev_layout() {
        let cases = [
            (1, "Escape"),
            (2, "Digit1"),
            (11, "Digit0"),
            (KEY_Q, "KeyQ"),
            (KEY_W, "KeyW"),
            (KEY_A, "KeyA"),
            (31, "KeyS"),
            (32, "KeyD"),
            (44, "KeyZ"),
            (50, "KeyM"),
            (57, "Space"),
            (28, "Enter"),
            (42, "ShiftLeft"),
            (54, "ShiftRight"),
            (29, "ControlLeft"),
            (97, "ControlRight"),
            (56, "AltLeft"),
            (100, "AltRight"),
            (125, "MetaLeft"),
            (126, "MetaRight"),
            (59, "F1"),
            (68, "F10"),
            (87, "F11"),
            (88, "F12"),
            (103, "ArrowUp"),
            (108, "ArrowDown"),
            (105, "ArrowLeft"),
            (106, "ArrowRight"),
            (82, "Numpad0"),
            (96, "NumpadEnter"),
            (78, "NumpadAdd"),
            (110, "Insert"),
            (111, "Delete"),
        ];
        for (sc, code) in cases {
            assert_eq!(scan_code_name(sc), Some(code), "scan code {sc}");
        }
        assert_eq!(scan_code_name(0), None, "KEY_RESERVED = no scan code");
        assert_eq!(scan_code_name(0x130), None, "gamepad BTN_SOUTH is not a keyboard key");
    }

    /// Bug the scan-code-first rule prevents: on a French (AZERTY) layout
    /// Android's layout overlay remaps the physical Q key to `KEYCODE_A`.
    /// Deriving `code` from the keycode would move the game's "A" binding
    /// to the Q key — `code` must stay the physical key.
    #[test]
    fn regression_azerty_layout_remap_keeps_physical_code() {
        assert_eq!(physical_code(KEYCODE_A, KEY_Q), "KeyQ");
        assert_eq!(physical_code(KEYCODE_Q, KEY_A), "KeyA");
    }

    #[test]
    fn missing_or_foreign_scan_code_falls_back_to_keycode() {
        // `adb shell input keyevent` / IME-synthesized events carry no scan code.
        assert_eq!(physical_code(KEYCODE_W, 0), "KeyW");
        assert_eq!(physical_code(KEYCODE_DPAD_UP, 0), "ArrowUp");
        // A gamepad button's evdev code isn't a keyboard key.
        assert_eq!(physical_code(KEYCODE_SPACE, 0x130), "Space");
        assert_eq!(physical_code(0, 0), "");
    }

    #[test]
    fn key_name_named_keys_and_characters() {
        assert_eq!(key_name(KEYCODE_SHIFT_LEFT, 0, META_SHIFT_ON), "Shift");
        assert_eq!(key_name(113, 0, META_CTRL_ON), "Control");
        assert_eq!(key_name(57, 0, META_ALT_ON), "Alt");
        assert_eq!(key_name(117, 0, META_META_ON), "Meta");
        assert_eq!(key_name(131, 0, 0), "F1");
        assert_eq!(key_name(142, 0, 0), "F12");
        assert_eq!(key_name(115, 0, 0), "CapsLock");
        assert_eq!(key_name(124, 0, 0), "Insert");
        assert_eq!(key_name(KEYCODE_DPAD_UP, 0, 0), "ArrowUp");
        assert_eq!(key_name(KEYCODE_SPACE, ' ' as i32, 0), " ");
        assert_eq!(key_name(KEYCODE_A, 'a' as i32, 0), "a");
        assert_eq!(key_name(KEYCODE_A, 'A' as i32, META_SHIFT_ON), "A");
        // AZERTY: KEYCODE_A on the physical Q key still types the layout's char.
        assert_eq!(key_name(KEYCODE_A, 'a' as i32, 0), "a");
        assert_eq!(key_name(0, 0, 0), "");
    }

    /// Android's key character map yields no character for Ctrl chords, so
    /// `Ctrl+S` used to arrive with `key: ""` — unbindable as a shortcut.
    /// The web reports `key: "s"` with `ctrlKey`.
    #[test]
    fn regression_ctrl_chord_letter_has_a_key() {
        let s = 47; // KEYCODE_S
        assert_eq!(key_name(s, 0, META_CTRL_ON), "s");
        assert_eq!(key_name(s, 0, META_CTRL_ON | META_SHIFT_ON), "S");
        assert_eq!(key_name(KEYCODE_0 + 1, 0, META_CTRL_ON), "1");
        assert_eq!(key_name(KEYCODE_SPACE, 0, META_CTRL_ON), " ");
    }

    #[test]
    fn modifiers_decode_meta_state_bits() {
        assert_eq!(Modifiers::from_meta_state(0), Modifiers::default());
        let all = Modifiers::from_meta_state(META_SHIFT_ON | META_CTRL_ON | META_ALT_ON | META_META_ON);
        assert!(all.shift && all.ctrl && all.alt && all.meta);
        // META_SHIFT_LEFT_ON (0x40) alone is always accompanied by
        // META_SHIFT_ON on a real event; the *_ON mask is what we read.
        assert!(Modifiers::from_meta_state(0x40 | META_SHIFT_ON).shift);
    }

    /// The app-level path used to fire on ACTION_DOWN only — a game never
    /// saw a release, so held-key movement never stopped. Both phases now
    /// map, the release keeps the same physical `code` even when its `key`
    /// changed (Shift let go first), and a release is never a repeat.
    #[test]
    fn regression_key_up_is_delivered_with_matching_code() {
        let down = app_key_event(false, false, KEYCODE_W, KEY_W, META_SHIFT_ON, 'W' as i32);
        assert_eq!(down.phase, KeyPhase::Down);
        assert_eq!(down.key, "W");
        assert_eq!(down.code, "KeyW");
        assert!(down.shift && !down.repeat);

        let held = app_key_event(false, true, KEYCODE_W, KEY_W, 0, 'w' as i32);
        assert!(held.repeat, "Android repeatCount > 0 passes through");

        let up = app_key_event(true, true, KEYCODE_W, KEY_W, 0, 'w' as i32);
        assert_eq!(up.phase, KeyPhase::Up);
        assert_eq!(up.key, "w");
        assert_eq!(up.code, down.code, "release tracks the same physical key");
        assert!(!up.repeat, "a release is never a repeat");
        assert!(!up.shift);
    }

    #[test]
    fn modifier_keys_produce_down_and_up() {
        let down = app_key_event(false, false, KEYCODE_SHIFT_LEFT, 42, META_SHIFT_ON | 0x40, 0);
        assert_eq!((down.key.as_str(), down.code.as_str()), ("Shift", "ShiftLeft"));
        assert_eq!(down.phase, KeyPhase::Down);
        let up = app_key_event(true, false, KEYCODE_SHIFT_LEFT, 42, 0, 0);
        assert_eq!((up.key.as_str(), up.code.as_str()), ("Shift", "ShiftLeft"));
        assert_eq!(up.phase, KeyPhase::Up);
    }
}
