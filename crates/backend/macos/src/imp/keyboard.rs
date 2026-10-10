//! App-level keyboard source for the macOS backend.
//!
//! Unlike the per-`NSTextField` `on_key_down` (focus-scoped), this installs a
//! single `NSEvent addLocalMonitorForEventsMatchingMask:` monitor for
//! key-down, key-up AND flags-changed events. A local monitor sees every key
//! event the app dequeues regardless of which view is first responder — a
//! focused text field still receives its keys afterwards (the monitor returns
//! the event unchanged unless a listener claims it). Each event becomes an
//! [`AppKeyEvent`] delivered into the framework's [`KeyboardSink`].
//! Drives `MacosBackend::set_keyboard_sink_impl`.
//!
//! - **Modifier keys** never produce `keyDown`/`keyUp` on macOS — they arrive
//!   as `NSEventTypeFlagsChanged`, which carries no up/down. The phase is
//!   derived from the modifier flags *after* the change
//!   ([`flags_changed_phase`]), preferring the device-dependent left/right
//!   bits so releasing Right-Shift while Left-Shift is held reports an `Up`.
//! - **`code`** comes from the hardware virtual key code (`kVK_*`), never from
//!   the character, so WASD stays physical on AZERTY ([`physical_code`]).
//! - **Focus loss**: AppKit delivers no key-up for keys held when the app
//!   deactivates or a window resigns key, so we observe
//!   `NSApplicationDidResignActiveNotification` /
//!   `NSWindowDidResignKeyNotification` and call
//!   [`KeyboardSink::focus_lost`]; the dispatcher synthesizes the releases.

use std::cell::RefCell;
use std::rc::Rc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::{class, msg_send};
use objc2_foundation::{NSObject, NSString};
use runtime_shared::primitives::key::{AppKeyEvent, KeyOutcome, KeyPhase, KeyboardSink};

use super::MacosBackend;

/// `NSEventType` values for the three event kinds the monitor watches.
const NS_EVENT_TYPE_KEY_DOWN: usize = 10;
const NS_EVENT_TYPE_KEY_UP: usize = 11;
const NS_EVENT_TYPE_FLAGS_CHANGED: usize = 12;

/// `NSEventMask*` = `1 << NSEventType*`.
const NS_EVENT_MASK_KEYS: usize =
    (1 << NS_EVENT_TYPE_KEY_DOWN) | (1 << NS_EVENT_TYPE_KEY_UP) | (1 << NS_EVENT_TYPE_FLAGS_CHANGED);

// Device-independent `NSEventModifierFlags` bits.
const FLAG_CAPS_LOCK: usize = 1 << 16;
const FLAG_SHIFT: usize = 1 << 17;
const FLAG_CONTROL: usize = 1 << 18;
const FLAG_OPTION: usize = 1 << 19;
const FLAG_COMMAND: usize = 1 << 20;
const FLAG_FUNCTION: usize = 1 << 23;

// Device-DEPENDENT modifier bits (IOKit `NX_DEVICE*KEYMASK`), reported in the
// low 16 bits of `modifierFlags` alongside the independent ones. They are the
// only way to tell which side of a paired modifier is down.
const DEV_LCTL: usize = 0x0001;
const DEV_LSHIFT: usize = 0x0002;
const DEV_RSHIFT: usize = 0x0004;
const DEV_LCMD: usize = 0x0008;
const DEV_RCMD: usize = 0x0010;
const DEV_LALT: usize = 0x0020;
const DEV_RALT: usize = 0x0040;
const DEV_RCTL: usize = 0x2000;

/// The three key-event kinds the monitor delivers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RawKind {
    KeyDown,
    KeyUp,
    FlagsChanged,
}

impl RawKind {
    fn from_event_type(t: usize) -> Option<Self> {
        match t {
            NS_EVENT_TYPE_KEY_DOWN => Some(RawKind::KeyDown),
            NS_EVENT_TYPE_KEY_UP => Some(RawKind::KeyUp),
            NS_EVENT_TYPE_FLAGS_CHANGED => Some(RawKind::FlagsChanged),
            _ => None,
        }
    }
}

