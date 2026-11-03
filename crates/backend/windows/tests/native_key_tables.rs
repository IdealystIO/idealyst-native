//! The pure tables behind the robot's Win32 key injection
//! (`src/native_key.rs`): which virtual key a robot press posts, its
//! extended flag, and the key message's `lParam` — read back here with the
//! backend's own decoder (`keymap::decode_lparam` / `scancode_to_code`),
//! so an injected key names the same Web `code` a real one does.
//!
//! The crate body is `cfg(target_os = "windows")`; both table modules are
//! compiled directly (`#[path]`) so this runs on every host.

#[path = "../src/native_key/table.rs"]
mod table;
#[path = "../src/keymap.rs"]
mod keymap;

#[test]
fn named_keys_map_to_their_virtual_keys() {
    let vk = |k: &str| table::vk_for(k, k).unwrap();
    assert_eq!(vk("Enter"), 0x0D);
    assert_eq!(vk("Escape"), 0x1B);
    assert_eq!(vk("Tab"), 0x09);
    assert_eq!(vk("Backspace"), 0x08);
    assert_eq!(vk("ArrowDown"), 0x28);
    assert_eq!(vk("F1"), 0x70);
    assert_eq!(vk("F24"), 0x87);
    assert_eq!(table::vk_for(" ", "Space"), Some(0x20));
    // Every named key round-trips through the backend's own key names.
    for k in ["Enter", "Escape", "Tab", "Backspace", "Delete", "Home", "End", "PageUp", "ArrowLeft", "F5", "Meta"] {
        assert_eq!(keymap::vk_named_key(vk(k) as u32), Some(k), "{k}");
    }
}

#[test]
fn characters_map_by_their_us_code() {
    assert_eq!(table::vk_for("a", "KeyA"), Some(b'A' as u16));
    assert_eq!(table::vk_for("K", "KeyK"), Some(b'K' as u16));
    assert_eq!(table::vk_for("!", "Digit1"), Some(b'1' as u16));
    assert_eq!(table::vk_for("/", "Slash"), Some(0xBF));
    assert_eq!(table::vk_for("é", ""), None);
}

#[test]
fn lparam_carries_what_the_backend_decodes() {
    // Enter: scancode 0x1C, not extended.
    let down = table::key_lparam(0x1C, false, true, false);
    let lp = keymap::decode_lparam(down);
    assert_eq!((lp.scancode, lp.extended, lp.was_down), (0x1C, false, false), "a fresh press, not a repeat");
    assert_eq!(down & 0xFFFF, 1, "repeat count 1");
    assert_eq!(keymap::scancode_to_code(lp.scancode, lp.extended), "Enter");
    // An arrow is extended; its release sets bits 30 and 31.
    assert!(table::is_extended(0x26));
    let up = table::key_lparam(0x48, true, false, false) as u32;
    assert_eq!(up >> 30, 0b11);
    let lp = keymap::decode_lparam(up as i32 as isize);
    assert_eq!(keymap::scancode_to_code(lp.scancode, lp.extended), "ArrowUp");
    // Alt held: context code (bit 29) and the WM_SYS* pair.
    assert_eq!((table::key_lparam(0x25, false, true, true) >> 29) & 1, 1);
    assert_eq!(table::key_message(true, true), keymap::WM_SYSKEYDOWN);
    assert_eq!(table::key_message(false, false), keymap::WM_KEYUP);
    assert!(!table::is_extended(0x0D));
}

#[test]
fn modifiers_set_the_generic_and_left_keys() {
    assert_eq!(table::modifier_vks(true, true, false, false), vec![0x10, 0xA0, 0x11, 0xA2]);
    assert_eq!(table::modifier_vks(false, false, true, true), vec![0x12, 0xA4, 0x5B]);
    assert!(table::modifier_vks(false, false, false, false).is_empty());
}
