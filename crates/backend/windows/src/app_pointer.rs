//! App-level pointer source: mouse / trackpad / pen buttons, movement and
//! wheel for the whole window, plus pointer lock and cursor hiding,
//! delivered into the framework's [`PointerSink`]
//! (`AppEnvOps::set_pointer_sink` / `set_pointer_lock` / `set_cursor_hidden`).
//!
//! ## Why the message PUMP (like the keyboard)
//!
//! Win32 sends a mouse message to the window UNDER the cursor — a child
//! control (`EDIT`, `BUTTON`, trackbar) when the pointer is over one — so
//! a host `WndProc` hook would go deaf over every native control. Every
//! mouse message of the thread passes through the host's `GetMessageW`
//! loop, though, so the host offers each one to
//! [`PointerSlot::pre_dispatch`] before `TranslateMessage` /
//! `DispatchMessageW` — the seam `app_keys.rs` already uses. Positions are
//! mapped from the target window's client area into the HOST client area
//! (the app root). The host is DPI-unaware (see `OnResizeEvent` in
//! `lib.rs`), so client pixels ARE logical pixels.
//!
//! ## PreventDefault
//!
//! A prevented down is not dispatched: the control under the pointer never
//! sees it (no focus change, no text selection, no button press). For a
//! SECONDARY down the matching up is swallowed too — `DefWindowProc`
//! turns `WM_RBUTTONUP` into `WM_CONTEXTMENU` (sent, not posted, to the
//! control itself — an `EDIT` opens its Cut/Copy/Paste menu from it), so
//! withholding the up is what suppresses the menu on EVERY window,
//! including child controls whose `WM_CONTEXTMENU` the host never sees. A
//! prevented wheel is not dispatched (nothing scrolls). Moves cannot be
//! prevented.
//!
//! ## Pointer lock
//!
//! - `ClipCursor` to a 1×1 rectangle at the centre of the client area:
//!   the cursor is held in place and cannot leave the window (no
//!   re-centring loop needed).
//! - Raw input (`RegisterRawInputDevices`, HID usage page 1 / usage 2 —
//!   the generic mouse) → `WM_INPUT` → `GetRawInputData` gives the device
//!   movement BEFORE pointer acceleration, reported as `Move` `dx`/`dy`
//!   (`pointer_policy::raw_mouse_delta`). `WM_MOUSEMOVE` is not reported
//!   while locked (the clipped cursor does not move).
//! - The cursor is hidden through `WM_SETCURSOR` (below).
//! - Granted only to the foreground window (as the browser requires focus).
//!   `lock_changed(true)` is reported from a microtask: the request arrives
//!   through the dispatcher while it holds a mutable borrow of the backend.
//! - Dropped — and reported `lock_changed(false)` — on
//!   `WM_ACTIVATE(WA_INACTIVE)` (Alt+Tab, Win key, another window
//!   activating). `WM_CAPTURECHANGED` is not involved: the lock never
//!   takes mouse capture (`ClipCursor` keeps the cursor inside, so capture
//!   would add nothing), so there is no capture to lose. Release
//!   un-clips, unregisters raw input (`RIDEV_REMOVE`) and puts the cursor
//!   back where it was when the lock began.
//!
//! ## Cursor hiding
//!
//! `WM_SETCURSOR` over the client area (`HTCLIENT`) answers
//! `SetCursor(NULL)` + "handled" while hidden or locked. Chosen over
//! `ShowCursor(FALSE)`: `ShowCursor` is a process-wide display COUNTER —
//! every hide must be balanced by exactly one show, it hides the cursor
//! over the title bar and other windows of the thread too, and an
//! unbalanced count (an early return, a crash path) leaves the cursor gone.
//! `WM_SETCURSOR` is asked every time the cursor moves over the window, so
//! hiding is scoped to the client area and showing again is just "stop
//! answering" (plus one nudge so it reappears without waiting for motion).
//! `DefWindowProc` of a child control forwards `WM_SETCURSOR` to its
//! parent first, so the host's answer also wins over child controls.
//!
//! ## Re-entrancy
//!
//! Shared slot like `KeyboardSlot`: the pump and `WndProc` reach it
//! without borrowing the backend, and the slot borrow is released before
//! the sink runs (a listener may add/remove listeners, which writes this
//! slot through `set_pointer_sink`).