/// Everything read off an `NSEvent` — the pure input to [`translate`].
/// `chars` / `is_repeat` are only meaningful (and only READ — see
/// [`read_raw`]) for key-down/up.
#[derive(Clone, Debug)]
pub(crate) struct RawKey {
    pub kind: RawKind,
    pub key_code: u16,
    pub flags: usize,
    pub chars: Option<String>,
    pub is_repeat: bool,
}

/// Physical key for an AppKit virtual key code (`kVK_*`, Carbon
/// `HIToolbox/Events.h`), in Web `KeyboardEvent.code` vocabulary. Matches the
/// codes Chromium/WebKit report for the same keys on macOS. `""` when unknown.
pub(crate) fn physical_code(key_code: u16) -> &'static str {
    match key_code {
        0x00 => "KeyA",
        0x01 => "KeyS",
        0x02 => "KeyD",
        0x03 => "KeyF",
        0x04 => "KeyH",
        0x05 => "KeyG",
        0x06 => "KeyZ",
        0x07 => "KeyX",
        0x08 => "KeyC",
        0x09 => "KeyV",
        0x0A => "IntlBackslash", // kVK_ISO_Section
        0x0B => "KeyB",
        0x0C => "KeyQ",
        0x0D => "KeyW",
        0x0E => "KeyE",
        0x0F => "KeyR",
        0x10 => "KeyY",
        0x11 => "KeyT",
        0x12 => "Digit1",
        0x13 => "Digit2",
        0x14 => "Digit3",
        0x15 => "Digit4",
        0x16 => "Digit6",
        0x17 => "Digit5",
        0x18 => "Equal",
        0x19 => "Digit9",
        0x1A => "Digit7",
        0x1B => "Minus",
        0x1C => "Digit8",
        0x1D => "Digit0",
        0x1E => "BracketRight",
        0x1F => "KeyO",
        0x20 => "KeyU",
        0x21 => "BracketLeft",
        0x22 => "KeyI",
        0x23 => "KeyP",
        0x24 => "Enter",
        0x25 => "KeyL",
        0x26 => "KeyJ",
        0x27 => "Quote",
        0x28 => "KeyK",
        0x29 => "Semicolon",
        0x2A => "Backslash",
        0x2B => "Comma",
        0x2C => "Slash",
        0x2D => "KeyN",
        0x2E => "KeyM",
        0x2F => "Period",
        0x30 => "Tab",
        0x31 => "Space",
        0x32 => "Backquote",
        0x33 => "Backspace", // kVK_Delete
        0x35 => "Escape",
        0x36 => "MetaRight",
        0x37 => "MetaLeft",
        0x38 => "ShiftLeft",
        0x39 => "CapsLock",
        0x3A => "AltLeft",
        0x3B => "ControlLeft",
        0x3C => "ShiftRight",
        0x3D => "AltRight",
        0x3E => "ControlRight",
        0x3F => "Fn",
        0x40 => "F17",
        0x41 => "NumpadDecimal",
        0x43 => "NumpadMultiply",
        0x45 => "NumpadAdd",
        0x47 => "NumLock", // kVK_ANSI_KeypadClear — Chromium maps it to NumLock
        0x48 => "AudioVolumeUp",
        0x49 => "AudioVolumeDown",
        0x4A => "AudioVolumeMute",
        0x4B => "NumpadDivide",
        0x4C => "NumpadEnter",
        0x4E => "NumpadSubtract",
        0x4F => "F18",
        0x50 => "F19",
        0x51 => "NumpadEqual",
        0x52 => "Numpad0",
        0x53 => "Numpad1",
        0x54 => "Numpad2",
        0x55 => "Numpad3",
        0x56 => "Numpad4",
        0x57 => "Numpad5",
        0x58 => "Numpad6",
        0x59 => "Numpad7",
        0x5A => "F20",
        0x5B => "Numpad8",
        0x5C => "Numpad9",
        0x5D => "IntlYen",
        0x5E => "IntlRo",
        0x5F => "NumpadComma",
        0x60 => "F5",
        0x61 => "F6",
        0x62 => "F7",
        0x63 => "F3",
        0x64 => "F8",
        0x65 => "F9",
        0x66 => "Lang2", // kVK_JIS_Eisu
        0x67 => "F11",
        0x68 => "Lang1", // kVK_JIS_Kana
        0x69 => "F13",
        0x6A => "F16",
        0x6B => "F14",
        0x6D => "F10",
        0x6E => "ContextMenu",
        0x6F => "F12",
        0x71 => "F15",
        0x72 => "Insert", // kVK_Help — sits where Insert is on PC layouts
        0x73 => "Home",
        0x74 => "PageUp",
        0x75 => "Delete", // kVK_ForwardDelete
        0x76 => "F4",
        0x77 => "End",
        0x78 => "F2",
        0x79 => "PageDown",
        0x7A => "F1",
        0x7B => "ArrowLeft",
        0x7C => "ArrowRight",
        0x7D => "ArrowDown",
        0x7E => "ArrowUp",
        _ => "",
    }
}

