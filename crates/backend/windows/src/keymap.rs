//! Pure Win32 → Web key translation for the app-level keyboard sink.
//!
//! Deliberately free of any `windows`-crate type: the crate body is
//! `cfg(target_os = "windows")`, so tables inside it could only be tested on
//! a Windows machine. This file is ALSO compiled by `tests/keymap_tables.rs`
//! via `#[path]`, which runs on any host — the reachable test for the
//! platform translation (CLAUDE.md §8).
//!
//! ## Physical code (`AppKeyEvent::code`)
//!
//! `WM_KEYDOWN`/`WM_KEYUP` carry the key's PS/2 Set-1 scancode in
//! `lParam` bits 16–23 and the "extended" (`E0` prefix) flag in bit 24.
//! The scancode names the physical key independent of the active layout —
//! exactly Web `KeyboardEvent.code` (Chromium derives `code` on Windows from
//! the same pair). The virtual key is NOT used for `code`: it follows the
//! layout (AZERTY's `A` position sends `VK_Q`), so WASD would move.
//!
//! ## Meaning (`AppKeyEvent::key`)
//!
//! Non-printing keys come from the virtual key ([`vk_named_key`]); printing
//! keys from `ToUnicode` at the call site, post-processed by
//! [`classify_to_unicode`].

/// Message ids (`winuser.h`) — the four messages the keyboard source reads.
pub const WM_KEYDOWN: u32 = 0x0100;
pub const WM_KEYUP: u32 = 0x0101;
pub const WM_SYSKEYDOWN: u32 = 0x0104;
pub const WM_SYSKEYUP: u32 = 0x0105;

/// `Some(true)` for a key-down message, `Some(false)` for a key-up, `None`
/// for anything else. The `SYS` variants are what Windows sends while Alt
/// is held (and for F10) — they are ordinary keys to the app.
pub fn key_message_is_down(msg: u32) -> Option<bool> {
    match msg {
        WM_KEYDOWN | WM_SYSKEYDOWN => Some(true),
        WM_KEYUP | WM_SYSKEYUP => Some(false),
        _ => None,
    }
}

/// The fields of a key message's `lParam`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct KeyLParam {
    /// Set-1 scancode (bits 16–23).
    pub scancode: u32,
    /// `E0`-prefixed key (bit 24): right Ctrl/Alt, the arrow cluster, …
    pub extended: bool,
    /// Previous key state (bit 30): on a key-down, `true` means the key was
    /// already down — an auto-repeat.
    pub was_down: bool,
}

pub fn decode_lparam(lparam: isize) -> KeyLParam {
    let l = lparam as usize;
    KeyLParam {
        scancode: ((l >> 16) & 0xFF) as u32,
        extended: (l >> 24) & 1 == 1,
        was_down: (l >> 30) & 1 == 1,
    }
}

/// Split the `MapVirtualKeyW(vk, MAPVK_VK_TO_VSC_EX)` result (`0xE0xx` for
/// an extended key) into the same `(scancode, extended)` pair `lParam`
/// carries. Used when a message arrives with scancode 0 (injected input).
pub fn split_vsc_ex(vsc: u32) -> (u32, bool) {
    (vsc & 0xFF, (vsc >> 8) & 0xFF == 0xE0 || (vsc >> 8) & 0xFF == 0xE1)
}

