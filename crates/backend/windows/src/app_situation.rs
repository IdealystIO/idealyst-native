//! Host facts about the app's situation (`runtime_shared::app_state`),
//! read from Win32 and reported as they change.
//!
//! | Fact | Source |
//! |---|---|
//! | `Focused` | `WM_ACTIVATE` (`WA_INACTIVE` → false, else true); boot: `GetForegroundWindow() == host` |
//! | `Visible` | `WM_SIZE` `SIZE_MINIMIZED` → false, `SIZE_RESTORED`/`SIZE_MAXIMIZED` → true; boot: `IsIconic` |
//! | `ReduceMotion` | `SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION)` off → true; re-read on `WM_SETTINGCHANGE` |
//! | `PrimaryPointer` | `GetSystemMetrics(SM_MOUSEPRESENT)` → Fine, else a ready touch digitizer (`SM_DIGITIZER`) → Coarse, else None; re-read on `WM_SETTINGCHANGE` / `WM_DEVICECHANGE` |
//!
//! `UserActivated` is never reported: a native window has no autoplay
//! gate (the default `true` is the truth). Occlusion by other windows is
//! not observable through plain Win32 messages (DWM cloaking aside), so a
//! covered-but-not-minimized window stays `visible` — the same answer a
//! browser gives for a window behind another one.
//!
//! The host shell calls [`window_message`] from its `WndProc` for the host
//! window; each report schedules a flush ([`crate::newcore::schedule_flush`])
//! so the per-world signals commit.

use runtime_shared::app_state::{self, AppStateChange};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetSystemMetrics, IsIconic, SystemParametersInfoW, SM_DIGITIZER,
    SM_MOUSEPRESENT, SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WA_INACTIVE,
    WM_ACTIVATE, WM_SETTINGCHANGE, WM_SIZE,
};

use crate::pointer_policy as policy;

/// `WM_DEVICECHANGE` (`dbt.h`); broadcast to top-level windows when a
/// device (a mouse) is added or removed.
const WM_DEVICECHANGE: u32 = 0x0219;

fn read_reduce_motion() -> bool {
    let mut on = windows::core::BOOL(1);
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(&mut on as *mut _ as *mut std::ffi::c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    // Unreadable (very old Windows): assume animations are on.
    policy::reduce_motion(ok.is_err() || on.as_bool())
}

fn read_pointer_kind() -> runtime_shared::app_state::PointerKind {
    unsafe { policy::pointer_kind(GetSystemMetrics(SM_MOUSEPRESENT) != 0, GetSystemMetrics(SM_DIGITIZER)) }
}

/// The facts readable at boot. The caller reports them with no backend
/// borrow held ([`app_state::report`] runs world observers).
pub fn boot_facts(host: HWND) -> Vec<AppStateChange> {
    unsafe {
        vec![
            AppStateChange::Focused(GetForegroundWindow() == host),
            AppStateChange::Visible(!IsIconic(host).as_bool()),
            AppStateChange::ReduceMotion(read_reduce_motion()),
            AppStateChange::PrimaryPointer(read_pointer_kind()),
        ]
    }
}

/// The changes one host-window message implies (pure apart from the
/// settings reads). Split from [`window_message`] so tests can check the
/// mapping without a mounted world.
pub fn changes_for(msg: u32, wparam: WPARAM, _lparam: LPARAM) -> Vec<AppStateChange> {
    match msg {
        WM_ACTIVATE => vec![AppStateChange::Focused((wparam.0 & 0xFFFF) as u32 != WA_INACTIVE)],
        WM_SIZE => policy::visible_from_size(wparam.0).map(AppStateChange::Visible).into_iter().collect(),
        WM_SETTINGCHANGE => vec![
            AppStateChange::ReduceMotion(read_reduce_motion()),
            AppStateChange::PrimaryPointer(read_pointer_kind()),
        ],
        WM_DEVICECHANGE => vec![AppStateChange::PrimaryPointer(read_pointer_kind())],
        _ => Vec::new(),
    }
}

/// Host-`WndProc` hook for the host window: report what `msg` changes
/// and schedule a flush. Never consumes the message.
pub fn window_message(msg: u32, wparam: WPARAM, lparam: LPARAM) {
    let changes = changes_for(msg, wparam, lparam);
    if changes.is_empty() {
        return;
    }
    for c in changes {
        app_state::report(c);
    }
    crate::newcore::schedule_flush();
}

#[cfg(test)]
mod tests {
    //! Windows target only (Wine on the macOS dev machine). The pure
    //! mappings are also covered on every host by `tests/pointer_policy.rs`.
    use super::*;

    /// The bug: the Win32 backend reported nothing, so a minimized game
    /// kept `app_visible() == true` and could not auto-pause, and
    /// `reduce_motion()` ignored the Animation-effects setting.
    #[test]
    fn regression_win32_reports_focus_minimize_and_settings() {
        app_state::reset_for_tests();
        window_message(WM_SIZE, WPARAM(1 /* SIZE_MINIMIZED */), LPARAM(0));
        assert!(!app_state::current().visible, "minimized → not visible");
        window_message(WM_SIZE, WPARAM(0 /* SIZE_RESTORED */), LPARAM(0));
        assert!(app_state::current().visible);
        assert!(changes_for(WM_SIZE, WPARAM(3 /* SIZE_MAXSHOW */), LPARAM(0)).is_empty());

        window_message(WM_ACTIVATE, WPARAM(WA_INACTIVE as usize), LPARAM(0));
        assert!(!app_state::current().focused, "deactivated → not focused");
        window_message(WM_ACTIVATE, WPARAM(1 /* WA_ACTIVE */), LPARAM(0));
        assert!(app_state::current().focused);

        // Settings changes re-read reduce motion and the pointer kind.
        let c = changes_for(WM_SETTINGCHANGE, WPARAM(0), LPARAM(0));
        assert!(c.iter().any(|c| matches!(c, AppStateChange::ReduceMotion(_))), "{c:?}");
        assert!(c.iter().any(|c| matches!(c, AppStateChange::PrimaryPointer(_))), "{c:?}");
        let boot = boot_facts(HWND::default());
        assert_eq!(boot.len(), 4, "{boot:?}");
    }
}
