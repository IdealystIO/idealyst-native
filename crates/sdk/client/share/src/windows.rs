//! Windows share — the Windows share sheet (`DataTransferManager`).
//!
//! A desktop app reaches the share sheet through
//! `IDataTransferManagerInterop` for its window: `GetForWindow` gives the
//! window's `DataTransferManager`, whose `DataRequested` event fills the
//! package when `ShowShareUIForWindow` opens the sheet. The window is the
//! calling thread's active window — `share` is called from the UI thread,
//! on a user action, so that is the app's window (else the thread's first
//! visible top-level window).
//!
//! - **Content**: title (required by Windows; defaults to the text or URL
//!   when unset), text, the URL as a web link, files as storage items. The
//!   files are opened (`StorageFile::GetFileFromPathAsync`) before the sheet
//!   opens, so the synchronous `DataRequested` handler only attaches them; a
//!   missing file fails the share as `Backend`.
//! - **Outcome**: the package's `ShareCompleted` → `Completed`,
//!   `ShareCanceled` (Windows 11) → `Dismissed`. Without `ShareCanceled`
//!   (Windows 10) a dismissal is indistinguishable, so the share resolves
//!   `Completed` once the sheet has opened — the crate's documented
//!   "platform can't distinguish" case.

use windows::core::{AgileReference, Interface, HSTRING};
use windows::ApplicationModel::DataTransfer::{
    DataPackage, DataRequestedEventArgs, DataTransferManager, ShareCompletedEventArgs,
};
use windows::Foundation::{TypedEventHandler, Uri};
use windows::Storage::{IStorageItem, StorageFile};
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow;
use windows::Win32::UI::WindowsAndMessaging::{EnumThreadWindows, IsWindowVisible};
use windows::Win32::UI::Shell::IDataTransferManagerInterop;

use crate::{ShareContent, ShareError, ShareOutcome};

fn backend(e: windows::core::Error) -> ShareError {
    ShareError::Backend(format!("{e}"))
}

/// The window to share from: the calling (UI) thread's active window, else
/// its first visible top-level window — `share` can run while the app is
/// not the foreground app (a scripted press, a timer).
fn app_window() -> HWND {
    let active = unsafe { GetActiveWindow() };
    if !active.is_invalid() {
        return active;
    }
    unsafe extern "system" fn first_visible(hwnd: HWND, out: LPARAM) -> BOOL {
        if unsafe { IsWindowVisible(hwnd) }.as_bool() {
            unsafe { *(out.0 as *mut HWND) = hwnd };
            return BOOL(0);
        }
        BOOL(1)
    }
    let mut found = HWND::default();
    unsafe {
        let _ = EnumThreadWindows(GetCurrentThreadId(), Some(first_visible), LPARAM(&mut found as *mut HWND as isize));
    }
    found
}

pub(crate) async fn share(content: &ShareContent) -> Result<ShareOutcome, ShareError> {
    let hwnd = app_window();
    if hwnd.is_invalid() {
        return Err(ShareError::Backend("no active window to share from".into()));
    }
    // Agile references: the handler may run on another apartment, and must
    // be `Send`.
    let mut files: Vec<AgileReference<IStorageItem>> = Vec::new();
    for path in &content.files {
        let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(path.as_os_str()))
            .map_err(backend)?
            .await
            .map_err(|e| ShareError::Backend(format!("{}: {e}", path.display())))?;
        let item: IStorageItem = file.cast().map_err(backend)?;
        files.push(AgileReference::new(&item).map_err(backend)?);
    }

    let interop = windows::core::factory::<DataTransferManager, IDataTransferManagerInterop>().map_err(backend)?;
    let manager: DataTransferManager = unsafe { interop.GetForWindow(hwnd) }.map_err(backend)?;
    let (tx, rx) = async_channel::bounded::<ShareOutcome>(1);
    // Whether a package can report a cancel (`IDataPackage4`, Windows 11).
    // Probed up front: `DataRequested` may run after the sheet call returns.
    let cancel_known = DataPackage::new().and_then(|p| p.cast::<windows::ApplicationModel::DataTransfer::IDataPackage4>()).is_ok();

    let title = content
        .title
        .clone()
        .or_else(|| content.text.clone())
        .or_else(|| content.url.clone())
        .unwrap_or_else(|| "Share".to_string());
    let text = content.text.clone();
    let url = content.url.clone();
    let requested = TypedEventHandler::<DataTransferManager, DataRequestedEventArgs>::new(move |_, args| {
        let Some(args) = args.as_ref() else { return Ok(()) };
        let data: DataPackage = args.Request()?.Data()?;
        data.Properties()?.SetTitle(&HSTRING::from(title.as_str()))?;
        if let Some(t) = &text {
            data.SetText(&HSTRING::from(t.as_str()))?;
        }
        if let Some(u) = &url {
            data.SetWebLink(&Uri::CreateUri(&HSTRING::from(u.as_str()))?)?;
        }
        if !files.is_empty() {
            let items: Vec<Option<IStorageItem>> =
                files.iter().map(|f| f.resolve().map(Some)).collect::<windows::core::Result<_>>()?;
            let items: windows_collections::IIterable<IStorageItem> = items.into();
            data.SetStorageItems(&items, true)?;
        }
        let done = tx.clone();
        data.ShareCompleted(&TypedEventHandler::<DataPackage, ShareCompletedEventArgs>::new(move |_, _| {
            let _ = done.try_send(ShareOutcome::Completed);
            Ok(())
        }))?;
        if cancel_known {
            let cancelled = tx.clone();
            data.ShareCanceled(&TypedEventHandler::<DataPackage, windows::core::IInspectable>::new(move |_, _| {
                let _ = cancelled.try_send(ShareOutcome::Dismissed);
                Ok(())
            }))?;
        }
        Ok(())
    });
    let token = manager.DataRequested(&requested).map_err(backend)?;
    let shown = unsafe { interop.ShowShareUIForWindow(hwnd) };
    let outcome = match shown {
        Err(e) => Err(backend(e)),
        // Windows 10: no cancel event — the sheet opened, which is all
        // that can be told.
        Ok(()) if !cancel_known => Ok(ShareOutcome::Completed),
        Ok(()) => rx.recv().await.map_err(|_| ShareError::Backend("the share sheet went away".into())),
    };
    let _ = manager.RemoveDataRequested(token);
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows used the stub: `share` was always `NotSupported`. It now goes
    /// to the share sheet — which needs the app's window, so a test thread
    /// (no window of its own) gets that backend error rather than the stub's
    /// answer. The sheet itself is checked by hand in `examples/sdk-gallery`.
    #[test]
    fn regression_windows_share_reaches_the_share_sheet_path() {
        let r = pollster::block_on(share(&ShareContent::text("hello")));
        assert_eq!(r, Err(ShareError::Backend("no active window to share from".into())));
    }
}
