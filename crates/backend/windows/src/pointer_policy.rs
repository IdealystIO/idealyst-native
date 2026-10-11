//! Pure Win32 → framework translation for the app-level pointer source
//! (`app_pointer.rs`) and the app-situation reports (`app_situation.rs`).
//!
//! Deliberately free of any `windows`-crate type: the crate body is
//! `cfg(target_os = "windows")`, so logic inside it could only be tested on
//! a Windows machine. This file is ALSO compiled by
//! `tests/pointer_policy.rs` via `#[path]`, which runs on any host — the
//! reachable test for the translation rules (CLAUDE.md §8).

use runtime_shared::app_state::PointerKind;
use runtime_shared::pointer_input::MouseButton;

// Message ids (`winuser.h`).
pub const WM_MOUSEMOVE: u32 = 0x0200;
pub const WM_LBUTTONDOWN: u32 = 0x0201;
pub const WM_LBUTTONUP: u32 = 0x0202;
pub const WM_LBUTTONDBLCLK: u32 = 0x0203;
pub const WM_RBUTTONDOWN: u32 = 0x0204;
pub const WM_RBUTTONUP: u32 = 0x0205;
pub const WM_RBUTTONDBLCLK: u32 = 0x0206;
pub const WM_MBUTTONDOWN: u32 = 0x0207;
pub const WM_MBUTTONUP: u32 = 0x0208;
pub const WM_MBUTTONDBLCLK: u32 = 0x0209;
pub const WM_MOUSEWHEEL: u32 = 0x020A;
pub const WM_XBUTTONDOWN: u32 = 0x020B;
pub const WM_XBUTTONUP: u32 = 0x020C;
pub const WM_XBUTTONDBLCLK: u32 = 0x020D;
pub const WM_MOUSEHWHEEL: u32 = 0x020E;
pub const WM_INPUT: u32 = 0x00FF;

/// `WHEEL_DELTA`: one wheel notch in `WM_MOUSEWHEEL` units.
pub const WHEEL_DELTA: f32 = 120.0;
/// Logical pixels one notch scrolls: Windows' default "3 lines" at ~13⅓ px
/// a line — the same 40 px the host's own view scrolling uses, and the
/// same constant the GTK backend applies, so a notch is one distance on
/// every desktop. High-resolution wheels send fractions of
/// [`WHEEL_DELTA`] and scale proportionally.
pub const WHEEL_NOTCH_PX: f32 = runtime_shared::pointer_input::WHEEL_NOTCH_PX;

/// What a mouse message is.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MouseMsg {
    Button { button: MouseButton, down: bool },
    Move,
    /// `horizontal` = `WM_MOUSEHWHEEL`.
    Wheel { horizontal: bool },
}

/// Classify a retrieved message. `wparam` is needed for the X buttons
/// (`HIWORD(wParam)`: `XBUTTON1` = 1 = back, `XBUTTON2` = 2 = forward).
/// Double-click messages (sent instead of the second down to windows whose
/// class has `CS_DBLCLKS`, e.g. some common controls) are downs: the
/// dispatcher wants every press.
pub fn classify(msg: u32, wparam: usize) -> Option<MouseMsg> {
    let button = |button, down| Some(MouseMsg::Button { button, down });
    match msg {
        WM_MOUSEMOVE => Some(MouseMsg::Move),
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => button(MouseButton::Primary, true),
        WM_LBUTTONUP => button(MouseButton::Primary, false),
        WM_RBUTTONDOWN | WM_RBUTTONDBLCLK => button(MouseButton::Secondary, true),
        WM_RBUTTONUP => button(MouseButton::Secondary, false),
        WM_MBUTTONDOWN | WM_MBUTTONDBLCLK => button(MouseButton::Middle, true),
        WM_MBUTTONUP => button(MouseButton::Middle, false),
        WM_XBUTTONDOWN | WM_XBUTTONDBLCLK | WM_XBUTTONUP => {
            let b = match (wparam >> 16) & 0xFFFF {
                1 => MouseButton::Back,
                2 => MouseButton::Forward,
                _ => return None,
            };
            button(b, msg != WM_XBUTTONUP)
        }
        WM_MOUSEWHEEL => Some(MouseMsg::Wheel { horizontal: false }),
        WM_MOUSEHWHEEL => Some(MouseMsg::Wheel { horizontal: true }),
        _ => None,
    }
}

/// The signed point packed in a mouse message's `lParam`
/// (`GET_X_LPARAM` / `GET_Y_LPARAM` — signed, because coordinates left of
/// or above a window, or on a monitor left of the primary, are negative).
pub fn lparam_point(lparam: isize) -> (i32, i32) {
    let l = lparam as usize;
    ((l & 0xFFFF) as u16 as i16 as i32, ((l >> 16) & 0xFFFF) as u16 as i16 as i32)
}

/// Wheel amounts in logical px with the Web `WheelEvent` sign. `delta` is
/// `GET_WHEEL_DELTA_WPARAM` (signed `HIWORD(wParam)`).
///
/// - `WM_MOUSEWHEEL`: positive = rotated AWAY from the user, which scrolls
///   content UP — the opposite of Web `deltaY`, so it is negated.
/// - `WM_MOUSEHWHEEL`: positive = tilted RIGHT, which scrolls content right
///   — the same sign as Web `deltaX`.
pub fn wheel_px(delta: i16, horizontal: bool) -> (f32, f32) {
    let px = delta as f32 / WHEEL_DELTA * WHEEL_NOTCH_PX;
    if horizontal {
        (px, 0.0)
    } else {
        (0.0, -px)
    }
}