/// Web `KeyboardEvent.key` for the keys whose `key` is a NAME rather than the
/// character they type (arrows, editing keys, function keys, modifiers).
/// Printable keys return `None` and use `charactersIgnoringModifiers`, so
/// `+`/`-`/`=`/letters are themselves. Function keys MUST be named here: their
/// characters are private-use code points (`NSF1FunctionKey` = U+F704, …).
pub(crate) fn key_name(key_code: u16) -> Option<&'static str> {
    Some(match key_code {
        0x7B => "ArrowLeft",
        0x7C => "ArrowRight",
        0x7D => "ArrowDown",
        0x7E => "ArrowUp",
        0x24 | 0x4C => "Enter", // Return + keypad Enter
        0x30 => "Tab",
        0x31 => " ", // Space — Web reports a literal space
        0x33 => "Backspace",
        0x35 => "Escape",
        0x75 => "Delete", // forward delete
        0x73 => "Home",
        0x77 => "End",
        0x74 => "PageUp",
        0x79 => "PageDown",
        0x72 => "Insert",
        0x47 => "Clear",
        0x6E => "ContextMenu",
        0x38 | 0x3C => "Shift",
        0x3B | 0x3E => "Control",
        0x3A | 0x3D => "Alt",
        0x37 | 0x36 => "Meta",
        0x39 => "CapsLock",
        0x3F => "Fn",
        0x7A => "F1",
        0x78 => "F2",
        0x63 => "F3",
        0x76 => "F4",
        0x60 => "F5",
        0x61 => "F6",
        0x62 => "F7",
        0x64 => "F8",
        0x65 => "F9",
        0x6D => "F10",
        0x67 => "F11",
        0x6F => "F12",
        0x69 => "F13",
        0x6B => "F14",
        0x71 => "F15",
        0x6A => "F16",
        0x40 => "F17",
        0x4F => "F18",
        0x50 => "F19",
        0x5A => "F20",
        _ => return None,
    })
}

/// `(this side's device bit, other side's device bit, independent flag)` for
/// a modifier key code; `None` for non-modifier keys. Unpaired modifiers
/// (Caps Lock, Fn) have no device bits.
fn modifier_bits(key_code: u16) -> Option<(usize, usize, usize)> {
    Some(match key_code {
        0x38 => (DEV_LSHIFT, DEV_RSHIFT, FLAG_SHIFT),
        0x3C => (DEV_RSHIFT, DEV_LSHIFT, FLAG_SHIFT),
        0x3B => (DEV_LCTL, DEV_RCTL, FLAG_CONTROL),
        0x3E => (DEV_RCTL, DEV_LCTL, FLAG_CONTROL),
        0x3A => (DEV_LALT, DEV_RALT, FLAG_OPTION),
        0x3D => (DEV_RALT, DEV_LALT, FLAG_OPTION),
        0x37 => (DEV_LCMD, DEV_RCMD, FLAG_COMMAND),
        0x36 => (DEV_RCMD, DEV_LCMD, FLAG_COMMAND),
        0x39 => (0, 0, FLAG_CAPS_LOCK),
        0x3F => (0, 0, FLAG_FUNCTION),
        _ => return None,
    })
}

