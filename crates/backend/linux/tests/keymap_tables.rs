//! The pure GDK → Web key tables behind the app-level keyboard sink.
//!
//! ## The bug this pins
//!
//! GTK had NO app-level keyboard at all: `set_app_key_handler` was never
//! overridden, so a game's key listeners heard nothing on Linux. The new
//! source must name the PHYSICAL key (`code`) from the hardware keycode —
//! evdev + 8 on both X11 and Wayland — not from the keyval, or WASD
//! follows the layout and a Shift-held key changes identity mid-hold.
//!
//! The crate body is `cfg(target_os = "linux")`; this test compiles the
//! table module directly (`#[path]`) so it runs on every host, including
//! the macOS dev machine. The live GTK wiring is covered by
//! `tests/app_keyboard.rs` (Linux container).

#[path = "../src/keymap.rs"]
mod keymap;

use keymap::{evdev_to_code, hardware_keycode_to_code, named_key, own_modifier_flags};

#[test]
fn regression_gtk_app_keys_name_the_physical_key_from_the_hardware_keycode() {
    // GDK hardware keycode = evdev + 8.
    let cases = [
        (17 + 8, "KeyW"),
        (30 + 8, "KeyA"),
        (31 + 8, "KeyS"),
        (32 + 8, "KeyD"),
        (57 + 8, "Space"),
        (42 + 8, "ShiftLeft"),
        (54 + 8, "ShiftRight"),
        (29 + 8, "ControlLeft"),
        (97 + 8, "ControlRight"),
        (56 + 8, "AltLeft"),
        (100 + 8, "AltRight"),
        (125 + 8, "MetaLeft"),
        (126 + 8, "MetaRight"),
        (103 + 8, "ArrowUp"),
        (108 + 8, "ArrowDown"),
        (105 + 8, "ArrowLeft"),
        (106 + 8, "ArrowRight"),
        (1 + 8, "Escape"),
        (28 + 8, "Enter"),
        (96 + 8, "NumpadEnter"),
        (15 + 8, "Tab"),
        (14 + 8, "Backspace"),
        (111 + 8, "Delete"),
        (2 + 8, "Digit1"),
        (11 + 8, "Digit0"),
        (59 + 8, "F1"),
        (68 + 8, "F10"),
        (87 + 8, "F11"),
        (88 + 8, "F12"),
        (82 + 8, "Numpad0"),
        (73 + 8, "Numpad9"),
        (78 + 8, "NumpadAdd"),
        (58 + 8, "CapsLock"),
        (41 + 8, "Backquote"),
        (40 + 8, "Quote"),
    ];
    for (keycode, code) in cases {
        assert_eq!(hardware_keycode_to_code(keycode), code, "keycode {keycode}");
    }
}

#[test]
fn unknown_or_underflowing_keycodes_report_an_empty_code() {
    assert_eq!(hardware_keycode_to_code(0), "");
    assert_eq!(hardware_keycode_to_code(7), "");
    assert_eq!(evdev_to_code(0), "");
    assert_eq!(evdev_to_code(10_000), "");
}

#[test]
fn every_letter_and_digit_has_a_code() {
    let letters: Vec<&str> = (0..=255u32).map(evdev_to_code).filter(|c| c.starts_with("Key")).collect();
    assert_eq!(letters.len(), 26, "{letters:?}");
    let digits: Vec<&str> = (0..=255u32).map(evdev_to_code).filter(|c| c.starts_with("Digit")).collect();
    assert_eq!(digits.len(), 10, "{digits:?}");
}

#[test]
fn named_keysyms_use_web_key_names() {
    assert_eq!(named_key(0x0020), Some(" "));
    assert_eq!(named_key(0xff52), Some("ArrowUp"));
    assert_eq!(named_key(0xff0d), Some("Enter"));
    assert_eq!(named_key(0xff8d), Some("Enter"));
    assert_eq!(named_key(0xfe20), Some("Tab"));
    assert_eq!(named_key(0xffe1), Some("Shift"));
    assert_eq!(named_key(0xffe2), Some("Shift"));
    assert_eq!(named_key(0xffe3), Some("Control"));
    assert_eq!(named_key(0xffe9), Some("Alt"));
    assert_eq!(named_key(0xffeb), Some("Meta"));
    assert_eq!(named_key(0xffe5), Some("CapsLock"));
    assert_eq!(named_key(0xffbe), Some("F1"));
    assert_eq!(named_key(0xffc9), Some("F12"));
    assert_eq!(named_key(0xffd5), Some("F24"));
    assert_eq!(named_key(0xff63), Some("Insert"));
    assert_eq!(named_key(0xff55), Some("PageUp"));
    // Printing keysyms are left to gdk_keyval_to_unicode.
    assert_eq!(named_key(0x0077), None); // 'w'
    assert_eq!(named_key(0x0041), None); // 'A'
}

#[test]
fn regression_gtk_modifier_key_reports_its_own_flag_like_web() {
    // GDK reports the PRE-event state: Shift down arrives with shift=false.
    assert_eq!(own_modifier_flags(true, "ShiftLeft", false, false, false, false), (true, false, false, false));
    // …and its release with shift=true.
    assert_eq!(own_modifier_flags(false, "ShiftRight", true, false, false, false), (false, false, false, false));
    assert_eq!(own_modifier_flags(true, "ControlRight", false, false, false, false), (false, true, false, false));
    assert_eq!(own_modifier_flags(true, "AltLeft", false, false, false, false), (false, false, true, false));
    assert_eq!(own_modifier_flags(true, "MetaLeft", false, false, false, false), (false, false, false, true));
    // A non-modifier key passes the state through.
    assert_eq!(own_modifier_flags(true, "KeyW", true, false, true, false), (true, false, true, false));
}
