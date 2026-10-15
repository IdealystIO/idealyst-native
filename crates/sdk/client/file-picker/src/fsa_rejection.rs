//! What a rejected File System Access pick means for the web backend: try the
//! `<input type=file>` fallback, resolve as dismissed, or fail.
//!
//! Pure decision logic, split out of `web.rs` so it is unit-testable natively
//! (`web.rs` only compiles on `wasm32`).
//!
//! # The bug this prevents
//!
//! `pick()` used to fall back to the hidden `<input type=file>` on ANY
//! rejection other than `AbortError`. A rejection that arrives AFTER the
//! `showOpenFilePicker()` dialog was shown — the user already chose a file —
//! then opened a second dialog: the first choice was thrown away, and
//! cancelling the second resolved the whole call as dismissed. Seen in VS
//! Code's embedded browser (Electron), where the embedder's `fileSystem`
//! permission check runs after the user picks and can reject the pick (or the
//! handle's `getFile()`) with `NotAllowedError`.
//!
//! # Which rejections prove no dialog was shown
//!
//! Per the WICG File System Access spec, `showOpenFilePicker(options)` does
//! these checks synchronously, BEFORE the "in parallel" step that displays the
//! prompt, and rejects with:
//!
//! - `TypeError` — processing `options.types` failed (an invalid MIME type or
//!   extension in `accept`).
//! - `SecurityError` — the environment's origin is opaque, the document is a
//!   cross-origin subframe (Chromium), or the window has no transient
//!   activation ("must be handling a user gesture").
//!
//! Those two are safe to fall back from: no dialog was shown, and the
//! `<input>` (which has no origin restriction and a more lenient `accept`) may
//! still work. Everything else can come from after the dialog:
//!
//! - `AbortError` — the user dismissed the dialog (or Chromium's
//!   sensitive-directory refusal, which also shows UI first). → dismissed.
//! - `NotAllowedError` — the pick's permission grant was refused after the
//!   user chose (Electron's `fileSystem` permission request carries the chosen
//!   `filePath`, so it necessarily runs post-dialog), or Chromium's "File
//!   picker already active" (another dialog is already up — opening an
//!   `<input>` dialog on top of it is exactly the double-dialog bug). → fail.
//! - Anything unknown → fail. Erring toward an error the app can show beats
//!   silently opening a second picker.
//!
//! Every rejection from a handle's `getFile()` (stage [`FsaStage::Read`]) is
//! after the dialog by construction — the handles came from it — so it never
//! falls back.

/// Where in the File System Access pick the rejection came from.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum FsaStage {
    /// `showOpenFilePicker()` itself rejected.
    Picker,
    /// The picker returned handles; a handle's `getFile()` rejected.
    Read,
}

/// What `pick()` does with a rejected File System Access pick.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum FsaRejection {
    /// No dialog was shown — try the `<input type=file>` fallback.
    Fallback,
    /// The user dismissed the dialog — resolve as cancelled.
    Dismissed,
    /// A dialog may have been shown — surface the error; never open another.
    Fail,
}

/// Classify a rejection by its stage and its `DOMException` / `Error` name.
pub(crate) fn classify_fsa_rejection(stage: FsaStage, name: &str) -> FsaRejection {
    match (stage, name) {
        // `getFile()` on a handle the dialog returned: always post-dialog.
        (FsaStage::Read, _) => FsaRejection::Fail,
        (FsaStage::Picker, "AbortError") => FsaRejection::Dismissed,
        // The spec's pre-prompt checks (see the module docs).
        (FsaStage::Picker, "SecurityError" | "TypeError") => FsaRejection::Fallback,
        (FsaStage::Picker, _) => FsaRejection::Fail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reported bug: a rejection after the dialog was shown (Electron's
    /// post-pick `NotAllowedError`, a `getFile()` failure on a returned
    /// handle, or an unrecognized name) fell through to the `<input>` and
    /// opened a second dialog. None of them may classify as `Fallback`.
    #[test]
    fn regression_post_dialog_rejection_never_reopens_picker() {
        for name in ["NotAllowedError", "NotReadableError", "NotFoundError", "InvalidStateError", "UnknownError", ""] {
            assert_eq!(
                classify_fsa_rejection(FsaStage::Picker, name),
                FsaRejection::Fail,
                "picker rejection {name:?} must not reopen a picker"
            );
        }
        for name in ["NotAllowedError", "NotReadableError", "NotFoundError", "SecurityError", "TypeError", "AbortError", ""] {
            assert_eq!(
                classify_fsa_rejection(FsaStage::Read, name),
                FsaRejection::Fail,
                "getFile() rejection {name:?} comes after the dialog and must not reopen a picker"
            );
        }
    }

    #[test]
    fn pre_dialog_rejections_fall_back_to_the_input() {
        assert_eq!(classify_fsa_rejection(FsaStage::Picker, "SecurityError"), FsaRejection::Fallback);
        assert_eq!(classify_fsa_rejection(FsaStage::Picker, "TypeError"), FsaRejection::Fallback);
    }

    #[test]
    fn abort_from_the_picker_is_a_dismiss() {
        assert_eq!(classify_fsa_rejection(FsaStage::Picker, "AbortError"), FsaRejection::Dismissed);
    }
}
