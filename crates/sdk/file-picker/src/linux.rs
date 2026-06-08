//! Linux open via the desktop **portal** (`xdg-desktop-portal`)
//! `FileChooser.OpenFile` — the sandbox-friendly, compositor-native open
//! dialog, driven through `ashpd`. Same portal posture `file-export`'s Linux
//! save takes. The portal returns `file://` URIs that map to real paths, so
//! reads stream via the shared [`fsread`](crate::fsread) reader.
//!
//! VERIFICATION: type-checked for `x86_64-unknown-linux-gnu`; the D-Bus portal
//! exchange resolves only at runtime against a running portal service.

use std::path::PathBuf;

use ashpd::desktop::file_chooser::{FileFilter, SelectedFiles};

use crate::fsread::file_meta;
use crate::{PickError, PickKind, PickRequest};

pub(crate) use crate::fsread::FileStream;

/// A file the user picked on Linux: a real path plus metadata.
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
    let mut req = SelectedFiles::open_file()
        .title("Open File")
        .multiple(request.allow_multiple);

    if let Some(filter) = build_filter(request) {
        req = req.filter(filter);
    }

    let response = req
        .send()
        .await
        .map_err(|e| PickError::Backend(format!("portal request: {e}")))?
        .response();

    let files = match response {
        Ok(files) => files,
        // The user dismissing the dialog comes back as a cancelled response.
        Err(ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled)) => {
            return Ok(None)
        }
        Err(e) => return Err(PickError::Backend(format!("portal response: {e}"))),
    };

    let mut out = Vec::new();
    for uri in files.uris() {
        let path = match uri.to_file_path() {
            Ok(p) => p,
            // Non-file URI (rare) — skip rather than fail the whole pick.
            Err(_) => continue,
        };
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

/// A portal `FileFilter` for the request, or `None` to leave it unfiltered.
/// Concrete MIME types go in by mimetype; wildcard/media types expand to globs.
fn build_filter(request: &PickRequest) -> Option<FileFilter> {
    let mimes: Vec<&str> = match &request.kind {
        PickKind::Documents(m) if m.is_empty() => return None,
        PickKind::Documents(m) => m.iter().map(String::as_str).collect(),
        PickKind::Media(k) => crate::mime::media_mimes(*k).to_vec(),
    };

    let mut filter = FileFilter::new("Files");
    let mut any = false;
    for mime in mimes {
        match crate::mime::win_filter(mime) {
            // Reuse the Windows glob table for wildcard/known types.
            Some((_label, patterns)) => {
                for glob in patterns.split(';') {
                    filter = filter.glob(glob);
                    any = true;
                }
            }
            // Untabled concrete MIME — match it directly.
            None => {
                filter = filter.mimetype(mime);
                any = true;
            }
        }
    }
    any.then_some(filter)
}