/// Down/up for a `flagsChanged` event — AppKit reports only the modifier
/// flags AFTER the change. `None` for a key code that isn't a modifier.
///
/// - This side's device-dependent bit set → `Down`.
/// - Independent flag clear → `Up` (no side of this modifier is held).
/// - Independent flag set, this side's device bit clear: if the OTHER side's
///   device bit is set, this side was released while the other is still held
///   → `Up`. If no device bits are reported at all (some synthetic/remote
///   events carry only the independent flags), fall back to the independent
///   flag → `Down`.
///
/// Caps Lock follows its lock state (on → `Down`, off → `Up`), which is what
/// browsers on macOS report for it.
pub(crate) fn flags_changed_phase(key_code: u16, flags: usize) -> Option<KeyPhase> {
    let (this_dev, other_dev, independent) = modifier_bits(key_code)?;
    if this_dev != 0 && flags & this_dev != 0 {
        return Some(KeyPhase::Down);
    }
    if flags & independent == 0 {
        return Some(KeyPhase::Up);
    }
    if this_dev != 0 && flags & other_dev != 0 {
        return Some(KeyPhase::Up);
    }
    Some(KeyPhase::Down)
}

/// Translate one raw key event into the framework's [`AppKeyEvent`]. `None`
/// for a `flagsChanged` on a key that isn't a modifier we track.
pub(crate) fn translate(raw: &RawKey) -> Option<AppKeyEvent> {
    let (phase, repeat) = match raw.kind {
        RawKind::KeyDown => (KeyPhase::Down, raw.is_repeat),
        RawKind::KeyUp => (KeyPhase::Up, false),
        RawKind::FlagsChanged => (flags_changed_phase(raw.key_code, raw.flags)?, false),
    };
    let key = match key_name(raw.key_code) {
        Some(name) => name.to_string(),
        None => match raw.chars.as_deref() {
            // Private-use function-key code points (U+F700–U+F8FF) are not a
            // `key` value any web listener would match — report it unknown.
            Some(s) if s.chars().any(|c| ('\u{F700}'..='\u{F8FF}').contains(&c)) => "Unidentified".to_string(),
            Some(s) if !s.is_empty() => s.to_string(),
            _ => "Unidentified".to_string(),
        },
    };
    Some(AppKeyEvent {
        phase,
        key,
        code: physical_code(raw.key_code).to_string(),
        repeat,
        shift: raw.flags & FLAG_SHIFT != 0,
        ctrl: raw.flags & FLAG_CONTROL != 0,
        alt: raw.flags & FLAG_OPTION != 0,
        meta: raw.flags & FLAG_COMMAND != 0,
    })
}

/// Read the fields [`translate`] needs off an `NSEvent`. `None` for an event
/// type the monitor doesn't handle.
///
/// `charactersIgnoringModifiers` and `isARepeat` are ONLY read for
/// key-down/up: on an `NSEventTypeFlagsChanged` event both raise an
/// Objective-C `NSInternalInconsistencyException`, which would abort the app.
unsafe fn read_raw(event: *mut NSObject) -> Option<RawKey> {
    let event_type: usize = msg_send![event, type];
    let kind = RawKind::from_event_type(event_type)?;
    let key_code: u16 = msg_send![event, keyCode];
    let flags: usize = msg_send![event, modifierFlags];
    let (chars, is_repeat) = match kind {
        RawKind::KeyDown | RawKind::KeyUp => {
            let s: *mut NSString = msg_send![event, charactersIgnoringModifiers];
            let chars = if s.is_null() { None } else { Some((*s).to_string()) };
            let rep: bool = msg_send![event, isARepeat];
            (chars, rep)
        }
        RawKind::FlagsChanged => (None, false),
    };
    Some(RawKey { kind, key_code, flags, chars, is_repeat })
}

/// The installed app-level key source: the live sink (swappable without
/// reinstalling), the `NSEvent` monitor token, and the focus-loss observers.
pub(crate) struct AppKeyboard {
    /// Shared with the monitor block and the observers. Callers CLONE the sink
    /// out and drop the borrow before calling it: a listener may add/remove
    /// listeners, which re-enters `set_keyboard_sink` on this backend.
    sink: Rc<RefCell<Option<KeyboardSink>>>,
    monitor: Retained<NSObject>,
    observers: Vec<Retained<NSObject>>,
}

impl AppKeyboard {
    fn teardown(self) {
        // Clear the slot first: a focus-loss callback already queued on the
        // main queue (observers defer one turn) then finds no sink.
        self.sink.borrow_mut().take();
        unsafe {
            let _: () = msg_send![class!(NSEvent), removeMonitor: &*self.monitor];
        }
        for observer in &self.observers {
            super::callbacks::remove_observer(observer);
        }
    }
}

