//! Fallback backend for targets without a file-open UI. [`pick`] reports
//! [`PickError::Unsupported`] so author code degrades predictably; the
//! `PickedFile`/`FileStream` types exist only to satisfy the public wrappers
//! and are never constructed (so their methods are unreachable).

use std::path::Path;

use crate::{PickError, PickRequest};

pub(crate) async fn pick(_request: &PickRequest) -> Result<Option<Vec<PickedFile>>, PickError> {
    Err(PickError::Unsupported)
}

/// Never constructed — `pick` errors before any file is produced.
pub(crate) struct PickedFile {
    _never: (),
}

impl PickedFile {
    pub(crate) fn name(&self) -> &str {
        unreachable!("file-picker stub: PickedFile is never constructed")
    }
    pub(crate) fn mime(&self) -> &str {
        unreachable!("file-picker stub: PickedFile is never constructed")
    }
    pub(crate) fn size(&self) -> Option<u64> {
        unreachable!("file-picker stub: PickedFile is never constructed")
    }
    pub(crate) fn path(&self) -> Option<&Path> {
        unreachable!("file-picker stub: PickedFile is never constructed")
    }
    pub(crate) async fn open(&self) -> Result<FileStream, PickError> {
        unreachable!("file-picker stub: PickedFile is never constructed")
    }
}

/// Never constructed.
pub(crate) struct FileStream {
    _never: (),
}

impl FileStream {
    pub(crate) async fn chunk(&mut self) -> Result<Option<Vec<u8>>, PickError> {
        unreachable!("file-picker stub: FileStream is never constructed")
    }
}
