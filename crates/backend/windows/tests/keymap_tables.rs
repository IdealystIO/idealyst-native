//! The pure Win32 → Web key tables behind the app-level keyboard sink.
//!
//! ## The bug this pins
//!
//! The Win32 backend had NO app-level keyboard: it inherited the trait's
//! no-op key hook, so a game's key listeners heard nothing on Windows. The
//! new source must name the PHYSICAL key (`code`) from the Set-1 scancode
//! + extended flag in `lParam` — not from the virtual key, which follows
//! the layout (AZERTY's `A` position sends `VK_Q`) — and must read the
//! auto-repeat bit.
//!
//! The crate body is `cfg(target_os = "windows")`; this test compiles the
//! table module directly (`#[path]`) so it runs on every host. The live
//! slot/translation path is covered by `app_keys::tests` (Windows target).

#[path = "../src/keymap.rs"]
mod keymap;

use keymap::*;

#[test]
fn regression_win32_app_keys_name_the_physical_key_from_the_scancode() {
    let cases = [
        ((0x11, false), "KeyW"),
        ((0x1E, false), "KeyA"),
        ((0x1F, false), "KeyS"),
        ((0x20, false), "KeyD"),
        ((0x39, false), "Space"),
        ((0x2A, false), "ShiftLeft"),
        ((0x36, false), "ShiftRight"),
        ((0x1D, false), "ControlLeft"),
        ((0x1D, true), "ControlRight"),
        ((0x38, false), "AltLeft"),
        ((0x38, true), "AltRight"),
        ((0x5B, true), "MetaLeft"),
        ((0x5C, true), "MetaRight"),
        // The arrow cluster is extended; the same scancodes without E0 are
        // the numpad keys (NumLock off sends those as arrows by VK, but the
        // physical key is still the numpad one).
        ((0x48, true), "ArrowUp"),
        ((0x50, true), "ArrowDown"),
        ((0x4B, true), "ArrowLeft"),
        ((0x4D, true), "ArrowRight"),
        ((0x48, false), "Numpad8"),
        ((0x4B, false), "Numpad4"),
        ((0x1C, false), "Enter"),
        ((0x1C, true), "NumpadEnter"),
        ((0x35, false), "Slash"),
        ((0x35, true), "NumpadDivide"),
        ((0x47, true), "Home"),
        ((0x4F, true), "End"),
        ((0x49, true), "PageUp"),
        ((0x51, true), "PageDown"),
        ((0x52, true), "Insert"),
        ((0x53, true), "Delete"),
        ((0x45, true), "NumLock"),
        ((0x45, false), "Pause"),
        ((0x37, true), "PrintScreen"),
        ((0x37, false), "NumpadMultiply"),
        ((0x01, false), "Escape"),
        ((0x0E, false), "Backspace"),
        ((0x0F, false), "Tab"),
        ((0x02, false), "Digit1"),
        ((0x0B, false), "Digit0"),
        ((0x3B, false), "F1"),
        ((0x44, false), "F10"),
        ((0x57, false), "F11"),
        ((0x58, false), "F12"),
        ((0x3A, false), "CapsLock"),
        ((0x29, false), "Backquote"),
        ((0x28, false), "Quote"),
    ];
    for ((scan, ext), code) in cases {
        assert_eq!(scancode_to_code(scan, ext), code, "scancode {scan:#x} ext={ext}");
    }
    assert_eq!(scancode_to_code(0x00, false), "");
    assert_eq!(scancode_to_code(0x11, true), "", "no extended W");
}

#[test]
fn every_letter_and_digit_has_a_scancode() {
    let letters = (0..=0xFFu32).map(|s| scancode_to_code(s, false)).filter(|c| c.starts_with("Key")).count();
    let digits = (0..=0xFFu32).map(|s| scancode_to_code(s, false)).filter(|c| c.starts_with("Digit")).count();
    assert_eq!((letters, digits), (26, 10));
}