/// Install (or, with `None`, remove) the app-level keyboard source on
/// `backend`. A `Some` while already installed swaps the sink in place.
pub(crate) fn set_keyboard_sink(backend: &mut MacosBackend, sink: Option<KeyboardSink>) {
    let Some(sink) = sink else {
        if let Some(kb) = backend.app_keyboard.take() {
            kb.teardown();
        }
        return;
    };
    if let Some(kb) = backend.app_keyboard.as_ref() {
        *kb.sink.borrow_mut() = Some(sink);
        return;
    }

    let slot: Rc<RefCell<Option<KeyboardSink>>> = Rc::new(RefCell::new(Some(sink)));

    // The monitor block: NSEvent → AppKeyEvent → sink. Returns `nil` to
    // SWALLOW a key-down/up a listener claimed (`PreventDefault`) — so AppKit
    // doesn't `NSBeep` on an unhandled key — and the event unchanged
    // otherwise so normal key routing (a focused text field) continues.
    // `flagsChanged` is NEVER swallowed: AppKit tracks modifier state from
    // it, and eating one would leave the rest of the app with a stuck
    // modifier. Crash-loud per project policy (an FFI callback that unwinds
    // aborts).
    let block_slot = slot.clone();
    let block = RcBlock::new(move |event: *mut NSObject| -> *mut NSObject {
        let outcome = backend_apple_core::crash::abort_on_panic("app key monitor", || unsafe {
            let Some(raw) = read_raw(event) else {
                return None;
            };
            let ev = translate(&raw)?;
            let sink = block_slot.borrow().clone()?;
            Some((raw.kind, sink.key(&ev)))
        });
        match outcome {
            Some((RawKind::KeyDown | RawKind::KeyUp, KeyOutcome::PreventDefault)) => std::ptr::null_mut(),
            _ => event,
        }
    });

    // `addLocalMonitor…` copies the handler block internally, so the local
    // `block` may drop after this; we retain the returned monitor token to feed
    // `removeMonitor:` later.
    let monitor: *mut NSObject = unsafe {
        msg_send![
            class!(NSEvent),
            addLocalMonitorForEventsMatchingMask: NS_EVENT_MASK_KEYS,
            handler: &*block,
        ]
    };
    let Some(monitor) = (unsafe { Retained::retain(monitor) }) else {
        return;
    };

    // Focus loss: the app deactivating, or any of its windows resigning key
    // (key focus moved to another window/app). Releases for keys held now
    // will never arrive. `observe_notification` runs the callback on the
    // next main-queue turn, which is fine here — no key event can be
    // delivered to us in between that the dispatcher would mis-order.
    let observers = ["NSApplicationDidResignActiveNotification", "NSWindowDidResignKeyNotification"]
        .into_iter()
        .map(|name| {
            let slot = slot.clone();
            super::callbacks::observe_notification(
                backend.mtm,
                name,
                std::ptr::null_mut(),
                Rc::new(move || {
                    let sink = slot.borrow().clone();
                    if let Some(sink) = sink {
                        sink.focus_lost();
                    }
                }),
            )
        })
        .collect();

    backend.app_keyboard = Some(AppKeyboard { sink: slot, monitor, observers });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(kind: RawKind, key_code: u16, flags: usize, chars: Option<&str>, is_repeat: bool) -> RawKey {
        RawKey { kind, key_code, flags, chars: chars.map(str::to_string), is_repeat }
    }

    /// The physical-key table: a letter, a digit, the movement keys, and the
    /// layout-scrambled kVK ordering (kVK_ANSI_5 = 0x17, kVK_ANSI_6 = 0x16).
    #[test]
    fn physical_code_table_matches_web_code_vocabulary() {
        for (kc, code) in [
            (0x0D, "KeyW"),
            (0x00, "KeyA"),
            (0x01, "KeyS"),
            (0x02, "KeyD"),
            (0x12, "Digit1"),
            (0x17, "Digit5"),
            (0x16, "Digit6"),
            (0x1D, "Digit0"),
            (0x31, "Space"),
            (0x24, "Enter"),
            (0x4C, "NumpadEnter"),
            (0x35, "Escape"),
            (0x30, "Tab"),
            (0x33, "Backspace"),
            (0x75, "Delete"),
            (0x7B, "ArrowLeft"),
            (0x7C, "ArrowRight"),
            (0x7D, "ArrowDown"),
            (0x7E, "ArrowUp"),
            (0x38, "ShiftLeft"),
            (0x3C, "ShiftRight"),
            (0x3B, "ControlLeft"),
            (0x3E, "ControlRight"),
            (0x3A, "AltLeft"),
            (0x3D, "AltRight"),
            (0x37, "MetaLeft"),
            (0x36, "MetaRight"),
            (0x39, "CapsLock"),
            (0x1B, "Minus"),
            (0x18, "Equal"),
            (0x21, "BracketLeft"),
            (0x1E, "BracketRight"),
            (0x2A, "Backslash"),
            (0x29, "Semicolon"),
            (0x27, "Quote"),
            (0x32, "Backquote"),
            (0x2B, "Comma"),
            (0x2F, "Period"),
            (0x2C, "Slash"),
            (0x7A, "F1"),
            (0x6F, "F12"),
            (0x73, "Home"),
            (0x77, "End"),
            (0x74, "PageUp"),
            (0x79, "PageDown"),
            (0x72, "Insert"),
            (0x52, "Numpad0"),
            (0x5C, "Numpad9"),
            (0x45, "NumpadAdd"),
            (0x4E, "NumpadSubtract"),
        ] {
            assert_eq!(physical_code(kc), code, "kVK 0x{kc:02X}");
        }
        assert_eq!(physical_code(0x34), "", "unassigned kVK → empty code");
        assert_eq!(physical_code(0xFF), "");
    }

    /// Every F-key has a name (their characters are private-use code points).
    #[test]
    fn function_keys_are_named() {
        for (kc, name) in [
            (0x7A, "F1"),
            (0x78, "F2"),
            (0x63, "F3"),
            (0x76, "F4"),
            (0x60, "F5"),
            (0x61, "F6"),
            (0x62, "F7"),
            (0x64, "F8"),
            (0x65, "F9"),
            (0x6D, "F10"),
            (0x67, "F11"),
            (0x6F, "F12"),
        ] {
            assert_eq!(key_name(kc), Some(name));
            assert_eq!(physical_code(kc), name);
        }
    }

    /// REGRESSION (game controls): the app-level hook delivered key-DOWN only,
    /// so a held key never released. A `keyUp` now translates to an `Up` with
    /// the same physical `code`, even though its `key` changed case because
    /// Shift was released first.
    #[test]
    fn regression_key_up_is_delivered_with_physical_code() {
        let down = translate(&raw(RawKind::KeyDown, 0x0D, FLAG_SHIFT, Some("W"), false)).unwrap();
        assert_eq!(down.phase, KeyPhase::Down);
        assert_eq!((down.key.as_str(), down.code.as_str()), ("W", "KeyW"));
        assert!(down.shift && !down.repeat);

        let up = translate(&raw(RawKind::KeyUp, 0x0D, 0, Some("w"), false)).unwrap();
        assert_eq!(up.phase, KeyPhase::Up);
        assert_eq!((up.key.as_str(), up.code.as_str()), ("w", "KeyW"));
        assert!(!up.shift);
    }

    #[test]
    fn key_down_carries_platform_repeat_flag_and_key_up_never_repeats() {
        let rep = translate(&raw(RawKind::KeyDown, 0x00, 0, Some("a"), true)).unwrap();
        assert!(rep.repeat);
        let up = translate(&raw(RawKind::KeyUp, 0x00, 0, Some("a"), true)).unwrap();
        assert!(!up.repeat, "Up is never a repeat");
    }

    #[test]
    fn named_keys_and_private_use_chars() {
        let space = translate(&raw(RawKind::KeyDown, 0x31, 0, Some(" "), false)).unwrap();
        assert_eq!((space.key.as_str(), space.code.as_str()), (" ", "Space"));
        let up = translate(&raw(RawKind::KeyDown, 0x7E, 0, Some("\u{F700}"), false)).unwrap();
        assert_eq!((up.key.as_str(), up.code.as_str()), ("ArrowUp", "ArrowUp"));
        // An unnamed key whose characters are a private-use function-key
        // code point (e.g. F21+) → "Unidentified", never the raw code point.
        let odd = translate(&raw(RawKind::KeyDown, 0x34, 0, Some("\u{F718}"), false)).unwrap();
        assert_eq!((odd.key.as_str(), odd.code.as_str()), ("Unidentified", ""));
        let modded = translate(&raw(RawKind::KeyDown, 0x08, FLAG_COMMAND | FLAG_CONTROL, Some("c"), false)).unwrap();
        assert!(modded.meta && modded.ctrl && !modded.alt && !modded.shift);
    }

    /// REGRESSION (game controls): holding Shift as a "run" key produced no
    /// event at all — macOS reports modifiers only as `flagsChanged`, which the
    /// monitor didn't watch. Press + release of Left Shift now map to
    /// Down + Up with key "Shift" / code "ShiftLeft".
    #[test]
    fn regression_modifier_keys_produce_down_and_up() {
        let down = translate(&raw(RawKind::FlagsChanged, 0x38, FLAG_SHIFT | DEV_LSHIFT, None, false)).unwrap();
        assert_eq!(down.phase, KeyPhase::Down);
        assert_eq!((down.key.as_str(), down.code.as_str()), ("Shift", "ShiftLeft"));
        assert!(down.shift && !down.repeat);

        let up = translate(&raw(RawKind::FlagsChanged, 0x38, 0, None, false)).unwrap();
        assert_eq!(up.phase, KeyPhase::Up);
        assert!(!up.shift);

        for (kc, flag, dev, key, code) in [
            (0x3B, FLAG_CONTROL, DEV_LCTL, "Control", "ControlLeft"),
            (0x3E, FLAG_CONTROL, DEV_RCTL, "Control", "ControlRight"),
            (0x3A, FLAG_OPTION, DEV_LALT, "Alt", "AltLeft"),
            (0x3D, FLAG_OPTION, DEV_RALT, "Alt", "AltRight"),
            (0x37, FLAG_COMMAND, DEV_LCMD, "Meta", "MetaLeft"),
            (0x36, FLAG_COMMAND, DEV_RCMD, "Meta", "MetaRight"),
            (0x3C, FLAG_SHIFT, DEV_RSHIFT, "Shift", "ShiftRight"),
        ] {
            let d = translate(&raw(RawKind::FlagsChanged, kc, flag | dev, None, false)).unwrap();
            assert_eq!((d.phase, d.key.as_str(), d.code.as_str()), (KeyPhase::Down, key, code));
            let u = translate(&raw(RawKind::FlagsChanged, kc, 0, None, false)).unwrap();
            assert_eq!(u.phase, KeyPhase::Up, "{code}");
        }
    }

    /// Releasing Right Shift while Left Shift is still held leaves the
    /// device-independent Shift flag SET — only the device bits tell the
    /// right side went up.
    #[test]
    fn releasing_one_side_while_other_side_held_is_up() {
        // Left down, then Right down.
        assert_eq!(flags_changed_phase(0x38, FLAG_SHIFT | DEV_LSHIFT), Some(KeyPhase::Down));
        assert_eq!(flags_changed_phase(0x3C, FLAG_SHIFT | DEV_LSHIFT | DEV_RSHIFT), Some(KeyPhase::Down));
        // Right released; Left still held.
        assert_eq!(flags_changed_phase(0x3C, FLAG_SHIFT | DEV_LSHIFT), Some(KeyPhase::Up));
        // Left released last.
        assert_eq!(flags_changed_phase(0x38, 0), Some(KeyPhase::Up));
    }

    /// Events carrying only the device-independent flags (no left/right
    /// bits) still get a phase from the independent flag.
    #[test]
    fn flags_changed_without_device_bits_falls_back_to_independent_flag() {
        assert_eq!(flags_changed_phase(0x3A, FLAG_OPTION), Some(KeyPhase::Down));
        assert_eq!(flags_changed_phase(0x3A, 0), Some(KeyPhase::Up));
    }

    #[test]
    fn caps_lock_follows_lock_state_and_non_modifiers_are_ignored() {
        assert_eq!(flags_changed_phase(0x39, FLAG_CAPS_LOCK), Some(KeyPhase::Down));
        assert_eq!(flags_changed_phase(0x39, 0), Some(KeyPhase::Up));
        assert_eq!(flags_changed_phase(0x00, FLAG_SHIFT), None);
        assert!(translate(&raw(RawKind::FlagsChanged, 0x00, FLAG_SHIFT, None, false)).is_none());
    }
}
