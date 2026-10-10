//! App-level keyboard source: key down AND up for the whole window,
//! delivered into the framework's [`KeyboardSink`]
//! (`AppEnvOps::set_keyboard_sink`).
//!
//! ## Why the message PUMP, not the host `WndProc`
//!
//! Win32 posts a key message to the window with keyboard focus. When a
//! native control has focus — the `EDIT` behind `text_input` is a child
//! HWND — `WM_KEYDOWN` goes to the edit's own window procedure and the host
//! window never sees it, so a `WndProc` hook would go deaf exactly while an
//! input is focused. Every key message for every window of the thread does
//! pass through the host's `GetMessageW` loop, though, so the host calls
//! [`KeyboardSlot::pre_dispatch`] on each message BEFORE
//! `TranslateMessage`/`DispatchMessageW` — the same seam
//! `IsDialogMessage` / `TranslateAccelerator` use.
//!
//! `KeyOutcome::PreventDefault` makes the pump skip `TranslateMessage` and
//! `DispatchMessageW` for that message: the focused control never sees the
//! key and no `WM_CHAR` is generated, i.e. nothing is typed (Web
//! `preventDefault` on `keydown`). `WM_SYSKEYDOWN` (Alt chords, F10) is
//! delivered like any key and swallowed ONLY on `PreventDefault` — a
//! listener that returns `Default` keeps Alt+F4 / Alt+Space working.
//!
//! Limitation, inherent to Win32: while the OS runs a modal loop of its own
//! (title-bar drag/resize, a system menu, a `MessageBox`), messages are
//! pumped by that loop and bypass ours. Those loops start only from a
//! focus-taking interaction, and the window is not taking game input then.
//!
//! ## `key` for printable keys
//!
//! `key` has to be known at key-DOWN, before `TranslateMessage` produces
//! the `WM_CHAR`, so it comes from `ToUnicode` over the current keyboard
//! state (which `GetMessageW` has already updated for this message). The
//! call passes flag `0x4` — "do not change keyboard state" (Windows 10
//! 1607+) — so peeking does NOT consume a pending dead key: `´` then `e`
//! still types `é` in a focused input. On older Windows the flag is
//! ignored and a dead-key sequence could be disturbed. With Ctrl held,
//! `ToUnicode` yields a C0 control char (Ctrl+A → `0x01`); Web reports the
//! letter, so the translation retries with Ctrl cleared (AltGr = Ctrl+Alt
//! characters such as `@` on German layouts already translate on the first
//! try and are kept).
//!
//! ## Focus loss
//!
//! The host `WndProc` calls [`KeyboardSlot::focus_lost`] on
//! `WM_ACTIVATE(WA_INACTIVE)`: a deactivated window receives no further
//! key messages, including the releases of keys held at that moment.
//! `WM_KILLFOCUS` is NOT used — it also fires when focus moves from the
//! host window to one of its own child controls, which is not a loss.
//!
//! ## Re-entrancy
//!
//! The sink lives in a shared slot rather than behind the backend's
//! `RefCell`: the pump and the `WndProc` clone it out without borrowing
//! the backend, and the slot's own borrow is released before the sink
//! runs — a listener may add/remove listeners, which calls back into
//! `set_keyboard_sink` (writes this slot).

use std::cell::RefCell;
use std::rc::Rc;

use runtime_shared::primitives::key::{AppKeyEvent, KeyOutcome, KeyPhase, KeyboardSink};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, GetKeyboardState, MapVirtualKeyW, ToUnicode, MAPVK_VK_TO_VSC_EX, VK_CONTROL,
    VK_LCONTROL, VK_LWIN, VK_MENU, VK_RCONTROL, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::MSG;

use crate::keymap::{self, ToUnicodeResult};

/// `ToUnicode` `wFlags` bit 2: translate without changing the keyboard
/// state (dead-key buffer). Windows 10 1607+; see the module docs.
const TO_UNICODE_KEEP_STATE: u32 = 0x4;

/// The installed sink, shared between the backend (which writes it in
/// `set_keyboard_sink`) and the host shell (whose pump and `WndProc` read
/// it). Cloning shares the slot.
#[derive(Clone, Default)]
pub struct KeyboardSlot(Rc<RefCell<Option<KeyboardSink>>>);

