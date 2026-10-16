//! The app's identity to Windows shell features — its AppUserModelID
//! (AUMID) and the name shown for it.
//!
//! A toast notification is attributed to an AUMID, and Windows only shows
//! toasts for one it knows. A packaged (MSIX) app gets both from its
//! manifest; an unpackaged `.exe` — every app this framework builds — has
//! neither, so `ToastNotifier::Show` silently showed nothing. The
//! documented fix for unpackaged apps is to register the id under
//! `HKCU\Software\Classes\AppUserModelId\<id>` with a `DisplayName` (the
//! name in the toast header and the notification settings) and to set it
//! as the process's explicit AUMID (which also groups the app's windows on
//! the taskbar under it).
//!
//! The Win32 host calls [`set`] at startup with the manifest's bundle id
//! and app name; SDKs read the result with [`current`]. A process that no
//! host identified (a test binary, an embedder) gets one derived from the
//! executable's name, so SDK calls still work there.

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE,
    REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::UI::Shell::{
    GetCurrentProcessExplicitAppUserModelID, SetCurrentProcessExplicitAppUserModelID,
};

/// Identify the process as `id` (a bundle id such as
/// `"com.example.notes"`; at most 128 characters, no spaces) shown as
/// `display_name`. Registers the id for the current user and sets it as
/// the process's explicit AUMID.
pub fn set(id: &str, display_name: &str) -> windows::core::Result<()> {
    register(id, display_name)?;
    unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(id)) }
}

/// The process's AUMID: the one [`set`] (or anything else in the process)
/// established, else an id derived from the executable's file name, which
/// this registers and sets on first use.
pub fn current() -> String {
    if let Some(id) = explicit() {
        return id;
    }
    let stem = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "app".to_string());
    let id = fallback_id(&stem);
    let _ = set(&id, &stem);
    id
}

/// `idealyst.<exe stem>`, with characters an AUMID cannot hold replaced.
fn fallback_id(stem: &str) -> String {
    let clean: String = stem
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '-' })
        .collect();
    let mut id = format!("idealyst.{clean}");
    id.truncate(128);
    id
}

fn explicit() -> Option<String> {
    unsafe {
        let pwstr = GetCurrentProcessExplicitAppUserModelID().ok()?;
        let id = pwstr.to_string().ok();
        CoTaskMemFree(Some(pwstr.0 as *const _));
        id.filter(|s| !s.is_empty())
    }
}

/// Write `DisplayName` under `HKCU\Software\Classes\AppUserModelId\<id>`.
fn register(id: &str, display_name: &str) -> windows::core::Result<()> {
    let path = HSTRING::from(format!("Software\\Classes\\AppUserModelId\\{id}"));
    let mut key = HKEY::default();
    unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            &path,
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
        .ok()?;
        // REG_SZ data: UTF-16 with the terminating NUL, as bytes.
        let wide: Vec<u16> = display_name.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes = std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2);
        let set = RegSetValueExW(key, &HSTRING::from("DisplayName"), None, REG_SZ, Some(bytes)).ok();
        let _ = RegCloseKey(key);
        set
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_ids_are_valid_aumids() {
        assert_eq!(fallback_id("charts-demo-windows"), "idealyst.charts-demo-windows");
        assert_eq!(fallback_id("my app_1"), "idealyst.my-app-1");
        assert_eq!(fallback_id(&"x".repeat(300)).len(), 128);
    }

    /// `set` makes the id the process's AUMID, which `current` then
    /// reports.
    #[test]
    fn set_identifies_the_process() {
        set("idealyst.test.app-identity", "App identity test").expect("set");
        assert_eq!(current(), "idealyst.test.app-identity");
    }
}
