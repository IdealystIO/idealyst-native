//! Pure tables behind the robot's Win32 key injection (`native_key.rs`):
//! the virtual key a robot key press is, whether it is an `E0`-extended
//! key, and the key message's `lParam`.
//!
//! Free of any `windows`-crate type so `tests/native_key_tables.rs` can
//! compile it with `#[path]` and run it on every host (the crate body is
//! `cfg(target_os = "windows")`).

/// The virtual key for a robot press: named keys by their Web `key`;
/// characters by their US-layout Web `code` (the live path asks the
/// current layout with `VkKeyScanW` first, and falls back to this).
/// `None` for a key no virtual key names.
pub fn vk_for(key: &str, code: &str) -> Option<u16> {
    let named = match key {
        "Backspace" => 0x08,
        "Tab" => 0x09,
        "Enter" => 0x0D,
        "Shift" => 0x10,
        "Control" => 0x11,
        "Alt" => 0x12,
        "CapsLock" => 0x14,
        "Escape" => 0x1B,
        " " => 0x20,
        "PageUp" => 0x21,
        "PageDown" => 0x22,
        "End" => 0x23,
        "Home" => 0x24,
        "ArrowLeft" => 0x25,
        "ArrowUp" => 0x26,
        "ArrowRight" => 0x27,
        "ArrowDown" => 0x28,
        "Insert" => 0x2D,
        "Delete" => 0x2E,
        "Meta" => 0x5B,
        "ContextMenu" => 0x5D,
        _ => 0,
    };
    if named != 0 {
        return Some(named);
    }
    if let Some(n) = key.strip_prefix('F').and_then(|n| n.parse::<u16>().ok()) {
        if (1..=24).contains(&n) {
            // VK_F1 (0x70) … VK_F24 (0x87) are contiguous.
            return Some(0x70 + n - 1);
        }
    }
    if let Some(l) = code.strip_prefix("Key").filter(|l| l.len() == 1) {
        return Some(l.as_bytes()[0].to_ascii_uppercase() as u16);
    }
    if let Some(d) = code.strip_prefix("Digit").filter(|d| d.len() == 1) {
        return Some(d.as_bytes()[0] as u16);
    }
    Some(match code {
        "Semicolon" => 0xBA,
        "Equal" => 0xBB,
        "Comma" => 0xBC,
        "Minus" => 0xBD,
        "Period" => 0xBE,
        "Slash" => 0xBF,
        "Backquote" => 0xC0,
        "BracketLeft" => 0xDB,
        "Backslash" => 0xDC,
        "BracketRight" => 0xDD,
        "Quote" => 0xDE,
        _ => return None,
    })
}

/// Whether `vk` is an `E0`-extended key (lParam bit 24): the navigation
/// cluster, the arrows, the Windows and Applications keys — what a real
/// keyboard's message carries, and what the backend's `code` table reads.
pub fn is_extended(vk: u16) -> bool {
    matches!(vk, 0x21..=0x28 | 0x2D | 0x2E | 0x5B | 0x5C | 0x5D)
}

/// A key message's `lParam`: repeat count 1, the Set-1 scancode (bits
/// 16–23), the extended flag (24), the context code (29, Alt held — the
/// `WM_SYS*` messages), and on a release the previous-state and transition
/// bits (30, 31).
pub fn key_lparam(scancode: u32, extended: bool, down: bool, alt: bool) -> isize {
    let mut l: u32 = 1 | ((scancode & 0xFF) << 16);
    if extended {
        l |= 1 << 24;
    }
    if alt {
        l |= 1 << 29;
    }
    if !down {
        l |= (1 << 30) | (1 << 31);
    }
    // Sign-extend as the 32-bit LPARAM a real message carries.
    l as i32 as isize
}

/// `WM_KEYDOWN` / `WM_KEYUP`, or the `WM_SYS*` pair while Alt is held, as
/// Windows sends them.
pub fn key_message(down: bool, alt: bool) -> u32 {
    match (down, alt) {
        (true, false) => 0x0100,
        (false, false) => 0x0101,
        (true, true) => 0x0104,
        (false, true) => 0x0105,
    }
}

/// The virtual keys a press's modifiers hold (`VK_SHIFT`, `VK_CONTROL`,
/// `VK_MENU`, `VK_LWIN`), each with its left-hand variant, as a held
/// physical key sets both in the keyboard state.
pub fn modifier_vks(shift: bool, ctrl: bool, alt: bool, meta: bool) -> Vec<u16> {
    let mut vks = Vec::new();
    if shift {
        vks.extend([0x10, 0xA0]);
    }
    if ctrl {
        vks.extend([0x11, 0xA2]);
    }
    if alt {
        vks.extend([0x12, 0xA4]);
    }
    if meta {
        vks.push(0x5B);
    }
    vks
}
