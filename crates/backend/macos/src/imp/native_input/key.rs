//! Real AppKit key input for the robot (`IntrospectionOps::native_key`).
//!
//! A press becomes the event sequence a keyboard produces — a
//! `flagsChanged` per held modifier going down, `keyDown`, `keyUp`, the
//! modifiers coming back up — built with `+[NSEvent keyEventWithType:…]`
//! and POSTED to the application queue (`postEvent:atStart:NO`), like the
//! robot's mouse events (`super`). `-[NSApplication sendEvent:]` then runs
//! them through the local monitors (the app-level key source,
//! `imp/keyboard.rs`) and on to the key window's first responder: a
//! focused text field's field editor interprets them (`insertText:`,
//! `deleteBackward:`, `insertNewline:`), exactly as it does a user's typing.
//!
//! The key goes to the KEY window, as a user's does, so the injection makes
//! the node's window key first: a test drives one app window, and a key
//! aimed at a window that isn't key would go nowhere (or to another app).
//!
//! `keyCode` is the `kVK_*` code whose physical key is the press's Web
//! `code` — the inverse of the key source's own table
//! ([`physical_code`](crate::imp::keyboard::physical_code)), so the
//! app-level listener reads back the same `code` the robot sent.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{class, msg_send, msg_send_id};
use objc2_foundation::{CGPoint, NSString};
use runtime_vocabulary::caps::{KeyPress, NativeInjection};

use crate::imp::keyboard::physical_code;
use crate::imp::MacosNode;

const KEY_DOWN: usize = 10;
const KEY_UP: usize = 11;
const FLAGS_CHANGED: usize = 12;

// Device-independent `NSEventModifierFlags`.
const FLAG_SHIFT: usize = 1 << 17;
const FLAG_CONTROL: usize = 1 << 18;
const FLAG_OPTION: usize = 1 << 19;
const FLAG_COMMAND: usize = 1 << 20;
// Left-side device bits (IOKit `NX_DEVICEL*KEYMASK`): the key source tells a
// modifier's down from its up by them (`keyboard::flags_changed_phase`).
const DEV_LCTL: usize = 0x0001;
const DEV_LSHIFT: usize = 0x0002;
const DEV_LCMD: usize = 0x0008;
const DEV_LALT: usize = 0x0020;

/// `(kVK code, independent flag, left device bit)` per modifier, in the
/// order a user presses them.
const MODIFIERS: [(u16, usize, usize); 4] = [
    (0x3B, FLAG_CONTROL, DEV_LCTL),
    (0x3A, FLAG_OPTION, DEV_LALT),
    (0x38, FLAG_SHIFT, DEV_LSHIFT),
    (0x37, FLAG_COMMAND, DEV_LCMD),
];

/// The `kVK_*` virtual key code for a Web `KeyboardEvent.code`. A code the
/// key source doesn't know (a character no US key types, `code == ""`)
/// gets an unassigned key code, which the source reports as `code: ""`.
pub(crate) fn virtual_key_code(code: &str) -> u16 {
    if !code.is_empty() {
        if let Some(kc) = (0u16..0x80).find(|&kc| physical_code(kc) == code) {
            return kc;
        }
    }
    (0u16..0x80).find(|&kc| physical_code(kc).is_empty()).unwrap_or(0xFF)
}

/// `NSBackTabCharacter`: what AppKit reports for Shift+Tab.
const BACK_TAB: &str = "\u{19}";