use std::cell::RefCell;
use std::rc::Rc;

use runtime_shared::pointer_input::{AppPointerEvent, MouseButton, PointerOutcome, PointerPhase, PointerSink};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{ClientToScreen, MapWindowPoints, ScreenToClient};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_LWIN, VK_MENU, VK_RWIN};
use windows::Win32::UI::Input::{
    GetRawInputData, RegisterRawInputDevices, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER,
    RIDEV_REMOVE, RID_INPUT, RIM_TYPEMOUSE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    ClipCursor, GetAncestor, GetClientRect, GetCursorPos, GetForegroundWindow, GetMessageExtraInfo,
    GetSystemMetrics, SetCursor, SetCursorPos, GA_ROOT, HCURSOR, HTCLIENT, MSG, SM_CXSCREEN,
    SM_CXVIRTUALSCREEN, SM_CYSCREEN, SM_CYVIRTUALSCREEN, WA_INACTIVE, WM_ACTIVATE, WM_MOVE,
    WM_SETCURSOR, WM_SIZE,
};

use crate::pointer_policy::{self as policy, MouseMsg};

/// HID generic-desktop usage page / mouse usage — the raw-input device the
/// lock listens to.
const HID_USAGE_PAGE_GENERIC: u16 = 0x01;
const HID_USAGE_GENERIC_MOUSE: u16 = 0x02;

struct Lock {
    /// Lock point, host client coordinates (what `x`/`y` report).
    client_point: (f32, f32),
    /// Where the cursor was when the lock began (screen coordinates).
    restore: Option<POINT>,
    /// Previous absolute raw report (`MOUSE_MOVE_ABSOLUTE` devices).
    prev_abs: Option<(f32, f32)>,
}

struct Inner {
    host: HWND,
    sink: Option<PointerSink>,
    /// Last position (host client coordinates).
    last: Option<(f32, f32)>,
    /// Secondary downs that were prevented: swallow the matching up.
    swallow_secondary_up: bool,
    hidden: bool,
    lock: Option<Lock>,
}

/// The installed sink plus lock / cursor state, shared between the backend
/// (writes it from the `AppEnvOps` methods) and the host shell (its pump
/// calls [`pre_dispatch`](Self::pre_dispatch), its `WndProc`
/// [`window_message`](Self::window_message)). Cloning shares the slot.
#[derive(Clone)]
pub struct PointerSlot(Rc<RefCell<Inner>>);

impl PointerSlot {
    pub(crate) fn new(host: HWND) -> Self {
        PointerSlot(Rc::new(RefCell::new(Inner {
            host,
            sink: None,
            last: None,
            swallow_secondary_up: false,
            hidden: false,
            lock: None,
        })))
    }

    fn sink(&self) -> Option<PointerSink> {
        self.0.borrow().sink.clone()
    }

    /// `true` while a sink is installed.
    pub fn is_installed(&self) -> bool {
        self.0.borrow().sink.is_some()
    }

    /// `true` while the pointer is locked.
    pub fn is_locked(&self) -> bool {
        self.0.borrow().lock.is_some()
    }

    /// `true` while the cursor is hidden over the client area (hidden or locked).
    pub fn cursor_suppressed(&self) -> bool {
        let s = self.0.borrow();
        s.hidden || s.lock.is_some()
    }

    /// Install / remove the sink. Removal releases a held lock (silently —
    /// the dispatcher only removes the source when no lock is wanted) and
    /// shows the cursor again.
    pub(crate) fn set_sink(&self, sink: Option<PointerSink>) {
        let removing = sink.is_none();
        {
            let mut s = self.0.borrow_mut();
            s.sink = sink;
            s.last = None;
            s.swallow_secondary_up = false;
        }
        if removing {
            self.release_lock(true);
            self.set_cursor_hidden(false);
        }
    }

    /// Hide / show the cursor over the client area.
    pub(crate) fn set_cursor_hidden(&self, hidden: bool) {
        let was = self.cursor_suppressed();
        self.0.borrow_mut().hidden = hidden;
        self.refresh_cursor(was);
    }