impl KeyboardSlot {
    pub(crate) fn set(&self, sink: Option<KeyboardSink>) {
        *self.0.borrow_mut() = sink;
    }

    /// The installed sink, cloned out so no borrow is held while it runs.
    fn sink(&self) -> Option<KeyboardSink> {
        self.0.borrow().clone()
    }

    /// `true` while a sink is installed.
    pub fn is_installed(&self) -> bool {
        self.0.borrow().is_some()
    }

    /// Offer one retrieved message to the keyboard sink. Returns `true`
    /// when the sink asked to swallow it (`PreventDefault`): the caller
    /// must then skip `TranslateMessage` AND `DispatchMessageW`. Non-key
    /// messages, and every message while no sink is installed, return
    /// `false` untouched.
    pub fn pre_dispatch(&self, msg: &MSG) -> bool {
        let Some(down) = keymap::key_message_is_down(msg.message) else {
            return false;
        };
        let Some(sink) = self.sink() else {
            return false;
        };
        let ev = unsafe { translate(down, msg.wParam.0 as u32, msg.lParam.0) };
        matches!(sink.key(&ev), KeyOutcome::PreventDefault)
    }

    /// The window was deactivated: release every held key.
    pub fn focus_lost(&self) {
        if let Some(sink) = self.sink() {
            sink.focus_lost();
        }
    }
}

/// Build the [`AppKeyEvent`] for a key message. `unsafe` for the user32
/// keyboard-state reads; all of them are plain queries on the calling
/// (UI) thread's input state.
unsafe fn translate(down: bool, wparam_vk: u32, lparam: isize) -> AppKeyEvent {
    let vk = wparam_vk & 0xFFFF;
    let lp = keymap::decode_lparam(lparam);
    // Injected input (`SendInput` with a VK only) can carry scancode 0;
    // recover the physical key from the virtual key in that case.
    let (scancode, extended) = if lp.scancode == 0 {
        keymap::split_vsc_ex(MapVirtualKeyW(vk, MAPVK_VK_TO_VSC_EX))
    } else {
        (lp.scancode, lp.extended)
    };
    let code = keymap::scancode_to_code(scancode, extended);
    let held = |vk: u16| GetKeyState(vk as i32) < 0;
    let (shift, ctrl, alt, meta) = keymap::own_modifier_flags(
        down,
        code,
        held(VK_SHIFT.0),
        held(VK_CONTROL.0),
        held(VK_MENU.0),
        held(VK_LWIN.0) || held(VK_RWIN.0),
    );
    AppKeyEvent {
        phase: if down { KeyPhase::Down } else { KeyPhase::Up },
        key: key_for(vk, scancode),
        code: code.to_string(),
        repeat: down && lp.was_down,
        shift,
        ctrl,
        alt,
        meta,
    }
}

/// Web `KeyboardEvent.key` for a virtual key. See the module docs.
unsafe fn key_for(vk: u32, scancode: u32) -> String {
    if let Some(named) = keymap::vk_named_key(vk) {
        return named.to_string();
    }
    let mut state = [0u8; 256];
    if GetKeyboardState(&mut state).is_err() {
        return "Unidentified".to_string();
    }
    let mut buf = [0u16; 8];
    let ret = ToUnicode(vk, scancode, Some(&state), &mut buf, TO_UNICODE_KEEP_STATE);
    match keymap::classify_to_unicode(ret, &buf) {
        ToUnicodeResult::Text(t) => t,
        ToUnicodeResult::Dead => "Dead".to_string(),
        ToUnicodeResult::Control => {
            for k in [VK_CONTROL, VK_LCONTROL, VK_RCONTROL] {
                state[k.0 as usize] = 0;
            }
            let mut buf = [0u16; 8];
            let ret = ToUnicode(vk, scancode, Some(&state), &mut buf, TO_UNICODE_KEEP_STATE);
            match keymap::classify_to_unicode(ret, &buf) {
                ToUnicodeResult::Text(t) => t,
                _ => "Unidentified".to_string(),
            }
        }
        ToUnicodeResult::Nothing => "Unidentified".to_string(),
    }
}

