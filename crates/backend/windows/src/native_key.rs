//! Real Win32 key input for the robot (`IntrospectionOps::native_key`).
//!
//! A press is POSTED as the key-down and key-up messages a keyboard
//! produces (`WM_KEYDOWN` / `WM_KEYUP`, the `WM_SYS*` pair while Alt is
//! held), with the virtual key in `wParam` and the scancode, extended flag
//! and transition bits in `lParam`, to the window holding keyboard focus
//! (`GetFocus`: the `EDIT` behind a focused `text_input` — the robot
//! focuses the target first — else the host window). Posting, not sending,
//! puts them through the host's message loop exactly as input from the
//! system queue: the focused field's `on_key_down` (`field_keys`), the
//! app-level keyboard (`app_keys`), Escape-to-dismiss for the topmost
//! overlay, then `TranslateMessage` (which makes the `WM_CHAR` that types a
//! character, unless a handler prevented the key) and the control's own
//! window procedure (Backspace deletes, arrows move the caret).
//!
//! ## Modifiers: the keyboard state, not modifier messages
//!
//! A posted key message does not change the thread's keyboard state —
//! only input from the system queue does — yet everything downstream reads
//! that state: `app_keys::translate` takes shift/ctrl/alt/meta from
//! `GetKeyState`, its `key` comes from `ToUnicode` over
//! `GetKeyboardState`, and `TranslateMessage` picks the character (`!` vs
//! `1`) from it. So the injection sets the press's modifiers in the
//! thread's state (`SetKeyboardState`) before posting and clears the bits
//! it set once the queue has drained: a thread timer, because `WM_TIMER` is
//! only generated when no posted message is waiting, so it fires after the
//! key messages and the `WM_CHAR` they translate to. The modifier keys'
//! own down/up messages are not posted: a lone Alt release would open the
//! window menu's modal loop, and no handler needs them (a press of the
//! modifier itself — `"Shift"` — is posted like any key).
//!
//! A character is mapped with the current layout (`VkKeyScanW`, which also
//! says which modifiers type it — AltGr characters set Ctrl+Alt without
//! becoming `WM_SYS*`), falling back to the US table in `table.rs`.

mod table;

use std::cell::{Cell, RefCell};

use runtime_vocabulary::caps::{KeyPress, NativeInjection};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyboardState, MapVirtualKeyW, SetKeyboardState, VkKeyScanW, MAPVK_VK_TO_VSC,
};
use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, KillTimer, PostMessageW, SetTimer, GA_ROOT};

use crate::WindowsBackend;

std::thread_local! {
    /// Keyboard-state bits an injection set, cleared by the restore timer.
    static TO_CLEAR: RefCell<Vec<u16>> = const { RefCell::new(Vec::new()) };
    /// The pending restore timer (0 = none).
    static RESTORE_TIMER: Cell<usize> = const { Cell::new(0) };
}

impl WindowsBackend {
    /// `IntrospectionOps::native_key`.
    pub(crate) fn native_key_impl(&self, press: &KeyPress) -> Option<NativeInjection> {
        if self.host_hwnd.is_invalid() {
            return None;
        }
        let host = self.host_hwnd.0 as isize;
        let press = press.clone();
        Some(Box::new(move || unsafe { inject(HWND(host as *mut std::ffi::c_void), &press) }))
    }
}

unsafe fn inject(host: HWND, press: &KeyPress) -> Result<(), String> {
    let (mut shift, mut ctrl, mut alt) = (press.shift, press.ctrl, press.alt);
    // Alt the user holds makes `WM_SYS*`; Ctrl+Alt a layout needs for an
    // AltGr character does not.
    let sys = press.alt;
    let layout = press.text().map(|c| VkKeyScanW(c as u16)).filter(|&r| r != -1);
    let vk = match layout {
        Some(r) => {
            let mods = (r as u16) >> 8;
            shift |= mods & 1 != 0;
            ctrl |= mods & 2 != 0;
            alt |= mods & 4 != 0;
            (r as u16) & 0xFF
        }
        None => table::vk_for(&press.key, &press.code)
            .ok_or_else(|| format!("no key on this keyboard types '{}'", press.key))?,
    };
    let scancode = MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC);
    let extended = table::is_extended(vk);
    let target = focus_target(host);

    let mut state = [0u8; 256];
    GetKeyboardState(&mut state).map_err(|e| format!("GetKeyboardState: {e}"))?;
    let mut set = Vec::new();
    for m in table::modifier_vks(shift, ctrl, alt, press.meta) {
        if state[m as usize] & 0x80 == 0 {
            state[m as usize] |= 0x80;
            set.push(m);
        }
    }
    if !set.is_empty() {
        SetKeyboardState(&state).map_err(|e| format!("SetKeyboardState: {e}"))?;
        TO_CLEAR.with(|c| c.borrow_mut().extend(set));
        schedule_restore();
    }
    for down in [true, false] {
        PostMessageW(
            Some(target),
            table::key_message(down, sys),
            WPARAM(vk as usize),
            LPARAM(table::key_lparam(scancode, extended, down, sys)),
        )
        .map_err(|e| format!("PostMessageW: {e}"))?;
    }
    Ok(())
}