    /// Apply a change of [`cursor_suppressed`](Self::cursor_suppressed)
    /// immediately rather than at the next mouse move.
    fn refresh_cursor(&self, was_suppressed: bool) {
        let now = self.cursor_suppressed();
        if now == was_suppressed || !self.cursor_over_client() {
            return;
        }
        unsafe {
            if now {
                SetCursor(HCURSOR::default());
            } else {
                // Re-setting the position makes Windows send WM_SETCURSOR
                // to the window under the cursor, which picks its own
                // (class / control) cursor again.
                let mut p = POINT::default();
                if GetCursorPos(&mut p).is_ok() {
                    let _ = SetCursorPos(p.x, p.y);
                }
            }
        }
    }

    fn cursor_over_client(&self) -> bool {
        let host = self.0.borrow().host;
        unsafe {
            let mut p = POINT::default();
            if GetCursorPos(&mut p).is_err() || !ScreenToClient(host, &mut p).as_bool() {
                return false;
            }
            let mut rc = RECT::default();
            let _ = GetClientRect(host, &mut rc);
            p.x >= rc.left && p.x < rc.right && p.y >= rc.top && p.y < rc.bottom
        }
    }

    /// Request (`true`) or release the lock. A grant is reported from a
    /// microtask; a refusal reports `false` (never a fake `true`).
    pub(crate) fn set_pointer_lock(&self, locked: bool) {
        let Some(sink) = self.sink() else { return };
        let report = if locked {
            if self.is_locked() {
                return;
            }
            self.acquire_lock()
        } else {
            if !self.release_lock(true) {
                return;
            }
            false
        };
        runtime_shared::scheduling::schedule_microtask(move || sink.lock_changed(report));
    }

    fn acquire_lock(&self) -> bool {
        let was = self.cursor_suppressed();
        let host = self.0.borrow().host;
        unsafe {
            // Only the window the user is working in may take the pointer.
            if GetForegroundWindow() != host {
                return false;
            }
            let device = RAWINPUTDEVICE {
                usUsagePage: HID_USAGE_PAGE_GENERIC,
                usUsage: HID_USAGE_GENERIC_MOUSE,
                dwFlags: Default::default(),
                hwndTarget: host,
            };
            if RegisterRawInputDevices(&[device], std::mem::size_of::<RAWINPUTDEVICE>() as u32).is_err() {
                return false;
            }
            let mut rc = RECT::default();
            let _ = GetClientRect(host, &mut rc);
            let (cx, cy) = ((rc.right - rc.left) / 2, (rc.bottom - rc.top) / 2);
            let mut restore = POINT::default();
            let restore = GetCursorPos(&mut restore).is_ok().then_some(restore);
            self.0.borrow_mut().lock = Some(Lock {
                client_point: (cx as f32, cy as f32),
                restore,
                prev_abs: None,
            });
            self.clip();
        }
        self.refresh_cursor(was);
        true
    }

    /// (Re)apply the 1×1 clip at the lock point — also after the window
    /// moves or resizes while locked.
    fn clip(&self) {
        let s = self.0.borrow();
        if s.lock.is_none() {
            return;
        }
        unsafe {
            let mut rc = RECT::default();
            let _ = GetClientRect(s.host, &mut rc);
            let mut tl = POINT { x: rc.left, y: rc.top };
            let mut br = POINT { x: rc.right, y: rc.bottom };
            let _ = ClientToScreen(s.host, &mut tl);
            let _ = ClientToScreen(s.host, &mut br);
            let (x, y) = policy::lock_point((tl.x, tl.y, br.x, br.y));
            let clip = RECT { left: x, top: y, right: x + 1, bottom: y + 1 };
            let _ = ClipCursor(Some(&clip));
        }
    }