/// `wParam` key-state bits of mouse messages (`MK_SHIFT`, `MK_CONTROL`).
/// Alt and the Windows key are not in `wParam`; the caller reads them.
pub fn wparam_shift_ctrl(wparam: usize) -> (bool, bool) {
    (wparam & 0x0004 != 0, wparam & 0x0008 != 0)
}

/// Whether a mouse message was SYNTHESIZED from a finger touch.
///
/// Windows promotes touch and pen input to mouse messages and marks them
/// in `GetMessageExtraInfo()`: the upper 24 bits are the signature
/// `MI_WP_SIGNATURE` (`0xFF515700`), and bit 7 distinguishes touch (set)
/// from pen (clear). Finger touches belong to the view-level touch APIs;
/// pen input IS pointer input and passes.
pub fn is_touch_synthesized(extra_info: usize) -> bool {
    const SIGNATURE_MASK: usize = 0xFFFF_FF00;
    const MI_WP_SIGNATURE: usize = 0xFF51_5700;
    const TOUCH_BIT: usize = 0x80;
    (extra_info & SIGNATURE_MASK) == MI_WP_SIGNATURE && extra_info & TOUCH_BIT != 0
}

/// `RAWMOUSE.usFlags` bits.
pub const MOUSE_MOVE_ABSOLUTE: u16 = 0x01;
pub const MOUSE_VIRTUAL_DESKTOP: u16 = 0x02;

/// Raw-input mouse movement → `dx`/`dy` while locked.
///
/// A normal mouse reports RELATIVE motion (`lLastX`/`lLastY` = device
/// counts since the last report, before pointer acceleration — the "raw /
/// unaccelerated" motion the contract prefers; at the default pointer
/// speed one count is one pixel). Remote desktop, some VMs and tablets
/// report ABSOLUTE positions normalized to 0..=65535 over the primary
/// monitor (or the virtual desktop with `MOUSE_VIRTUAL_DESKTOP`); those
/// are turned into pixels against `extent` and differenced against the
/// previous absolute report (`prev_abs`). The first absolute report only
/// seeds `prev_abs`.
pub fn raw_mouse_delta(
    flags: u16,
    last_x: i32,
    last_y: i32,
    extent: (i32, i32),
    prev_abs: &mut Option<(f32, f32)>,
) -> (f32, f32) {
    if flags & MOUSE_MOVE_ABSOLUTE == 0 {
        return (last_x as f32, last_y as f32);
    }
    let x = last_x as f32 / 65535.0 * extent.0 as f32;
    let y = last_y as f32 / 65535.0 * extent.1 as f32;
    let d = match *prev_abs {
        Some((px, py)) => (x - px, y - py),
        None => (0.0, 0.0),
    };
    *prev_abs = Some((x, y));
    d
}

/// The point a lock holds the cursor at: the centre of the client rect
/// (given in SCREEN coordinates as left, top, right, bottom).
pub fn lock_point(client_screen: (i32, i32, i32, i32)) -> (i32, i32) {
    let (l, t, r, b) = client_screen;
    (l + (r - l).max(0) / 2, t + (b - t).max(0) / 2)
}

/// `WM_SIZE` `wParam` → visibility: `SIZE_MINIMIZED` (1) hides the app,
/// `SIZE_RESTORED` (0) / `SIZE_MAXIMIZED` (2) show it. `SIZE_MAXSHOW` /
/// `SIZE_MAXHIDE` (3/4) are about OTHER windows — no change.
pub fn visible_from_size(wparam: usize) -> Option<bool> {
    match wparam {
        1 => Some(false),
        0 | 2 => Some(true),
        _ => None,
    }
}

/// `SPI_GETCLIENTAREAANIMATION` → reduce motion. That flag is Settings →
/// Accessibility → Visual effects → "Animation effects"; off means the
/// user asked for less motion (it is what Chromium maps
/// `prefers-reduced-motion` to on Windows).
pub fn reduce_motion(client_area_animation: bool) -> bool {
    !client_area_animation
}

/// The primary pointer from `GetSystemMetrics`: `SM_MOUSEPRESENT` → Fine;
/// otherwise a ready touch digitizer (`SM_DIGITIZER` has `NID_READY` and
/// `NID_INTEGRATED_TOUCH` / `NID_EXTERNAL_TOUCH`) → Coarse; else None.
pub fn pointer_kind(mouse_present: bool, digitizer: i32) -> PointerKind {
    const NID_INTEGRATED_TOUCH: i32 = 0x01;
    const NID_EXTERNAL_TOUCH: i32 = 0x02;
    const NID_READY: i32 = 0x80;
    if mouse_present {
        PointerKind::Fine
    } else if digitizer & NID_READY != 0 && digitizer & (NID_INTEGRATED_TOUCH | NID_EXTERNAL_TOUCH) != 0 {
        PointerKind::Coarse
    } else {
        PointerKind::None
    }
}

/// Movement since the previous event. `None` for a repeat at the same
/// position: Windows re-sends `WM_MOUSEMOVE` without motion (after a
/// cursor change, window activation, `SetCursorPos`), which is not
/// movement. The first event has no previous position and moves by 0.
pub fn motion_delta(prev: Option<(f32, f32)>, now: (f32, f32)) -> Option<(f32, f32)> {
    match prev {
        Some(p) if p == now => None,
        Some((px, py)) => Some((now.0 - px, now.1 - py)),
        None => Some((0.0, 0.0)),
    }
}
