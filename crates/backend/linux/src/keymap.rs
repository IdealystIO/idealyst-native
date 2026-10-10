//! Pure GDK → Web key translation for the app-level keyboard sink.
//!
//! Deliberately free of any GTK type: the crate body is
//! `cfg(target_os = "linux")`, so tables inside it could only be tested in
//! the Linux container. This file is ALSO compiled by
//! `tests/keymap_tables.rs` via `#[path]`, which runs on any host — the
//! reachable test for the platform translation (see CLAUDE.md §8).
//!
//! ## Physical code (`AppKeyEvent::code`)
//!
//! GDK hands the key controller a *hardware keycode*. On both X11 and
//! Wayland (the two GDK backends a GTK4 app runs on) that is the Linux
//! evdev code + 8 — X11's keycode offset, which the Wayland backend keeps
//! for compatibility (`wl_keyboard` sends evdev codes; GDK adds 8). The
//! evdev code names the physical key independent of layout, which is
//! exactly Web `KeyboardEvent.code`. Translating from the keyval instead
//! would make WASD follow the layout (AZERTY `Z` would report `KeyW`'s
//! meaning) and would change mid-hold when Shift changes the keyval.
//!
//! ## Meaning (`AppKeyEvent::key`)
//!
//! [`named_key`] covers the non-printing keysyms by their X11 keysym value
//! (stable since X11R6; `gdk::Key` is a thin wrapper over the same `u32`).
//! Printing keys go through `gdk_keyval_to_unicode` at the call site.

/// Web `KeyboardEvent.code` for a GDK hardware keycode (evdev + 8).
/// Empty when unknown.
pub fn hardware_keycode_to_code(keycode: u32) -> &'static str {
    match keycode.checked_sub(8) {
        Some(evdev) => evdev_to_code(evdev),
        None => "",
    }
}

/// Web `KeyboardEvent.code` for a Linux evdev `KEY_*` code
/// (`linux/input-event-codes.h`). Empty when unknown.
pub fn evdev_to_code(evdev: u32) -> &'static str {
    match evdev {
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
        59 => "F1",
        60 => "F2",
        61 => "F3",
        62 => "F4",
        63 => "F5",
        64 => "F6",
        65 => "F7",
        66 => "F8",
        67 => "F9",
        68 => "F10",
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
        86 => "IntlBackslash",
        87 => "F11",
        88 => "F12",
        89 => "IntlRo",
        96 => "NumpadEnter",
        97 => "ControlRight",
        98 => "NumpadDivide",
        99 => "PrintScreen",
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
        113 => "AudioVolumeMute",
        114 => "AudioVolumeDown",
        115 => "AudioVolumeUp",
        117 => "NumpadEqual",
        119 => "Pause",
        121 => "NumpadComma",
        124 => "IntlYen",
        125 => "MetaLeft",
        126 => "MetaRight",
        127 => "ContextMenu",
        183 => "F13",
        184 => "F14",
        185 => "F15",
        186 => "F16",
        187 => "F17",
        188 => "F18",
        189 => "F19",
        190 => "F20",
        191 => "F21",
        192 => "F22",
        193 => "F23",
        194 => "F24",
        _ => "",
    }
}

/// Web `KeyboardEvent.key` for a non-printing X11/GDK keysym. `None` for
/// keysyms that carry a character (the caller asks
/// `gdk_keyval_to_unicode`) and for keysyms with no Web name.
pub fn named_key(keysym: u32) -> Option<&'static str> {
    Some(match keysym {
        0xff0d | 0xff8d => "Enter",                // Return, KP_Enter
        0xff1b => "Escape",
        0xff09 | 0xfe20 => "Tab",                  // Tab, ISO_Left_Tab (Shift+Tab)
        0xff08 => "Backspace",
        0xffff | 0xff9f => "Delete",               // Delete, KP_Delete
        0xff63 | 0xff9e => "Insert",               // Insert, KP_Insert
        0xff52 | 0xff97 => "ArrowUp",
        0xff54 | 0xff99 => "ArrowDown",
        0xff51 | 0xff96 => "ArrowLeft",
        0xff53 | 0xff98 => "ArrowRight",
        0xff50 | 0xff95 => "Home",
        0xff57 | 0xff9c => "End",
        0xff55 | 0xff9a => "PageUp",
        0xff56 | 0xff9b => "PageDown",
        0xff9d => "Clear",                          // KP_Begin (numpad 5, NumLock off)
        0x0020 | 0xff80 => " ",                    // space, KP_Space
        0xffe1 | 0xffe2 => "Shift",
        0xffe3 | 0xffe4 => "Control",
        0xffe9 | 0xffea => "Alt",
        0xfe03 => "AltGraph",                       // ISO_Level3_Shift
        0xffe7 | 0xffe8 | 0xffeb | 0xffec => "Meta", // Meta_L/R, Super_L/R
        0xffe5 => "CapsLock",
        0xff7f => "NumLock",
        0xff14 => "ScrollLock",
        0xff13 => "Pause",
        0xff61 => "PrintScreen",                    // Print
        0xff67 => "ContextMenu",                    // Menu
        // F1 (0xffbe) … F24 (0xffd5) are contiguous.
        k @ 0xffbe..=0xffd5 => return Some(F_KEYS[(k - 0xffbe) as usize]),
        _ => return None,
    })
}

const F_KEYS: [&str; 24] = [
    "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "F13", "F14",
    "F15", "F16", "F17", "F18", "F19", "F20", "F21", "F22", "F23", "F24",
];

/// Modifier flags as Web reports them for a modifier key's OWN event.
///
/// GDK (like X11) reports the modifier state *before* the event, so a
/// Shift press arrives with `shift = false` and its release with
/// `shift = true` — the reverse of Web (`keydown` of Shift has
/// `shiftKey: true`, its `keyup` has `shiftKey: false`). Folding the key's
/// own effect in here makes the flags converge with every other backend.
/// Non-modifier codes pass through unchanged.
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