    /// Drop the lock if held. Returns whether one was held.
    fn release_lock(&self, restore_cursor: bool) -> bool {
        let was = self.cursor_suppressed();
        let Some(lock) = self.0.borrow_mut().lock.take() else {
            return false;
        };
        unsafe {
            let _ = ClipCursor(None);
            let device = RAWINPUTDEVICE {
                usUsagePage: HID_USAGE_PAGE_GENERIC,
                usUsage: HID_USAGE_GENERIC_MOUSE,
                dwFlags: RIDEV_REMOVE,
                // RIDEV_REMOVE requires a null target.
                hwndTarget: HWND::default(),
            };
            let _ = RegisterRawInputDevices(&[device], std::mem::size_of::<RAWINPUTDEVICE>() as u32);
            if let (true, Some(p)) = (restore_cursor, lock.restore) {
                let _ = SetCursorPos(p.x, p.y);
            }
        }
        self.refresh_cursor(was);
        true
    }

    /// Release everything before the process exits (host `WM_DESTROY`).
    /// The clip rectangle is global to the desktop: it must not outlive
    /// the app.
    pub fn shutdown(&self) {
        self.release_lock(false);
    }

    /// Offer one retrieved message to the pointer sink. Returns `true` when
    /// the message must be swallowed (skip `TranslateMessage` AND
    /// `DispatchMessageW`). Everything else returns `false` untouched.
    pub fn pre_dispatch(&self, msg: &MSG) -> bool {
        if msg.message == policy::WM_INPUT {
            self.raw_input(msg.lParam);
            // Still dispatched: DefWindowProc releases the raw-input data.
            return false;
        }
        let Some(kind) = policy::classify(msg.message, msg.wParam.0) else {
            return false;
        };
        let Some(sink) = self.sink() else {
            return false;
        };
        let host = self.0.borrow().host;
        unsafe {
            // Only this app's window tree (not another top-level window of
            // the thread).
            if msg.hwnd != host && GetAncestor(msg.hwnd, GA_ROOT) != host {
                return false;
            }
            if policy::is_touch_synthesized(GetMessageExtraInfo().0 as usize) {
                return false;
            }
        }
        let pos = unsafe { self.host_point(msg, kind) };
        let mods = |e: AppPointerEvent| with_modifiers(e, msg.wParam.0);
        let locked_point = self.0.borrow().lock.as_ref().map(|l| l.client_point);

        match kind {
            MouseMsg::Move => {
                if locked_point.is_some() {
                    // Locked: movement comes from raw input; the clipped
                    // cursor stays put. Keep hover churn off the controls.
                    return true;
                }
                let delta = {
                    let mut s = self.0.borrow_mut();
                    let d = policy::motion_delta(s.last, pos);
                    s.last = Some(pos);
                    d
                };
                if let Some((dx, dy)) = delta {
                    sink.event(&mods(AppPointerEvent { dx, dy, ..AppPointerEvent::new(PointerPhase::Move, pos.0, pos.1) }));
                }
                false
            }
            MouseMsg::Button { button, down } => {
                let (x, y) = locked_point.unwrap_or(pos);
                if locked_point.is_none() {
                    self.0.borrow_mut().last = Some(pos);
                }
                let phase = if down { PointerPhase::Down } else { PointerPhase::Up };
                let outcome =
                    sink.event(&mods(AppPointerEvent { button: Some(button), ..AppPointerEvent::new(phase, x, y) }));
                let prevented = outcome == PointerOutcome::PreventDefault;
                if button == MouseButton::Secondary {
                    let mut s = self.0.borrow_mut();
                    if down {
                        s.swallow_secondary_up = prevented;
                    } else if std::mem::take(&mut s.swallow_secondary_up) {
                        return true; // no WM_CONTEXTMENU — module docs
                    }
                }
                down && prevented
            }
            MouseMsg::Wheel { horizontal } => {
                let delta = ((msg.wParam.0 >> 16) & 0xFFFF) as u16 as i16;
                let (wx, wy) = policy::wheel_px(delta, horizontal);
                let (x, y) = locked_point.unwrap_or(pos);
                let e = AppPointerEvent { wheel_dx: wx, wheel_dy: wy, ..AppPointerEvent::new(PointerPhase::Wheel, x, y) };
                sink.event(&mods(e)) == PointerOutcome::PreventDefault
            }
        }
    }

