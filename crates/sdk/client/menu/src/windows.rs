//! Windows implementation of the menu-bar SDK.
//!
//! Builds an `HMENU` tree from the supplied [`MenuBarSpec`], wires
//! each command's callback into [`WindowsBackend::dispatch_command`]
//! via a unique control id (Win32's `WM_COMMAND` carries the id in
//! `LOWORD(wParam)`), and installs the bar on the host window via
//! [`SetMenu`]. The bar appears as a traditional in-window menu strip
//! below the title bar — the Windows equivalent of macOS's system
//! menu bar.
//!
//! # WM_COMMAND routing
//!
//! Every command item gets its own u16 id allocated by
//! `WindowsBackend::alloc_control_id`. The closure is stored in the
//! backend's existing `command_handlers` map (the same one button
//! clicks use), so the host's WndProc just calls
//! `backend.dispatch_command(LOWORD(wParam))` from its `WM_COMMAND`
//! arm — same dispatch path as buttons; no separate menu-routing
//! pipeline.
//!
//! # Keyboard shortcuts
//!
//! V1 burns the shortcut into the menu item's label via the standard
//! tab-separated suffix Windows expects (e.g. `"&Save\tCtrl+S"`).
//! That renders the accelerator hint in the menu and lets the user
//! see what key fires the command. **Hooking the accelerator into
//! the message loop requires an accelerator table** (`HACCEL` via
//! `CreateAcceleratorTable` + `TranslateAccelerator` in the host's
//! GetMessage loop) — that's host-side plumbing, not SDK-side, and
//! lands when a `host-win32` crate exists.
//!
//! # Re-installation
//!
//! Calling [`install`] a second time replaces the previous menu.
//! `SetMenu(hwnd, new)` swaps atomically; the previous HMENU is
//! destroyed via `DestroyMenu` if we tracked it (we do, via the
//! thread-local anchor).

use crate::{MenuBarSpec, MenuCommand, MenuItem, Modifiers, Shortcut};
use backend_windows::WindowsBackend;
use std::cell::RefCell;
use windows::core::PCWSTR;
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateMenu, CreatePopupMenu, DestroyMenu, DrawMenuBar, SetMenu, HMENU,
    MF_ENABLED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING,
};

/// Install (or replace) the application's menu bar on the host
/// window. Idempotent — call again with a new spec to swap the bar
/// atomically.
pub fn install(backend: &mut WindowsBackend, spec: MenuBarSpec) {
    // Build the new top-level HMENU first; if anything fails partway
    // we DestroyMenu it before bailing rather than leaving the host
    // window with a half-built bar.
    let new_menu = match unsafe { CreateMenu() } {
        Ok(m) => m,
        Err(_) => return,
    };

    for menu in &spec.menus {
        let popup = match unsafe { CreatePopupMenu() } {
            Ok(p) => p,
            Err(_) => {
                let _ = unsafe { DestroyMenu(new_menu) };
                return;
            }
        };
        populate_popup(&popup, &menu.items, backend);
        let label = wide(&menu.title);
        unsafe {
            let _ = AppendMenuW(
                new_menu,
                MF_POPUP,
                popup.0 as usize,
                PCWSTR(label.as_ptr()),
            );
        }
    }

    // Attach. SetMenu retains its HMENU; the previous bar (if any)
    // is reachable via `LAST_MENU` and we DestroyMenu it below to
    // release the OS resources.
    let hwnd = backend.host_hwnd();
    let _ = unsafe { SetMenu(hwnd, new_menu) };
    // Force a redraw so the new bar appears immediately. SetMenu
    // marks the window as needing a non-client redraw on the next
    // message-loop tick; DrawMenuBar nudges it now so the user
    // doesn't see a stale bar before the next event.
    let _ = unsafe { DrawMenuBar(hwnd) };

    LAST_MENU.with(|slot| {
        if let Some(prev) = slot.borrow_mut().take() {
            // Destroy the old HMENU now that the new one is attached.
            // Win32 docs: SetMenu doesn't destroy the previous menu;
            // the app is responsible.
            let _ = unsafe { DestroyMenu(prev) };
        }
        *slot.borrow_mut() = Some(new_menu);
    });
}