/// What `-[NSEvent characters]` holds for a key: the control character a
/// named key types, the private-use function-key code point AppKit uses
/// for keys that type nothing (`NSUpArrowFunctionKey` = U+F700, …), or the
/// character itself. `None` for a modifier key (a `flagsChanged`, which
/// has no characters).
pub(crate) fn characters(key: &str) -> Option<String> {
    let fk = |offset: u32| char::from_u32(0xF700 + offset).map(String::from);
    Some(match key {
        "Enter" => "\r".into(),
        "Tab" => "\t".into(),
        "Backspace" => "\u{7f}".into(),
        "Escape" => "\u{1b}".into(),
        "ArrowUp" => return fk(0x00),
        "ArrowDown" => return fk(0x01),
        "ArrowLeft" => return fk(0x02),
        "ArrowRight" => return fk(0x03),
        "Insert" => return fk(0x27),
        "Delete" => return fk(0x28),
        "Home" => return fk(0x29),
        "End" => return fk(0x2B),
        "PageUp" => return fk(0x2C),
        "PageDown" => return fk(0x2D),
        "ContextMenu" => return fk(0x35),
        "Shift" | "Control" | "Alt" | "Meta" | "CapsLock" => return None,
        f if f.len() > 1 && f.starts_with('F') => {
            let n: u32 = f[1..].parse().ok()?;
            return fk(0x04 + n - 1);
        }
        other => other.into(),
    })
}

/// `(event type, key code, modifier flags, characters)` for each event of
/// `press`, in delivery order. Pure, so the sequence is unit-tested.
pub(crate) fn event_sequence(press: &KeyPress) -> Vec<(usize, u16, usize, String)> {
    let held: Vec<(u16, usize, usize)> = MODIFIERS
        .iter()
        .copied()
        .filter(|(_, flag, _)| match *flag {
            FLAG_CONTROL => press.ctrl,
            FLAG_OPTION => press.alt,
            FLAG_SHIFT => press.shift,
            _ => press.meta,
        })
        .collect();
    let mut flags = 0usize;
    let mut out = Vec::new();
    for &(kc, flag, dev) in &held {
        flags |= flag | dev;
        out.push((FLAGS_CHANGED, kc, flags, String::new()));
    }
    let key_code = virtual_key_code(&press.code);
    // Shift+Tab types the back-tab character, which is what the field
    // editor maps to `insertBacktab:` (focus back); a "\t" with the Shift
    // flag is `insertTab:` and moves focus forward.
    let chars = if press.key == "Tab" && press.shift { Some(BACK_TAB.into()) } else { characters(&press.key) };
    match chars {
        Some(chars) => {
            out.push((KEY_DOWN, key_code, flags, chars.clone()));
            out.push((KEY_UP, key_code, flags, chars));
        }
        // A modifier key on its own: its flag goes down and up.
        None => {
            let (flag, dev) = MODIFIERS
                .iter()
                .find(|(kc, _, _)| *kc == key_code)
                .map(|(_, f, d)| (*f, *d))
                .unwrap_or((0, 0));
            out.push((FLAGS_CHANGED, key_code, flags | flag | dev, String::new()));
            out.push((FLAGS_CHANGED, key_code, flags, String::new()));
        }
    }
    for &(kc, flag, dev) in held.iter().rev() {
        flags &= !(flag | dev);
        out.push((FLAGS_CHANGED, kc, flags, String::new()));
    }
    out
}