    /// The message's point in HOST client coordinates.
    unsafe fn host_point(&self, msg: &MSG, kind: MouseMsg) -> (f32, f32) {
        let host = self.0.borrow().host;
        let (x, y) = policy::lparam_point(msg.lParam.0);
        let mut p = POINT { x, y };
        unsafe {
            if matches!(kind, MouseMsg::Wheel { .. }) {
                // Wheel messages carry SCREEN coordinates.
                let _ = ScreenToClient(host, &mut p);
            } else if msg.hwnd != host {
                MapWindowPoints(msg.hwnd, host, std::slice::from_mut(&mut p));
            }
        }
        (p.x as f32, p.y as f32)
    }

    /// `WM_INPUT` while locked → `Move` with the raw device movement.
    fn raw_input(&self, lparam: LPARAM) {
        if !self.is_locked() {
            return;
        }
        let Some(sink) = self.sink() else { return };
        let mut raw = RAWINPUT::default();
        let mut size = std::mem::size_of::<RAWINPUT>() as u32;
        let read = unsafe {
            GetRawInputData(
                HRAWINPUT(lparam.0 as *mut std::ffi::c_void),
                RID_INPUT,
                Some(&mut raw as *mut RAWINPUT as *mut std::ffi::c_void),
                &mut size,
                std::mem::size_of::<RAWINPUTHEADER>() as u32,
            )
        };
        if read == u32::MAX || read == 0 || raw.header.dwType != RIM_TYPEMOUSE.0 {
            return;
        }
        let mouse = unsafe { raw.data.mouse };
        let flags = mouse.usFlags.0;
        let extent = unsafe {
            if flags & policy::MOUSE_VIRTUAL_DESKTOP != 0 {
                (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN))
            } else {
                (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN))
            }
        };
        let (point, (dx, dy)) = {
            let mut s = self.0.borrow_mut();
            let Some(lock) = s.lock.as_mut() else { return };
            let d = policy::raw_mouse_delta(flags, mouse.lLastX, mouse.lLastY, extent, &mut lock.prev_abs);
            (lock.client_point, d)
        };
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        let e = AppPointerEvent { dx, dy, ..AppPointerEvent::new(PointerPhase::Move, point.0, point.1) };
        sink.event(&with_modifiers(e, current_mk_flags()));
    }

    /// Host-`WndProc` hook. Returns `Some(result)` when the message is
    /// fully handled here (the caller returns it without running its own
    /// handling or `DefWindowProc`), `None` to continue as usual.
    pub fn window_message(&self, hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_SETCURSOR => {
                let hit = (lparam.0 as usize & 0xFFFF) as u32;
                if hit == HTCLIENT && self.cursor_suppressed() {
                    unsafe { SetCursor(HCURSOR::default()) };
                    return Some(LRESULT(1));
                }
                None
            }
            WM_ACTIVATE if (wparam.0 & 0xFFFF) as u32 == WA_INACTIVE => {
                let _ = hwnd;
                self.focus_lost();
                None
            }
            WM_MOVE | WM_SIZE => {
                if self.is_locked() {
                    self.clip();
                }
                None
            }
            _ => None,
        }
    }

    /// The window was deactivated: drop the lock (reported), then release
    /// held buttons.
    pub fn focus_lost(&self) {
        let Some(sink) = self.sink() else { return };
        if self.release_lock(false) {
            sink.lock_changed(false);
        }
        self.0.borrow_mut().swallow_secondary_up = false;
        sink.focus_lost();
    }
}

/// `MK_*` flags for the current modifier state (raw input carries none).
fn current_mk_flags() -> usize {
    let held = |vk: u16| unsafe { GetKeyState(vk as i32) } < 0;
    (if held(0x10) { 0x0004 } else { 0 }) | (if held(0x11) { 0x0008 } else { 0 })
}

fn with_modifiers(mut e: AppPointerEvent, wparam: usize) -> AppPointerEvent {
    let (shift, ctrl) = policy::wparam_shift_ctrl(wparam);
    let held = |vk: u16| unsafe { GetKeyState(vk as i32) } < 0;
    e.shift = shift;
    e.ctrl = ctrl;
    e.alt = held(VK_MENU.0);
    e.meta = held(VK_LWIN.0) || held(VK_RWIN.0);
    e
}