#[cfg(test)]
mod tests {
    //! Runs only on a Windows target (the crate is `cfg(windows)`); on the
    //! macOS dev machine it runs under Wine via
    //! `cargo test --target x86_64-pc-windows-gnu` with a Wine runner.
    //! The pure tables are tested on every host by `tests/keymap_tables.rs`.
    use super::*;
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};

    fn key_msg(message: u32, vk: u32, scancode: u32, extended: bool, was_down: bool) -> MSG {
        let lparam = 1 | (scancode << 16) as isize | ((extended as isize) << 24)
            | ((was_down as isize) << 30);
        MSG {
            hwnd: HWND(std::ptr::null_mut()),
            message,
            wParam: WPARAM(vk as usize),
            lParam: LPARAM(lparam),
            ..Default::default()
        }
    }

    fn recording(log: Rc<RefCell<Vec<AppKeyEvent>>>, lost: Rc<RefCell<u32>>) -> KeyboardSink {
        KeyboardSink::new(
            move |ev| {
                log.borrow_mut().push(ev.clone());
                if ev.code.starts_with("Arrow") {
                    KeyOutcome::PreventDefault
                } else {
                    KeyOutcome::Default
                }
            },
            move || *lost.borrow_mut() += 1,
        )
    }

    #[test]
    fn regression_win32_app_keyboard_delivers_down_up_repeat_and_physical_code() {
        let slot = KeyboardSlot::default();
        let log = Rc::new(RefCell::new(Vec::new()));
        let lost = Rc::new(RefCell::new(0));

        // No sink: nothing is consumed or delivered.
        assert!(!slot.pre_dispatch(&key_msg(keymap::WM_KEYDOWN, 0x57, 0x11, false, false)));

        slot.set(Some(recording(log.clone(), lost.clone())));
        // 'W' (VK 0x57, scancode 0x11) down, auto-repeat, up.
        assert!(!slot.pre_dispatch(&key_msg(keymap::WM_KEYDOWN, 0x57, 0x11, false, false)));
        assert!(!slot.pre_dispatch(&key_msg(keymap::WM_KEYDOWN, 0x57, 0x11, false, true)));
        assert!(!slot.pre_dispatch(&key_msg(keymap::WM_KEYUP, 0x57, 0x11, false, true)));
        // Up arrow: extended 0x48. PreventDefault → swallowed.
        assert!(slot.pre_dispatch(&key_msg(keymap::WM_KEYDOWN, 0x26, 0x48, true, false)));
        // Alt via WM_SYSKEYDOWN is an ordinary key; Default → not swallowed.
        assert!(!slot.pre_dispatch(&key_msg(keymap::WM_SYSKEYDOWN, 0x12, 0x38, false, false)));
        // A non-key message is ignored.
        assert!(!slot.pre_dispatch(&MSG { message: 0x0200, ..Default::default() }));

        let log = log.borrow();
        assert_eq!(log.len(), 5, "{log:?}");
        assert_eq!((log[0].phase, log[0].code.as_str(), log[0].repeat), (KeyPhase::Down, "KeyW", false));
        assert_eq!(log[0].key.to_lowercase(), "w");
        assert_eq!((log[1].phase, log[1].repeat), (KeyPhase::Down, true));
        assert_eq!((log[2].phase, log[2].code.as_str(), log[2].repeat), (KeyPhase::Up, "KeyW", false));
        assert_eq!((log[3].key.as_str(), log[3].code.as_str()), ("ArrowUp", "ArrowUp"));
        assert_eq!((log[4].key.as_str(), log[4].code.as_str(), log[4].alt), ("Alt", "AltLeft", true));
        drop(log);

        slot.focus_lost();
        assert_eq!(*lost.borrow(), 1);

        // None removes: later messages pass through undelivered.
        slot.set(None);
        assert!(!slot.is_installed());
        assert!(!slot.pre_dispatch(&key_msg(keymap::WM_KEYDOWN, 0x26, 0x48, true, false)));
        slot.focus_lost();
        assert_eq!(*lost.borrow(), 1);
    }

    #[test]
    fn a_listener_may_replace_the_sink_from_inside_delivery() {
        // Re-entrancy: the slot's borrow must be released before the sink
        // runs, or a listener that removes itself would double-borrow.
        let slot = KeyboardSlot::default();
        let inner = slot.clone();
        slot.set(Some(KeyboardSink::new(
            move |_| {
                inner.set(None);
                KeyOutcome::Default
            },
            || {},
        )));
        slot.pre_dispatch(&key_msg(keymap::WM_KEYDOWN, 0x41, 0x1E, false, false));
        assert!(!slot.is_installed());
    }
}
