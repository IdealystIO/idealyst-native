//! Windows save via the Shell `IFileSaveDialog` (the native "Save As" dialog).
//!
//! `Show` is modal and synchronous — it spins its own message loop until the
//! user picks a path or cancels (`ERROR_CANCELLED`). We then write the bytes
//! to the chosen path.
//!
//! VERIFICATION: type-checked for `x86_64-pc-windows-*`; the COM dialog path
//! resolves only at runtime on Windows.

use windows::core::HSTRING;
use windows::Win32::Foundation::{ERROR_CANCELLED, HWND};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::{FileSaveDialog, IFileSaveDialog, SIGDN_FILESYSPATH};

use crate::{ExportError, SaveOutcome, SaveRequest, Source};

pub(crate) async fn save(request: SaveRequest) -> Result<SaveOutcome, ExportError> {
    let bytes = match request.source {
        Source::Bytes(b) => b,
        Source::Path(p) => std::fs::read(&p).map_err(|e| ExportError::Io(e.to_string()))?,
    };

    // SAFETY: the documented IFileSaveDialog flow. COM is initialized for this
    // (apartment-threaded) thread; the dialog is created, configured, shown
    // modally, and its result read.
    unsafe {
        // Ignore the HRESULT — repeat init on an already-initialized thread is
        // a benign S_FALSE, and author code may have initialized COM already.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        let dialog: IFileSaveDialog =
            CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| ExportError::Backend(format!("CoCreateInstance: {e}")))?;

        let name = HSTRING::from(request.suggested_name.as_str());
        dialog
            .SetFileName(&name)
            .map_err(|e| ExportError::Backend(format!("SetFileName: {e}")))?;

        match dialog.Show(HWND::default()) {
            Ok(()) => {
                let item = dialog
                    .GetResult()
                    .map_err(|e| ExportError::Backend(format!("GetResult: {e}")))?;
                let pwstr = item
                    .GetDisplayName(SIGDN_FILESYSPATH)
                    .map_err(|e| ExportError::Backend(format!("GetDisplayName: {e}")))?;
                let path = pwstr
                    .to_string()
                    .map_err(|e| ExportError::Backend(format!("path decode: {e}")))?;
                CoTaskMemFree(Some(pwstr.0 as *const _));

                std::fs::write(&path, &bytes).map_err(|e| ExportError::Io(e.to_string()))?;
                Ok(SaveOutcome::Saved {
                    location: Some(path),
                })
            }
            Err(e) if e.code() == ERROR_CANCELLED.to_hresult() => Ok(SaveOutcome::Cancelled),
            Err(e) => Err(ExportError::Backend(format!("Show: {e}"))),
        }
    }
}