#[cfg(test)]
mod tests {
    //! Runs only on a Windows target (the crate is `cfg(windows)`); on the
    //! macOS dev machine it runs under Wine via
    //! `cargo test --target x86_64-pc-windows-gnu` with a Wine runner. Real
    //! HWNDs; messages are handed to the slot exactly as the host pump
    //! would after `GetMessageW`. `WM_INPUT` cannot be synthesized (its
    //! `lParam` is a kernel raw-input handle), so the raw-delta rules are
    //! covered by `tests/pointer_policy.rs` instead.
    use super::*;
    use std::cell::Cell;
    use windows::core::PCWSTR;
    use windows::Win32::UI::Input::GetRegisteredRawInputDevices;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, GetClipCursor, SetForegroundWindow, ShowWindow, SW_SHOW,
        WINDOW_EX_STYLE, WS_CHILD, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
    };

    #[derive(Debug, Clone, PartialEq)]
    enum Seen {
        Ev(AppPointerEvent),
        Lock(bool),
        FocusLost,
    }

    fn window(parent: Option<HWND>, x: i32, y: i32, w: i32, h: i32) -> HWND {
        let style = if parent.is_some() { WS_CHILD | WS_VISIBLE } else { WS_OVERLAPPEDWINDOW | WS_VISIBLE };
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(windows::core::w!("STATIC").as_ptr()),
                PCWSTR(windows::core::w!("pointer-test").as_ptr()),
                style,
                x,
                y,
                w,
                h,
                parent.unwrap_or_default(),
                None,
                None,
                None,
            )
        }
        .expect("window")
    }

    fn msg(hwnd: HWND, message: u32, wparam: usize, x: i32, y: i32) -> MSG {
        let lparam = ((x as u16 as u32) | ((y as u16 as u32) << 16)) as i32 as isize;
        MSG { hwnd, message, wParam: WPARAM(wparam), lParam: LPARAM(lparam), ..Default::default() }
    }

    fn recording(log: Rc<RefCell<Vec<Seen>>>, prevent: Rc<Cell<bool>>) -> PointerSink {
        let (l1, l2, l3) = (log.clone(), log.clone(), log);
        PointerSink::new(
            move |e| {
                l1.borrow_mut().push(Seen::Ev(e.clone()));
                let claim = e.button == Some(MouseButton::Secondary) || e.phase == PointerPhase::Wheel;
                if prevent.get() && claim {
                    PointerOutcome::PreventDefault
                } else {
                    PointerOutcome::Default
                }
            },
            move |v| l2.borrow_mut().push(Seen::Lock(v)),
            move || l3.borrow_mut().push(Seen::FocusLost),
        )
    }

    fn take(log: &Rc<RefCell<Vec<Seen>>>) -> Vec<Seen> {
        std::mem::take(&mut *log.borrow_mut())
    }

    fn ev(log: &Rc<RefCell<Vec<Seen>>>) -> AppPointerEvent {
        match take(log).as_slice() {
            [Seen::Ev(e)] => e.clone(),
            other => panic!("expected one event, got {other:?}"),
        }
    }

    fn raw_mouse_registrations() -> u32 {
        let mut n = 0u32;
        unsafe { GetRegisteredRawInputDevices(None, &mut n, std::mem::size_of::<RAWINPUTDEVICE>() as u32) };
        n
    }

    #[test]
    fn regression_win32_app_pointer_delivers_buttons_moves_wheel_and_locks() {
        let host = window(None, 0, 0, 400, 300);
        let child = window(Some(host), 50, 40, 100, 100);
        let other = window(None, 0, 0, 200, 200);
        let slot = PointerSlot::new(host);
        let log = Rc::new(RefCell::new(Vec::new()));
        let prevent = Rc::new(Cell::new(true));

        // No sink: nothing is consumed or delivered.
        assert!(!slot.pre_dispatch(&msg(host, policy::WM_RBUTTONDOWN, 0, 1, 1)));

        slot.set_sink(Some(recording(log.clone(), prevent.clone())));

        // Moves: first one has no jump; deltas after; a repeat is dropped.
        assert!(!slot.pre_dispatch(&msg(host, policy::WM_MOUSEMOVE, 0, 100, 100)));
        let m = ev(&log);
        assert_eq!((m.phase, m.x, m.y, m.dx, m.dy), (PointerPhase::Move, 100.0, 100.0, 0.0, 0.0));
        slot.pre_dispatch(&msg(host, policy::WM_MOUSEMOVE, 0x0004 /* MK_SHIFT */, 110, 95));
        let m = ev(&log);
        assert_eq!((m.dx, m.dy, m.shift), (10.0, -5.0, true));
        slot.pre_dispatch(&msg(host, policy::WM_MOUSEMOVE, 0, 110, 95));
        assert!(take(&log).is_empty(), "a WM_MOUSEMOVE without motion is not movement");

        // Over a child control: child client coords → host client coords.
        slot.pre_dispatch(&msg(child, policy::WM_MOUSEMOVE, 0, 10, 10));
        let m = ev(&log);
        assert_eq!((m.x, m.y), (60.0, 50.0), "mapped into the app root");

        // Primary: delivered, never swallowed (Default).
        assert!(!slot.pre_dispatch(&msg(child, policy::WM_LBUTTONDOWN, 0x0001, 10, 10)));
        assert_eq!(ev(&log).button, Some(MouseButton::Primary));
        assert!(!slot.pre_dispatch(&msg(child, policy::WM_LBUTTONUP, 0, 10, 10)));
        assert_eq!(ev(&log).phase, PointerPhase::Up);

        // Secondary, prevented: the down AND its up are swallowed, so
        // DefWindowProc never produces WM_CONTEXTMENU.
        assert!(slot.pre_dispatch(&msg(child, policy::WM_RBUTTONDOWN, 0x0002, 10, 10)), "prevented down swallowed");
        assert_eq!(ev(&log).button, Some(MouseButton::Secondary));
        assert!(slot.pre_dispatch(&msg(child, policy::WM_RBUTTONUP, 0, 10, 10)), "its up swallowed too");
        assert_eq!(ev(&log).phase, PointerPhase::Up, "the up is still delivered to the sink");
        prevent.set(false);
        assert!(!slot.pre_dispatch(&msg(child, policy::WM_RBUTTONDOWN, 0x0002, 10, 10)));
        assert!(!slot.pre_dispatch(&msg(child, policy::WM_RBUTTONUP, 0, 10, 10)), "unprevented: menu allowed");
        take(&log);
        prevent.set(true);

        // Side buttons: HIWORD(wParam) XBUTTON1 = back, XBUTTON2 = forward.
        slot.pre_dispatch(&msg(host, policy::WM_XBUTTONDOWN, 1 << 16, 5, 5));
        assert_eq!(ev(&log).button, Some(MouseButton::Back));
        slot.pre_dispatch(&msg(host, policy::WM_XBUTTONUP, 2 << 16, 5, 5));
        let e = ev(&log);
        assert_eq!((e.phase, e.button), (PointerPhase::Up, Some(MouseButton::Forward)));

        // Wheel toward the user (negative WHEEL_DELTA) = content down = +dy;
        // screen coordinates → client; prevented → swallowed.
        let mut p = POINT { x: 30, y: 20 };
        let _ = unsafe { ClientToScreen(host, &mut p) };
        let toward_user = ((-120i16) as u16 as usize) << 16;
        assert!(slot.pre_dispatch(&msg(child, policy::WM_MOUSEWHEEL, toward_user, p.x, p.y)), "prevented wheel swallowed");
        let w = ev(&log);
        assert_eq!((w.phase, w.wheel_dx, w.wheel_dy, w.x, w.y), (PointerPhase::Wheel, 0.0, 40.0, 30.0, 20.0));

        // Another top-level window of the thread is not this app.
        assert!(!slot.pre_dispatch(&msg(other, policy::WM_LBUTTONDOWN, 1, 5, 5)));
        assert!(take(&log).is_empty());

        // Cursor hiding answers WM_SETCURSOR over the client area only.
        let set_cursor = |hit: isize| slot.window_message(host, WM_SETCURSOR, WPARAM(host.0 as usize), LPARAM(hit));
        assert_eq!(set_cursor(HTCLIENT as isize), None);
        slot.set_cursor_hidden(true);
        assert_eq!(set_cursor(HTCLIENT as isize), Some(LRESULT(1)), "hidden over the client area");
        assert_eq!(set_cursor(2 /* HTCAPTION */), None, "title bar keeps its cursor");
        slot.set_cursor_hidden(false);
        assert_eq!(set_cursor(HTCLIENT as isize), None);

        // Lock: needs the foreground window.
        unsafe {
            let _ = ShowWindow(host, SW_SHOW);
            let _ = SetForegroundWindow(host);
        }
        let before = raw_mouse_registrations();
        slot.set_pointer_lock(true);
        if unsafe { GetForegroundWindow() } != host {
            assert_eq!(take(&log), vec![Seen::Lock(false)], "a refused lock reports false, never true");
            eprintln!("SKIP(lock section): could not make the test window foreground");
        } else {
            // No scheduler in unit tests: the microtask ran inline.
            assert_eq!(take(&log), vec![Seen::Lock(true)]);
            assert!(slot.is_locked());
            let mut clip = RECT::default();
            unsafe { GetClipCursor(&mut clip).unwrap() };
            assert_eq!((clip.right - clip.left, clip.bottom - clip.top), (1, 1), "cursor held at one point: {clip:?}");
            assert_eq!(raw_mouse_registrations(), before + 1, "raw mouse input registered");
            assert_eq!(set_cursor(HTCLIENT as isize), Some(LRESULT(1)), "a lock hides the cursor");
            // Cursor messages don't move a locked pointer.
            assert!(slot.pre_dispatch(&msg(host, policy::WM_MOUSEMOVE, 0, 7, 7)));
            assert!(take(&log).is_empty());
            // Buttons report the lock point.
            slot.pre_dispatch(&msg(host, policy::WM_LBUTTONDOWN, 1, 7, 7));
            let d = ev(&log);
            let mut rc = RECT::default();
            unsafe { GetClientRect(host, &mut rc).unwrap() };
            assert_eq!((d.x, d.y), ((rc.right / 2) as f32, (rc.bottom / 2) as f32));

            // Alt-tab: the platform drops the lock and reports it, then
            // releases held buttons.
            slot.window_message(host, WM_ACTIVATE, WPARAM(WA_INACTIVE as usize), LPARAM(0));
            assert_eq!(take(&log), vec![Seen::Lock(false), Seen::FocusLost]);
            assert!(!slot.is_locked());
            unsafe { GetClipCursor(&mut clip).unwrap() };
            assert!(clip.right - clip.left > 1, "clip released: {clip:?}");
            assert_eq!(raw_mouse_registrations(), before, "raw input unregistered");
            assert_eq!(set_cursor(HTCLIENT as isize), None, "cursor shown again");

            // Explicit release reports false once.
            unsafe { let _ = SetForegroundWindow(host); }
            slot.set_pointer_lock(true);
            assert_eq!(take(&log), vec![Seen::Lock(true)]);
            slot.set_pointer_lock(false);
            assert_eq!(take(&log), vec![Seen::Lock(false)]);
            slot.set_pointer_lock(false);
            assert!(take(&log).is_empty(), "releasing an unlocked pointer reports nothing");
        }

        // None: nothing delivered, nothing swallowed.
        slot.set_sink(None);
        assert!(!slot.is_installed());
        assert!(!slot.pre_dispatch(&msg(host, policy::WM_RBUTTONDOWN, 2, 1, 1)));
        assert!(take(&log).is_empty());
        unsafe {
            let _ = DestroyWindow(other);
            let _ = DestroyWindow(host);
        }
    }

    #[test]
    fn a_listener_may_replace_the_sink_from_inside_delivery() {
        let host = window(None, 0, 0, 100, 100);
        let slot = PointerSlot::new(host);
        let inner = slot.clone();
        slot.set_sink(Some(PointerSink::new(
            move |_| {
                inner.set_sink(None);
                PointerOutcome::Default
            },
            |_| {},
            || {},
        )));
        slot.pre_dispatch(&msg(host, policy::WM_LBUTTONDOWN, 1, 1, 1));
        assert!(!slot.is_installed());
        unsafe { let _ = DestroyWindow(host); }
    }
}