pub(crate) fn native_key(node: &MacosNode, press: &KeyPress) -> Option<NativeInjection> {
    let window: Option<Retained<AnyObject>> = unsafe { msg_send_id![node.as_view(), window] };
    let window = window?;
    let events = event_sequence(press);
    Some(Box::new(move || {
        let is_key: bool = unsafe { msg_send![&*window, isKeyWindow] };
        if !is_key {
            let _: () = unsafe { msg_send![&*window, makeKeyWindow] };
        }
        let number: isize = unsafe { msg_send![&*window, windowNumber] };
        let info: *mut AnyObject = unsafe { msg_send![class!(NSProcessInfo), processInfo] };
        let app: *mut AnyObject = unsafe { msg_send![class!(NSApplication), sharedApplication] };
        let ctx: *mut AnyObject = std::ptr::null_mut();
        for (kind, key_code, flags, chars) in events {
            let now: f64 = unsafe { msg_send![info, systemUptime] };
            let s = NSString::from_str(&chars);
            let event: Option<Retained<AnyObject>> = unsafe {
                msg_send_id![
                    class!(NSEvent),
                    keyEventWithType: kind,
                    location: CGPoint { x: 0.0, y: 0.0 },
                    modifierFlags: flags,
                    timestamp: now,
                    windowNumber: number,
                    context: ctx,
                    characters: &*s,
                    charactersIgnoringModifiers: &*s,
                    isARepeat: false,
                    keyCode: key_code,
                ]
            };
            let event = event.ok_or("NSEvent refused the key event")?;
            let _: () = unsafe { msg_send![app, postEvent: &*event, atStart: false] };
        }
        Ok(())
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(key: &str, code: &str, shift: bool, meta: bool) -> KeyPress {
        KeyPress { key: key.into(), code: code.into(), shift, ctrl: false, alt: false, meta }
    }

    #[test]
    fn virtual_key_codes_invert_the_key_sources_table() {
        for (code, kc) in [
            ("KeyA", 0x00),
            ("KeyK", 0x28),
            ("Digit1", 0x12),
            ("Enter", 0x24),
            ("Tab", 0x30),
            ("Space", 0x31),
            ("Backspace", 0x33),
            ("Escape", 0x35),
            ("Delete", 0x75),
            ("ArrowLeft", 0x7B),
            ("ArrowUp", 0x7E),
            ("Home", 0x73),
            ("PageDown", 0x79),
            ("F1", 0x7A),
            ("F5", 0x60),
            ("Slash", 0x2C),
            ("MetaLeft", 0x37),
        ] {
            assert_eq!(virtual_key_code(code), kc, "{code}");
            assert_eq!(physical_code(kc), code);
        }
        assert_eq!(physical_code(virtual_key_code("")), "", "an unknown code reads back unknown");
    }

    #[test]
    fn characters_are_what_appkit_reports() {
        assert_eq!(characters("Enter").as_deref(), Some("\r"));
        assert_eq!(characters("Backspace").as_deref(), Some("\u{7f}"));
        assert_eq!(characters("Escape").as_deref(), Some("\u{1b}"));
        assert_eq!(characters("ArrowUp").as_deref(), Some("\u{F700}"));
        assert_eq!(characters("Delete").as_deref(), Some("\u{F728}"));
        assert_eq!(characters("F1").as_deref(), Some("\u{F704}"));
        assert_eq!(characters("F12").as_deref(), Some("\u{F70F}"));
        assert_eq!(characters("a").as_deref(), Some("a"));
        assert_eq!(characters(" ").as_deref(), Some(" "));
        assert_eq!(characters("Shift"), None);
    }

    #[test]
    fn a_shortcut_wraps_the_key_in_its_modifiers() {
        let seq = event_sequence(&press("k", "KeyK", false, true));
        let cmd = FLAG_COMMAND | DEV_LCMD;
        assert_eq!(
            seq,
            vec![
                (FLAGS_CHANGED, 0x37, cmd, String::new()),
                (KEY_DOWN, 0x28, cmd, "k".into()),
                (KEY_UP, 0x28, cmd, "k".into()),
                (FLAGS_CHANGED, 0x37, 0, String::new()),
            ]
        );
        let shift = event_sequence(&press("Shift", "ShiftLeft", false, false));
        assert_eq!(shift[0], (FLAGS_CHANGED, 0x38, FLAG_SHIFT | DEV_LSHIFT, String::new()));
        assert_eq!(shift[1], (FLAGS_CHANGED, 0x38, 0, String::new()));
        assert_eq!(event_sequence(&press("Enter", "Enter", false, false)).len(), 2);
    }

    /// REGRESSION (robot E2E `tab_moves_focus_between_fields` on macOS):
    /// Shift+Tab was posted as "\t" with the Shift flag, which the field
    /// editor reads as `insertTab:` — focus moved forward. AppKit types
    /// Shift+Tab as the back-tab character.
    #[test]
    fn regression_shift_tab_types_the_back_tab_character() {
        let tab = press("Tab", "Tab", false, false);
        assert!(event_sequence(&tab).iter().any(|(k, _, _, c)| *k == KEY_DOWN && c == "\t"));
        let back = press("Tab", "Tab", true, false);
        let down = event_sequence(&back).into_iter().find(|(k, ..)| *k == KEY_DOWN).unwrap();
        assert_eq!(down.3, "\u{19}");
        assert_eq!(down.1, 0x30, "still the Tab key");
        assert_ne!(down.2 & FLAG_SHIFT, 0);
    }
}