/// Web `KeyboardEvent.code` for a Set-1 scancode + extended flag. Empty
/// when unknown.
pub fn scancode_to_code(scancode: u32, extended: bool) -> &'static str {
    if extended {
        return match scancode {
            0x1C => "NumpadEnter",
            0x1D => "ControlRight",
            0x20 => "AudioVolumeMute",
            0x2E => "AudioVolumeDown",
            0x30 => "AudioVolumeUp",
            0x35 => "NumpadDivide",
            0x37 => "PrintScreen",
            0x38 => "AltRight",
            // NumLock is the EXTENDED 0x45; plain 0x45 is Pause (it arrives
            // with an E1 prefix that Windows folds into the non-extended
            // form). Chromium's `dom_code_data.inc` maps them the same way.
            0x45 => "NumLock",
            0x46 => "Pause", // Ctrl+Break
            0x47 => "Home",
            0x48 => "ArrowUp",
            0x49 => "PageUp",
            0x4B => "ArrowLeft",
            0x4D => "ArrowRight",
            0x4F => "End",
            0x50 => "ArrowDown",
            0x51 => "PageDown",
            0x52 => "Insert",
            0x53 => "Delete",
            0x5B => "MetaLeft",
            0x5C => "MetaRight",
            0x5D => "ContextMenu",
            _ => "",
        };
    }
    match scancode {
        0x01 => "Escape",
        0x02 => "Digit1",
        0x03 => "Digit2",
        0x04 => "Digit3",
        0x05 => "Digit4",
        0x06 => "Digit5",
        0x07 => "Digit6",
        0x08 => "Digit7",
        0x09 => "Digit8",
        0x0A => "Digit9",
        0x0B => "Digit0",
        0x0C => "Minus",
        0x0D => "Equal",
        0x0E => "Backspace",
        0x0F => "Tab",
        0x10 => "KeyQ",
        0x11 => "KeyW",
        0x12 => "KeyE",
        0x13 => "KeyR",
        0x14 => "KeyT",
        0x15 => "KeyY",
        0x16 => "KeyU",
        0x17 => "KeyI",
        0x18 => "KeyO",
        0x19 => "KeyP",
        0x1A => "BracketLeft",
        0x1B => "BracketRight",
        0x1C => "Enter",
        0x1D => "ControlLeft",
        0x1E => "KeyA",
        0x1F => "KeyS",
        0x20 => "KeyD",
        0x21 => "KeyF",
        0x22 => "KeyG",
        0x23 => "KeyH",
        0x24 => "KeyJ",
        0x25 => "KeyK",
        0x26 => "KeyL",
        0x27 => "Semicolon",
        0x28 => "Quote",
        0x29 => "Backquote",
        0x2A => "ShiftLeft",
        0x2B => "Backslash",
        0x2C => "KeyZ",
        0x2D => "KeyX",
        0x2E => "KeyC",
        0x2F => "KeyV",
        0x30 => "KeyB",
        0x31 => "KeyN",
        0x32 => "KeyM",
        0x33 => "Comma",
        0x34 => "Period",
        0x35 => "Slash",
        0x36 => "ShiftRight",
        0x37 => "NumpadMultiply",
        0x38 => "AltLeft",
        0x39 => "Space",
        0x3A => "CapsLock",
        0x3B => "F1",
        0x3C => "F2",
        0x3D => "F3",
        0x3E => "F4",
        0x3F => "F5",
        0x40 => "F6",
        0x41 => "F7",
        0x42 => "F8",
        0x43 => "F9",
        0x44 => "F10",
        0x45 => "Pause",
        0x46 => "ScrollLock",
        0x47 => "Numpad7",
        0x48 => "Numpad8",
        0x49 => "Numpad9",
        0x4A => "NumpadSubtract",
        0x4B => "Numpad4",
        0x4C => "Numpad5",
        0x4D => "Numpad6",
        0x4E => "NumpadAdd",
        0x4F => "Numpad1",
        0x50 => "Numpad2",
        0x51 => "Numpad3",
        0x52 => "Numpad0",
        0x53 => "NumpadDecimal",
        0x56 => "IntlBackslash",
        0x57 => "F11",
        0x58 => "F12",
        0x59 => "NumpadEqual",
        0x64 => "F13",
        0x65 => "F14",
        0x66 => "F15",
        0x67 => "F16",
        0x68 => "F17",
        0x69 => "F18",
        0x6A => "F19",
        0x6B => "F20",
        0x6C => "F21",
        0x6D => "F22",
        0x6E => "F23",
        0x73 => "IntlRo",
        0x76 => "F24",
        0x7D => "IntlYen",
        0x7E => "NumpadComma",
        _ => "",
    }
}

