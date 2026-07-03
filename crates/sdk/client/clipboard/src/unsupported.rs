//! Fallback backend for native targets without an in-scope clipboard
//! implementation (Windows, Linux, other non-Apple/Android desktops).
//!
//! A cross-platform desktop clipboard (X11/Wayland/Win32) is deliberately
//! out of scope for this SDK — it would pull in a heavyweight platform
//! crate for a surface the framework's primary targets (web / iOS / macOS
//! / Android) already cover. Both ops return
//! [`ClipboardError::NotSupported`] so callers can branch honestly rather
//! than silently no-op'ing.

use crate::ClipboardError;

pub(crate) async fn set_text(_text: &str) -> Result<(), ClipboardError> {
    Err(ClipboardError::NotSupported)
}

pub(crate) async fn text() -> Result<Option<String>, ClipboardError> {
    Err(ClipboardError::NotSupported)
}