/// The window with keyboard focus when it is in the host's window, else
/// the host.
unsafe fn focus_target(host: HWND) -> HWND {
    let focused = GetFocus();
    if !focused.is_invalid() && GetAncestor(focused, GA_ROOT) == GetAncestor(host, GA_ROOT) {
        focused
    } else {
        host
    }
}

/// Arm the thread timer that clears the injected modifier bits once the
/// queue has drained (see the module docs). One at a time: a press queued
/// behind another shares the pending restore.
unsafe fn schedule_restore() {
    if RESTORE_TIMER.with(|t| t.get()) != 0 {
        return;
    }
    let id = SetTimer(None, 0, 0, Some(restore));
    RESTORE_TIMER.with(|t| t.set(id));
}

unsafe extern "system" fn restore(_: HWND, _: u32, id: usize, _: u32) {
    let _ = KillTimer(None, id);
    RESTORE_TIMER.with(|t| t.set(0));
    let bits = TO_CLEAR.with(|c| std::mem::take(&mut *c.borrow_mut()));
    let mut state = [0u8; 256];
    if GetKeyboardState(&mut state).is_err() {
        return;
    }
    for vk in bits {
        state[vk as usize] &= !0x80;
    }
    let _ = SetKeyboardState(&state);
}

#[cfg(test)]
mod tests {
    //! The live path: a real `EDIT` with focus, the injection, and a pump
    //! like the host's (`TranslateMessage` + `DispatchMessageW`). Needs real
    //! HWNDs, so it runs on a Windows target only (Wine on the Mac:
    //! `cargo test --target x86_64-pc-windows-gnu` with a Wine runner). The
    //! tables are tested on every host by `tests/native_key_tables.rs`.
    use super::*;
    use runtime_vocabulary::caps::{IntrospectionOps, KeyChord};
    use windows::core::w;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, SetFocus, VK_SHIFT};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, DispatchMessageW, GetWindowTextW, PeekMessageW, SendMessageW,
        TranslateMessage, MSG, PM_REMOVE, WINDOW_EX_STYLE, WM_KEYDOWN, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
    };

    /// Pump until the restore timer has run (bounded), recording whether
    /// Shift read as held when each key-down was retrieved.
    unsafe fn pump(shift_at_down: &mut Vec<bool>) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut msg = MSG::default();
        while std::time::Instant::now() < deadline {
            if PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_KEYDOWN {
                    shift_at_down.push(GetKeyState(VK_SHIFT.0 as i32) < 0);
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            } else if RESTORE_TIMER.with(|t| t.get()) == 0 {
                return;
            } else {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
        panic!("the restore timer never fired");
    }

    fn text(hwnd: HWND) -> String {
        let mut buf = [0u16; 64];
        let n = unsafe { GetWindowTextW(hwnd, &mut buf) } as usize;
        String::from_utf16_lossy(&buf[..n])
    }

    /// A key reaches the focused field as typing does: `TranslateMessage`
    /// types the capital with the injected Shift, Backspace deletes, and
    /// the Shift bit is gone once the queue drained.
    #[test]
    fn injected_keys_type_into_the_focused_edit_and_release_shift() {
        let _serial = crate::render_test_lock();
        let host = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("native-key-test"),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                0,
                0,
                400,
                300,
                None,
                None,
                None,
                None,
            )
        }
        .expect("host window");
        let mut be = WindowsBackend::new(host);
        let input = be.create_text_input("ab", None, std::rc::Rc::new(|_| {}), None, None, false, &Default::default());
        unsafe {
            let _ = SetFocus(Some(input.hwnd));
            // Caret at the end.
            SendMessageW(input.hwnd, crate::EM_SETSEL, Some(WPARAM(2)), Some(LPARAM(2)));
        }
        let mut shift_at_down = Vec::new();
        for key in ["A", "Backspace", "c"] {
            let press = KeyChord::parse(key).unwrap().resolve(false);
            be.native_key(&input, &press).expect("injection")().expect("posted");
            unsafe { pump(&mut shift_at_down) };
        }
        assert_eq!(text(input.hwnd), "abc");
        assert_eq!(shift_at_down, [true, false, false], "Shift held for the capital only");
        assert!(unsafe { GetKeyState(VK_SHIFT.0 as i32) } >= 0, "the injected Shift is released");
        unsafe {
            let _ = DestroyWindow(host);
        }
    }
}
