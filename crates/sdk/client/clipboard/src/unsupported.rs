//! Fallback backend for native targets with no clipboard implementation
//! (anything that is not web, Apple, Android, Linux or Windows). Both ops
//! return
//! [`ClipboardError::NotSupported`] so callers can branch honestly rather
//! than silently no-op'ing.

use crate::ClipboardError;

pub(crate) async fn set_text(_text: &str) -> Result<(), ClipboardError> {
    Err(ClipboardError::NotSupported)
}

pub(crate) async fn text() -> Result<Option<String>, ClipboardError> {
    Err(ClipboardError::NotSupported)
}