#[test]
fn regression_win32_lparam_decodes_scancode_extended_and_repeat() {
    // Up arrow, first press: scancode 0x48, extended, previous state up.
    let lp = (1isize) | (0x48 << 16) | (1 << 24);
    assert_eq!(decode_lparam(lp), KeyLParam { scancode: 0x48, extended: true, was_down: false });
    // Auto-repeat W: bit 30 set.
    let lp = (1isize) | (0x11 << 16) | (1 << 30);
    assert_eq!(decode_lparam(lp), KeyLParam { scancode: 0x11, extended: false, was_down: true });
    // Key-up messages set bits 30 and 31; bit 31 makes the 32-bit value
    // negative when sign-extended — decoding must not care.
    let lp = (0xC011_0001u32 as i32) as isize;
    assert_eq!(decode_lparam(lp), KeyLParam { scancode: 0x11, extended: false, was_down: true });
}

#[test]
fn mapvirtualkey_ex_results_split_into_scancode_and_extended() {
    assert_eq!(split_vsc_ex(0x0011), (0x11, false));
    assert_eq!(split_vsc_ex(0xE048), (0x48, true));
    assert_eq!(split_vsc_ex(0xE11D), (0x1D, true));
}

#[test]
fn key_messages_are_classified() {
    assert_eq!(key_message_is_down(WM_KEYDOWN), Some(true));
    assert_eq!(key_message_is_down(WM_SYSKEYDOWN), Some(true));
    assert_eq!(key_message_is_down(WM_KEYUP), Some(false));
    assert_eq!(key_message_is_down(WM_SYSKEYUP), Some(false));
    assert_eq!(key_message_is_down(0x0102), None); // WM_CHAR
}

#[test]
fn named_virtual_keys_use_web_key_names() {
    assert_eq!(vk_named_key(0x20), Some(" "));
    assert_eq!(vk_named_key(0x26), Some("ArrowUp"));
    assert_eq!(vk_named_key(0x0D), Some("Enter"));
    assert_eq!(vk_named_key(0x10), Some("Shift"));
    assert_eq!(vk_named_key(0xA1), Some("Shift"));
    assert_eq!(vk_named_key(0x11), Some("Control"));
    assert_eq!(vk_named_key(0x12), Some("Alt"));
    assert_eq!(vk_named_key(0x5B), Some("Meta"));
    assert_eq!(vk_named_key(0x14), Some("CapsLock"));
    assert_eq!(vk_named_key(0x70), Some("F1"));
    assert_eq!(vk_named_key(0x7B), Some("F12"));
    assert_eq!(vk_named_key(0x87), Some("F24"));
    assert_eq!(vk_named_key(0x21), Some("PageUp"));
    assert_eq!(vk_named_key(0x2D), Some("Insert"));
    // Letters / digits / OEM punctuation translate through ToUnicode.
    assert_eq!(vk_named_key(0x57), None);
    assert_eq!(vk_named_key(0x31), None);
    assert_eq!(vk_named_key(0xBA), None);
}

#[test]
fn to_unicode_results_are_classified() {
    assert_eq!(classify_to_unicode(1, &[b'w' as u16, 0]), ToUnicodeResult::Text("w".into()));
    assert_eq!(classify_to_unicode(-1, &[0x00B4, 0]), ToUnicodeResult::Dead);
    assert_eq!(classify_to_unicode(0, &[0, 0]), ToUnicodeResult::Nothing);
    // Ctrl+A → 0x01: retried with Ctrl cleared by the caller.
    assert_eq!(classify_to_unicode(1, &[0x01, 0]), ToUnicodeResult::Control);
    // A ret larger than the buffer never slices out of bounds.
    assert_eq!(classify_to_unicode(5, &[b'a' as u16]), ToUnicodeResult::Text("a".into()));
}

#[test]
fn regression_win32_modifier_key_reports_its_own_flag_like_web() {
    assert_eq!(own_modifier_flags(true, "ShiftLeft", false, false, false, false), (true, false, false, false));
    assert_eq!(own_modifier_flags(false, "ShiftRight", true, false, false, false), (false, false, false, false));
    assert_eq!(own_modifier_flags(true, "AltRight", false, false, false, false), (false, false, true, false));
    assert_eq!(own_modifier_flags(true, "KeyW", true, true, false, false), (true, true, false, false));
}