/// Web `KeyboardEvent.key` for a non-printing virtual key. `None` for keys
/// whose meaning is a character (the caller asks `ToUnicode`).
pub fn vk_named_key(vk: u32) -> Option<&'static str> {
    Some(match vk {
        0x08 => "Backspace",
        0x09 => "Tab",
        0x0C => "Clear",
        0x0D => "Enter",
        0x10 | 0xA0 | 0xA1 => "Shift",
        0x11 | 0xA2 | 0xA3 => "Control",
        0x12 | 0xA4 | 0xA5 => "Alt",
        0x13 => "Pause",
        0x14 => "CapsLock",
        0x1B => "Escape",
        0x20 => " ",
        0x21 => "PageUp",
        0x22 => "PageDown",
        0x23 => "End",
        0x24 => "Home",
        0x25 => "ArrowLeft",
        0x26 => "ArrowUp",
        0x27 => "ArrowRight",
        0x28 => "ArrowDown",
        0x2C => "PrintScreen",
        0x2D => "Insert",
        0x2E => "Delete",
        0x5B | 0x5C => "Meta",
        0x5D => "ContextMenu",
        // VK_F1 (0x70) … VK_F24 (0x87) are contiguous.
        v @ 0x70..=0x87 => return Some(F_KEYS[(v - 0x70) as usize]),
        0x90 => "NumLock",
        0x91 => "ScrollLock",
        0xAD => "AudioVolumeMute",
        0xAE => "AudioVolumeDown",
        0xAF => "AudioVolumeUp",
        _ => return None,
    })
}

const F_KEYS: [&str; 24] = [
    "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "F13", "F14",
    "F15", "F16", "F17", "F18", "F19", "F20", "F21", "F22", "F23", "F24",
];

/// What a `ToUnicode` call produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToUnicodeResult {
    /// Printable text — the Web `key`.
    Text(String),
    /// A dead key (`ToUnicode` returned < 0) — Web reports `"Dead"`.
    Dead,
    /// A C0 control character (Ctrl+letter yields `0x01`…`0x1A`). Web
    /// reports the letter, so the caller retries with Ctrl cleared.
    Control,
    /// No translation.
    Nothing,
}

/// Classify a `ToUnicode` return value and its output buffer.
pub fn classify_to_unicode(ret: i32, buf: &[u16]) -> ToUnicodeResult {
    if ret < 0 {
        return ToUnicodeResult::Dead;
    }
    let n = (ret as usize).min(buf.len());
    if n == 0 {
        return ToUnicodeResult::Nothing;
    }
    let text = String::from_utf16_lossy(&buf[..n]);
    if text.chars().all(|c| c.is_control()) {
        return ToUnicodeResult::Control;
    }
    ToUnicodeResult::Text(text)
}

/// Modifier flags as Web reports them for a modifier key's OWN event: a
/// Shift down carries `shift: true`, its up `shift: false`. Win32's
/// `GetKeyState` already agrees once the message is retrieved, but folding
/// the key's own effect in makes that independent of when the state is
/// sampled (and identical to the GTK backend's normalization).
pub fn own_modifier_flags(
    down: bool,
    code: &str,
    mut shift: bool,
    mut ctrl: bool,
    mut alt: bool,
    mut meta: bool,
) -> (bool, bool, bool, bool) {
    match code {
        "ShiftLeft" | "ShiftRight" => shift = down,
        "ControlLeft" | "ControlRight" => ctrl = down,
        "AltLeft" | "AltRight" => alt = down,
        "MetaLeft" | "MetaRight" => meta = down,
        _ => {}
    }
    (shift, ctrl, alt, meta)
}
