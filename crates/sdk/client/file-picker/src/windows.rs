//! Windows open via the Shell `IFileOpenDialog` (the native "Open" dialog).
//!
//! `Show` is modal and synchronous — it spins its own message loop until the
//! user picks file(s) or cancels (`ERROR_CANCELLED`). The chosen paths are real
//! filesystem paths, so reads stream via the shared [`fsread`](crate::fsread)
//! reader.
//!
//! VERIFICATION: type-checked for `x86_64-pc-windows-*`; the COM dialog path
//! resolves only at runtime on Windows.

use std::path::PathBuf;

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{ERROR_CANCELLED, HWND};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FileOpenDialog, IFileOpenDialog, IShellItem, IShellItemArray, FILEOPENDIALOGOPTIONS,
    FOS_ALLOWMULTISELECT, FOS_FILEMUSTEXIST, SIGDN_FILESYSPATH,
};

use crate::fsread::file_meta;
use crate::{PickError, PickKind, PickRequest};

pub(crate) use crate::fsread::FileStream;

/// A file the user picked on Windows: a real path plus metadata.
pub(crate) struct PickedFile {
    name: String,
    mime: String,
    size: Option<u64>,
    path: PathBuf,
}

impl PickedFile {
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
    pub(crate) fn mime(&self) -> &str {
        &self.mime
    }
    pub(crate) fn size(&self) -> Option<u64> {
        self.size
    }
    pub(crate) fn path(&self) -> Option<&std::path::Path> {
        Some(&self.path)
    }
    pub(crate) async fn open(&self) -> Result<FileStream, PickError> {
        FileStream::open(&self.path)
    }
}

pub(crate) async fn pick(request: &PickRequest) -> Result<Option<Vec<PickedFile>>, PickError> {
    // SAFETY: the documented IFileOpenDialog flow. COM is initialized for this
    // (apartment-threaded) thread; the dialog is created, configured, shown
    // modally, and its results read.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| PickError::Backend(format!("CoCreateInstance: {e}")))?;

        // Options: files must exist; allow multi-select per the request.
        let mut opts: FILEOPENDIALOGOPTIONS = dialog
            .GetOptions()
            .map_err(|e| PickError::Backend(format!("GetOptions: {e}")))?;
        opts |= FOS_FILEMUSTEXIST;
        if request.allow_multiple {
            opts |= FOS_ALLOWMULTISELECT;
        }
        dialog
            .SetOptions(opts)
            .map_err(|e| PickError::Backend(format!("SetOptions: {e}")))?;

        // File-type filters. Keep the HSTRINGs alive for the Show() call.
        let filters = build_filters(request);
        let specs: Vec<COMDLG_FILTERSPEC> = filters
            .iter()
            .map(|(name, spec)| COMDLG_FILTERSPEC {
                pszName: PCWSTR(name.as_ptr()),
                pszSpec: PCWSTR(spec.as_ptr()),
            })
            .collect();
        if !specs.is_empty() {
            dialog
                .SetFileTypes(&specs)
                .map_err(|e| PickError::Backend(format!("SetFileTypes: {e}")))?;
        }

        match dialog.Show(HWND::default()) {
            Ok(()) => {
                let items: IShellItemArray = dialog
                    .GetResults()
                    .map_err(|e| PickError::Backend(format!("GetResults: {e}")))?;
                let count = items
                    .GetCount()
                    .map_err(|e| PickError::Backend(format!("GetCount: {e}")))?;
                let mut out = Vec::with_capacity(count as usize);
                for i in 0..count {
                    let item: IShellItem = items
                        .GetItemAt(i)
                        .map_err(|e| PickError::Backend(format!("GetItemAt: {e}")))?;
                    let pwstr = item
                        .GetDisplayName(SIGDN_FILESYSPATH)
                        .map_err(|e| PickError::Backend(format!("GetDisplayName: {e}")))?;
                    let path = pwstr
                        .to_string()
                        .map_err(|e| PickError::Backend(format!("path decode: {e}")))?;
                    CoTaskMemFree(Some(pwstr.0 as *const _));
                    let path = PathBuf::from(path);
                    let (name, mime, size) = file_meta(&path);
                    out.push(PickedFile {
                        name,
                        mime,
                        size,
                        path,
                    });
                }
                Ok(Some(out))
            }
            Err(e) if e.code() == ERROR_CANCELLED.to_hresult() => Ok(None),
            Err(e) => Err(PickError::Backend(format!("Show: {e}"))),
        }
    }
}

/// `(label, pattern)` filter specs from the request. Always appends an
/// "All files" entry (and that's the only one when unfiltered).
fn build_filters(request: &PickRequest) -> Vec<(HSTRING, HSTRING)> {
    let mimes: Vec<&str> = match &request.kind {
        PickKind::Documents(m) => m.iter().map(String::as_str).collect(),
        PickKind::Media(k) => crate::mime::media_mimes(*k).to_vec(),
    };
    let mut out = Vec::new();
    for mime in &mimes {
        if let Some((label, pattern)) = crate::mime::win_filter(mime) {
            out.push((HSTRING::from(label), HSTRING::from(pattern)));
        }
    }
    out.push((HSTRING::from("All files"), HSTRING::from("*.*")));
    out
}