thread_local! {
    /// Anchor for the last-installed HMENU so the next `install` can
    /// `DestroyMenu` it cleanly. Holding the HMENU here also ensures
    /// the OS resource lifetime matches our intent (alive while the
    /// bar is attached; destroyed atomically on replacement).
    static LAST_MENU: RefCell<Option<HMENU>> = const { RefCell::new(None) };
}

// =========================================================================
// Popup-menu population
// =========================================================================

fn populate_popup(popup: &HMENU, items: &[MenuItem], backend: &mut WindowsBackend) {
    for item in items {
        match item {
            MenuItem::Command(cmd) => append_command(popup, cmd, backend),
            MenuItem::Separator => unsafe {
                let _ = AppendMenuW(*popup, MF_SEPARATOR, 0, PCWSTR::null());
            },
            MenuItem::Submenu(sub) => {
                let inner = match unsafe { CreatePopupMenu() } {
                    Ok(p) => p,
                    Err(_) => continue,
                };
                populate_popup(&inner, &sub.items, backend);
                let label = wide(&sub.title);
                unsafe {
                    let _ = AppendMenuW(
                        *popup,
                        MF_POPUP,
                        inner.0 as usize,
                        PCWSTR(label.as_ptr()),
                    );
                }
            }
        }
    }
}

fn append_command(popup: &HMENU, cmd: &MenuCommand, backend: &mut WindowsBackend) {
    // Build the displayed label. Win32 splits "label\tshortcut" at
    // the tab character; the shortcut appears right-aligned in the
    // popup, matching native apps. The shortcut here is purely
    // visual — actual key dispatch needs a HACCEL table on the host.
    let label = match &cmd.shortcut {
        Some(s) => format!("{}\t{}", cmd.label, format_shortcut(s)),
        None => cmd.label.clone(),
    };
    let label_wide = wide(&label);

    // Allocate a control id only if the command has a callback. No
    // callback = no dispatch path = no id needed (saves slots in the
    // u16 namespace).
    let control_id = match &cmd.on_click {
        Some(cb) => backend.register_command_handler(cb.clone()),
        None => 0,
    };

    let mut flags = MF_STRING;
    if !cmd.enabled {
        flags |= MF_GRAYED;
    } else {
        flags |= MF_ENABLED;
    }
    unsafe {
        let _ = AppendMenuW(
            *popup,
            flags,
            control_id as usize,
            PCWSTR(label_wide.as_ptr()),
        );
    }
}

/// Render a shortcut as the suffix Win32 expects in menu labels.
/// Examples: `"Ctrl+S"`, `"Ctrl+Shift+Z"`, `"Alt+F4"`.
///
/// Note: on Windows, our `Modifiers::COMMAND` maps to `Ctrl` (the
/// platform's primary modifier, per the `Shortcut` docs). Author
/// code that writes `Shortcut::cmd('s')` gets the natural Ctrl+S
/// here without per-platform branching.
fn format_shortcut(s: &Shortcut) -> String {
    let mut parts: Vec<&'static str> = Vec::new();
    if s.modifiers.contains(Modifiers::CONTROL) || s.modifiers.contains(Modifiers::COMMAND) {
        parts.push("Ctrl");
    }
    if s.modifiers.contains(Modifiers::OPTION) {
        // macOS Option → Windows Alt. Same physical key on most
        // cross-platform keyboards.
        parts.push("Alt");
    }
    if s.modifiers.contains(Modifiers::SHIFT) {
        parts.push("Shift");
    }
    let key_upper = s.key.to_ascii_uppercase();
    let mut out = parts.join("+");
    if !out.is_empty() {
        out.push('+');
    }
    out.push(key_upper);
    out
}

// =========================================================================
// Wide-string helper — `AppendMenuW` takes a wide-char PCWSTR.
// =========================================================================

/// UTF-16 buffer terminated with a null. Returned as `Vec<u16>` so
/// the caller keeps it alive across the AppendMenuW call (PCWSTR is
/// a raw pointer to the buffer).
fn wide(s: &str) -> Vec<u16> {
    let mut buf: Vec<u16> = s.encode_utf16().collect();
    buf.push(0);
    buf
}
