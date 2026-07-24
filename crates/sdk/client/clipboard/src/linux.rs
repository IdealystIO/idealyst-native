//! Linux (desktop) clipboard backend — `arboard`.
//!
//! `arboard` is a cross-platform desktop clipboard crate; on Linux it
//! speaks X11 (via `x11rb`) and Wayland (via the `wlr-data-control`
//! protocol / GTK clipboard, depending on the compositor), so one backend
//! covers both display servers. This SDK only needs plain text, which is
//! `arboard`'s `get_text` / `set_text` surface.
//!
//! `arboard`'s API is synchronous. Clipboard ops are cheap, so — matching
//! the other native arms in this crate (Apple / Android do their work
//! synchronously inside the returned future, no executor) — we call
//! `arboard` directly inside the `async fn` rather than offloading to a
//! blocking pool. No Tokio/async runtime is assumed to be present.
//!
//! # Wayland/X11 ownership caveat
//!
//! A clipboard on Linux needs a *live display connection* — an X11 server
//! or a Wayland compositor. In a headless process (no `DISPLAY` /
//! `WAYLAND_DISPLAY`) `Clipboard::new()` fails; that surfaces as
//! [`ClipboardError::Backend`].
//!
//! On both X11 and Wayland the clipboard has no independent store: its
//! contents are *served on demand* by whichever client currently owns the
//! selection. Because these are stateless free functions, [`set_text`]
//! drops its `Clipboard` (and thus releases selection ownership) as soon as
//! it returns. On a normal desktop a **clipboard manager** — GNOME/KDE's
//! built-in one, `klipper`, `wl-clip-persist`, etc. — immediately takes
//! ownership of the offered text so it survives for other apps to paste;
//! that is the standard mechanism copy/paste relies on. On a bare session
//! with *no* clipboard manager (e.g. a nested/headless compositor), the
//! text is dropped the instant `set_text` returns, so a subsequent
//! [`text`] sees nothing. We deliberately do NOT use arboard's
//! `SetExtLinux::wait()` to hold ownership: it blocks the calling thread
//! until another client claims the clipboard, which would hang a quick
//! async op indefinitely.

use arboard::{Clipboard, Error as ArboardError};

use crate::ClipboardError;

/// Map an `arboard::Error` to this crate's `ClipboardError`.
///
/// Every arboard failure is a live-platform backend failure, so they all
/// map to [`ClipboardError::Backend`] with the platform detail preserved.
/// (`ContentNotAvailable` is handled by the caller as `Ok(None)` before it
/// ever reaches here — an empty/non-text clipboard is not an error.)
fn map_err(e: ArboardError) -> ClipboardError {
    ClipboardError::Backend(format!("arboard: {e}"))
}

pub(crate) async fn set_text(text: &str) -> Result<(), ClipboardError> {
    let mut clipboard = Clipboard::new().map_err(map_err)?;
    clipboard.set_text(text.to_string()).map_err(map_err)
}

pub(crate) async fn text() -> Result<Option<String>, ClipboardError> {
    let mut clipboard = Clipboard::new().map_err(map_err)?;
    match clipboard.get_text() {
        Ok(s) if s.is_empty() => Ok(None),
        Ok(s) => Ok(Some(s)),
        // No text on the clipboard (it's empty or holds only a non-text
        // representation like an image). The paste contract reports this as
        // `Ok(None)`, matching the web / Android backends' "absent → None".
        Err(ArboardError::ContentNotAvailable) => Ok(None),
        Err(e) => Err(map_err(e)),
    }
}
